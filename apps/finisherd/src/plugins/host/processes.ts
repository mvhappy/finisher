// The plugin layer's hold on its processes (sp8b plan Task 5): the `plugin` family is
// registered with the job engine before it starts, then bound to it; the lifecycle stops a
// plugin's processes through it (disable, revocation, removal, crash loop: running jobs are
// cancelled and its warm processes get `shutdown`, then a kill after 2 s; an update drains the
// old version's), removes a stopped key's temp directory, checks a code plugin can be enabled
// on this host, and says whether its processes are confined. Only processes the engine started
// are touched.
import { rmSync } from "node:fs";
import type { FinError, JobState } from "@finisher/contracts";
import { validateManifest } from "@finisher/fin-node";
import type { ValidationReport } from "@finisher/core-types";
import { Effect } from "effect";
import type { Audit } from "../../control/audit.ts";
import type { ControlStore } from "../../control/db.ts";
import { attemptJson, finError } from "../../errors.ts";
import type { JobEngine } from "../../jobs/api.ts";
import type { KindRegistry } from "../../jobs/kinds.ts";
import { RETIRE_GRACE_MS } from "../../jobs/warm-worker.ts";
import type { InstalledFull, PluginTables } from "../tables.ts";
import { makeCrashLoop } from "./crash-loop.ts";
import { PLUGIN_FAMILY, registerPluginFamily } from "./family.ts";
import { keyTmpDir, sessionRoots } from "./keys.ts";
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

export function makePluginHost(d: PluginHostDeps) {
  const keys = new Set<string>();
  let engine: JobEngine | undefined;
  const clock = d.clock ?? Date.now;

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
      registerPluginFamily(r, { serverData: d.serverData, installed: (id) => d.tables.installedFull(id), rootSessionOf: sessionRoots(d.control), onKey: (k) => keys.add(k) });
    },
    /** Binds the started engine: stops go to it, its job ends feed the crash loop. */
    bind(e: JobEngine): void {
      engine = e;
      e.onDone((job) => crashLoop.onDone(job));
    },
    hooks: { stop, enabled: (id: string) => crashLoop.clear(id), confined: () => engine?.workers().caps.profiles.plugin === "enforced", enableCheck } satisfies PluginHostHooks,
  };
}

export type PluginHost = ReturnType<typeof makePluginHost>;
