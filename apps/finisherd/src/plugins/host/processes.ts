// The plugin layer's hold on its processes (sp8b plan Task 5): the `plugin` family is
// registered with the job engine before it starts, then bound to it; the lifecycle stops a
// plugin's processes through it (disable, revocation, removal, crash loop: running jobs are
// cancelled and its warm processes get `shutdown`, then a kill after 2 s; an update drains the
// old version's), removes a stopped key's temp directory, checks a code plugin can be enabled
// on this host, and says whether its processes are confined. Only processes the engine started
// are touched.
//
// Authority keys (spec §3.3, Decision 28; `authority.ts`): the host owns the classes' policy
// generations (`control.db`'s `plugin_authority`). A credential minted or ended (an agent
// session tree that starts, gains a child or ends; an MCP credential revoked or expired) bumps
// its class, a device revoked bumps `person`; a bump retires the class's warm processes of
// older generations, then (once they and the class's running jobs are gone) removes their temp
// directories and the class's authority-keyed `pluginData`. The start sweeps both.
import { rmSync } from "node:fs";
import type { FinError, JobState, JobView } from "@finisher/contracts";
import { validateManifest } from "@finisher/fin-node";
import type { ValidationReport } from "@finisher/core-types";
import { Effect } from "effect";
import type { CredentialChange } from "../../auth/credentials.ts";
import type { DeviceEvent } from "../../auth/devices.ts";
import type { Audit } from "../../control/audit.ts";
import type { ControlStore } from "../../control/db.ts";
import { attemptJson, finError } from "../../errors.ts";
import type { JobEngine } from "../../jobs/api.ts";
import type { KindRegistry } from "../../jobs/kinds.ts";
import { RETIRE_GRACE_MS } from "../../jobs/warm-worker.ts";
import type { InstalledFull, PluginTables } from "../tables.ts";
import { makeCrashLoop } from "./crash-loop.ts";
import { PLUGIN_FAMILY, registerPluginFamily } from "./family.ts";
import { type Authority, type AuthorityGenerations, authorityOfWarmKey, classSlug, jobAuthority, removeClassAuthData, storedGenerations, sweepAuthData } from "./authority.ts";
import { pluginDataFor } from "./family.ts";
import { keyTmpDir, pluginWarmKey, principalClass, sessionRoots } from "./keys.ts";
import { storeDirOf } from "./program.ts";

/** The host API a code plugin must accept to be enabled (sp8b Decision 14). */
export const PLUGIN_HOST_API = "1.1.0";

/** What the lifecycle asks of the plugin processes. */
export interface PluginHostHooks {
  /**
   * Stops `id`'s processes (every version, or `version`'s); `drain` lets running jobs finish;
   * `fail`: its queued and paused jobs fail with it at once (the dispatch check stays the backstop).
   */
  stop(id: string, opts?: { readonly version?: string; readonly drain?: boolean; readonly fail?: FinError }): Effect.Effect<void>;
  /** The owner enabled `id`: its crash count starts again. */
  enabled(id: string): void;
  /** Whether plugin processes start under 7C's `plugin` profile on this host. */
  confined(): boolean;
  /** Why code plugin `row` cannot be enabled here (platform, host API, Node, `models`), if it cannot. */
  enableCheck(row: InstalledFull): Effect.Effect<void, FinError>;
}

/** The `<id>@<version>|…` part of a warm key (a confined key is `<project>/<key>`). */
export function keyPlugin(key: string): string {
  return key.startsWith("@") ? key : key.slice(key.indexOf("/") + 1);
}

export interface PluginHostDeps {
  /** Links resolved. */
  readonly serverData: string;
  readonly control: ControlStore;
  readonly tables: PluginTables;
  readonly audit?: Audit;
  readonly clock?: () => number;
}

/** How long a retired authority's temp and data wait for its last job (a command's 60 s ceiling, plus grace). */
export const AUTHORITY_DRAIN_MS = 65_000;

/** What bumps the classes' generations (`watch`). */
export interface AuthoritySources {
  readonly credentials?: { onChange(f: (c: CredentialChange) => void): () => void };
  readonly devices?: { onEvent(f: (e: DeviceEvent) => void): () => void };
}

export function makePluginHost(d: PluginHostDeps) {
  const keys = new Set<string>();
  let engine: JobEngine | undefined;
  const clock = d.clock ?? Date.now;
  const rootSessionOf = sessionRoots(d.control);
  const generations: AuthorityGenerations = storedGenerations(d.control);

  const matcher = (id: string, version?: string) => (plugin: string) => (version === undefined ? plugin.startsWith(`${id}@`) : plugin === `${id}@${version}`);

  const stop: PluginHostHooks["stop"] = (id, opts = {}) =>
    Effect.gen(function* () {
      const e = engine;
      if (e === undefined) return;
      const mine = matcher(id, opts.version);
      const ofKey = (key: string) => mine(keyPlugin(key).split("|")[0] ?? "");
      const each = (state: ReadonlyArray<JobState>, f: (id: string) => Effect.Effect<unknown, FinError>) =>
        Effect.gen(function* () {
          for (let cursor: string | undefined, page = 0; page < 100; page++) {
            const l = e.list({ state, limit: 500, ...(cursor === undefined ? {} : { cursor }) });
            for (const j of l.jobs) if (j.family === PLUGIN_FAMILY && j.via !== undefined && mine(j.via.plugin)) yield* Effect.ignore(f(j.id));
            if (l.next === undefined) break;
            cursor = l.next;
          }
        });
      // Its queued jobs fail now, before a slot frees and before a running one is cancelled.
      const fail = opts.fail;
      if (fail !== undefined) yield* each(["queued", "paused"], (j) => e.failQueued(j, fail));
      // Its running jobs are cancelled (their processes are killed after the cancel grace).
      if (opts.drain !== true) yield* each(["running"], (j) => e.cancel(j));
      e.retireWarm(PLUGIN_FAMILY, ofKey);
      // Its warm processes exit within the grace (or are killed at its end).
      const live = () => e.workers().warm.filter((w) => w.family === PLUGIN_FAMILY && ofKey(w.key));
      for (let waited = 0; live().length > 0 && waited < RETIRE_GRACE_MS + 1500; waited += 50) yield* Effect.sleep(50);
      // A temp directory goes only once no process can use it. `keys` holds a key from the
      // family's `program()`, which the pool calls in the same turn as it lists the new worker,
      // so a key whose worker is still spawning is in `workers().warm` here and is kept. A
      // cold or drained job still running may use its key's: then nothing is removed now
      // (the next start's sweep does it).
      if (e.list({ state: ["running"], limit: 500 }).jobs.some((j) => j.family === PLUGIN_FAMILY && j.via !== undefined && mine(j.via.plugin))) return;
      const still = new Set(live().map((w) => w.key));
      for (const key of [...keys]) {
        if (!ofKey(key) || still.has(key)) continue;
        keys.delete(key);
        rmSync(keyTmpDir(d.serverData, key), { recursive: true, force: true });
      }
    });

  /**
   * `cls`'s policy generation moves on (spec §3.3): its warm processes of older generations are
   * retired at once (a busy one after its job), and once none is left and no job of the class
   * runs (at most `AUTHORITY_DRAIN_MS`), their temp directories and the class's
   * authority-keyed `pluginData` are removed. Resolves after the removal.
   */
  const authorityChanged = (cls: string): Effect.Effect<number> =>
    Effect.gen(function* () {
      const generation = generations.bump(cls);
      const slug = classSlug(cls);
      const stale = (key: string) => {
        const a = authorityOfWarmKey(key);
        return a !== undefined && a.slug === slug && a.generation !== generation;
      };
      const e = engine;
      if (e !== undefined) {
        e.retireWarm(PLUGIN_FAMILY, stale);
        const busy = () =>
          e.workers().warm.some((w) => w.family === PLUGIN_FAMILY && stale(w.key)) ||
          e.list({ state: ["running"], limit: 500 }).jobs.some((j) => j.family === PLUGIN_FAMILY && principalClass(j.owner, rootSessionOf) === cls);
        for (let waited = 0; busy() && waited < AUTHORITY_DRAIN_MS; waited += 50) yield* Effect.sleep(50);
      }
      const still = new Set(e?.workers().warm.map((w) => w.key) ?? []);
      for (const key of [...keys]) {
        if (!stale(key) || still.has(key)) continue;
        keys.delete(key);
        yield* Effect.ignore(Effect.try(() => rmSync(keyTmpDir(d.serverData, key), { recursive: true, force: true })));
      }
      yield* Effect.ignore(Effect.try(() => removeClassAuthData(d.serverData, cls, generation)));
      return generation;
    });

  const crashLoop = makeCrashLoop({
    tables: d.tables,
    clock,
    ...(d.audit === undefined ? {} : { audit: d.audit }),
    stop: (id) => void Effect.runFork(stop(id, { fail: finError("PLUGIN_CRASH_LOOP", `${id} kept crashing and is stopped until its owner enables it`, { plugin: id }) })),
  });

  const enableCheck: PluginHostHooks["enableCheck"] = (row) =>
    Effect.gen(function* () {
      const report = yield* attemptJson<ValidationReport>(() => validateManifest(storeDirOf(d.serverData, row), row.source));
      // The host-dependent refusals first (platform, host API, Node); then anything else.
      const errors = report.errors ?? [];
      const first = errors.find((x) => x.code !== "PLUGIN_MANIFEST_INVALID") ?? errors[0];
      if (!report.ok && first !== undefined) return yield* Effect.fail(finError(first.code, first.message, { ...first.details, plugin: row.id }));
      if ((report.manifest?.models ?? []).length > 0) {
        return yield* Effect.fail(
          finError("PLUGIN_CONTRIBUTION_UNSUPPORTED", `${row.id} needs models, which this version of Finisher cannot provide yet`, { plugin: row.id, kind: "models", hostApi: PLUGIN_HOST_API }, "Keep it installed: a later Finisher update can run it."),
        );
      }
    });

  return {
    /** Registers the `plugin` family (before the engine starts) and empties the keys' temp. */
    register(r: KindRegistry): void {
      registerPluginFamily(r, { serverData: d.serverData, installed: (id) => d.tables.installedFull(id), rootSessionOf, generations, onKey: (k) => keys.add(k) });
      sweepAuthData(d.serverData, generations);
    },
    /**
     * Bumps the classes' generations from the credentials and devices: a credential minted or
     * ended bumps its class (an agent's is its session tree's root), a device revoked `person`.
     * Returns the unsubscribe.
     */
    watch(src: AuthoritySources): () => void {
      const changed = (cls: string) => void Effect.runFork(authorityChanged(cls));
      const offs = [
        src.credentials?.onChange((c) => changed(principalClass({ kind: c.kind, id: c.id }, rootSessionOf))),
        src.devices?.onEvent((ev) => {
          if (ev.kind === "revoked") changed("person");
        }),
      ];
      return () => {
        for (const off of offs) off?.();
      };
    },
    /** `cls`'s generation moves on now (`watch` calls it; tests too). */
    authorityChanged,
    /** The generations (tests; Task 7 compares a call's key with the job's live one). */
    generations,
    /** The authority `job` runs under now. */
    authorityOf: (job: JobView): Authority | undefined => jobAuthority(job, principalClass(job.owner, rootSessionOf), generations),
    /** `job`'s warm key now: a worker whose key differs serves a retired authority (Task 7). */
    warmKeyOf: (job: JobView): string => pluginWarmKey(job, rootSessionOf, generations),
    /** `PluginContext.dirs.data` of `job` (Task 7), created on first use; only with `fs pluginData`. */
    dataDirOf: (job: JobView) => pluginDataFor({ serverData: d.serverData, rootSessionOf, generations }, job),
    /** Binds the started engine: stops go to it, its job ends feed the crash loop. */
    bind(e: JobEngine): void {
      engine = e;
      e.onDone((job) => crashLoop.onDone(job));
    },
    hooks: { stop, enabled: (id: string) => crashLoop.clear(id), confined: () => engine?.workers().caps.profiles.plugin === "enforced", enableCheck } satisfies PluginHostHooks,
  };
}

export type PluginHost = ReturnType<typeof makePluginHost>;
