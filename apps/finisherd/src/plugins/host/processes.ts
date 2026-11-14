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
//
// Diagnostics (Task 15): the host, never a plugin, records `plugin.stats` each minute
// (`stats.ts`: calls, its jobs' timeouts and crashes, dropped log and stderr, egress) and
// `plugin.command` through `onCommandRun` (`command-journal.ts`); `worker.exit` and
// `plugin.exit` come from the job engine's sink (`jobs/journal-sink.ts`).
//
// Hook handlers (7F Task 10): `register` also registers the `plugin.hook` kind, which carries
// this host's handlers (`hooks.ts`, bound to the engine at `bind`) for the hook engine.
import { rmSync } from "node:fs";
import { type FinError, isJobRights, type JobState, type JobView } from "@finisher/contracts";
import { type Journal, validateManifest } from "@finisher/fin-node";
import type { ValidationReport } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import type { CredentialChange } from "../../auth/credentials.ts";
import type { DeviceEvent } from "../../auth/devices.ts";
import type { Audit } from "../../control/audit.ts";
import type { ControlStore } from "../../control/db.ts";
import { attemptJson, finError } from "../../errors.ts";
import type { JobEngine } from "../../jobs/api.ts";
import type { KindRegistry } from "../../jobs/kinds.ts";
import { pluginHookKind } from "../../jobs/kinds/plugin.hook.ts";
import { makePluginHooks } from "./hooks.ts";
import { RETIRE_GRACE_MS } from "../../jobs/warm-worker.ts";
import type { InstalledFull, PluginTables } from "../tables.ts";
import { makeCrashLoop } from "./crash-loop.ts";
import { PLUGIN_FAMILY, registerPluginFamily } from "./family.ts";
import { type Authority, type AuthorityGenerations, authorityOfWarmKey, classSlug, jobAuthority, raiseFromAuthData, removeClassAuthData, storedGenerations, sweepAuthData } from "./authority.ts";
import { pluginDataFor } from "./family.ts";
import { type CallDeps, makeHostCalls, PAGE_BYTES } from "./calls.ts";
import { bareKey, keyTmpDir, pluginWarmKey, principalClass, sessionRoots } from "./keys.ts";
import { type EgressTestHooks, makeEgress } from "./egress.ts";
import { makePluginLog, scrubLogText, type ServerLog } from "./log.ts";
import { type CommandRunEventLike, type CommandRunExtra, commandRunOf, journalPluginCommand } from "./command-journal.ts";
import { makePluginStats } from "./stats.ts";
import { splitVia, storeDirOf } from "./program.ts";
import type { JobRights, Permission, PluginContext } from "@finisher/core-types";
import { type Invoker, rightsFor } from "./rights.ts";

/**
 * The host API a code plugin must accept to be enabled (sp8b Decision 14): 1.2 consumes hooks
 * (7F Decision 20), in step with `fin_plugin_api::HOST_API`.
 */
export const PLUGIN_HOST_API = "1.2.0";

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
export const keyPlugin = bareKey;

export interface PluginHostDeps {
  /** Links resolved. */
  readonly serverData: string;
  readonly control: ControlStore;
  readonly tables: PluginTables;
  readonly audit?: Audit;
  readonly clock?: () => number;
  /** How long a bump's drain waits (tests); `AUTHORITY_DRAIN_MS` by default. */
  readonly drainMs?: number;
  /** The server log plugins' `log` calls and stderr go to (Task 7); stderr of the host otherwise. */
  readonly log?: ServerLog;
  /** The broker's test hooks (`egress-test-hooks.ts`; refused outside a test-hooks build). */
  readonly egressHooks?: EgressTestHooks;
  /** The diagnostics journal: `plugin.stats` and `plugin.command` (Task 15). */
  readonly journal?: Pick<Journal, "enabled" | "training" | "recordPlugin">;
}

/** 5C's envelope line (Decision 5). */
const MESSAGE_BYTES = 1 << 20;

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
  /** Cold processes' keys and plugins by job (their calls and stderr), until the job ends. */
  const cold = new Map<string, { readonly key: string; readonly plugin: string }>();
  let callDeps: CallDeps | undefined;
  const warmKeyOf = (job: JobView): string => pluginWarmKey(job, rootSessionOf, generations);
  const serverLog: ServerLog = d.log ?? ((level, message) => void process.stderr.write(`finisherd: ${level}: ${message}\n`));
  const stats = makePluginStats({
    ...(d.journal === undefined ? {} : { journal: d.journal }),
    egress: () => egress.stats(),
    log: (m) => serverLog("warning", m),
    now: clock,
  });
  const pluginLog = makePluginLog({
    write: serverLog,
    clock,
    bundled: (id) => d.tables.installedFull(id)?.source === "bundled",
    dropped: (plugin, what) => stats.dropped(plugin, what),
  });
  // Refusals go to the server log under the plugin's keyed id, scrubbed (never to the journal).
  const egressLog = (plugin: string, message: string): void => serverLog("warning", `[${pluginLog.name(plugin)}] ${scrubLogText(message)}`);
  const egress = makeEgress({ log: egressLog, ...(d.egressHooks === undefined ? {} : { hooks: d.egressHooks }) });
  const calls = makeHostCalls({ deps: () => callDeps, keyNow: warmKeyOf, coldKey: (job) => cold.get(job)?.key, log: pluginLog, egress, stats });
  const onStderr = (line: string, from: { readonly job?: string; readonly key?: string }): void => {
    const plugin = from.key !== undefined ? (keyPlugin(from.key).split("|")[0] ?? "") : (cold.get(from.job ?? "")?.plugin ?? "");
    pluginLog.stderr(from.key ?? `job:${from.job ?? ""}`, plugin, line);
  };

  /** A job's rights (Tasks 9, 10): `invoker` ∩ `grants` under its class's generation now. */
  const rightsOf = (invoker: Invoker, grants: ReadonlyArray<Permission>, sessionRoot?: string): JobRights => {
    const cls = principalClass({ kind: invoker.kind, id: invoker.id }, rootSessionOf);
    return rightsFor({ invoker, grants, class: cls, sessionRoot, generation: generations.of(cls) });
  };
  const pluginHooks = makePluginHooks({ serverData: d.serverData, tables: d.tables, rightsOf });

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
      const still = new Set(live().map((w) => bareKey(w.key)));
      for (const key of [...keys]) {
        if (!ofKey(key) || still.has(key)) continue;
        keys.delete(key);
        rmSync(keyTmpDir(d.serverData, key), { recursive: true, force: true });
      }
    });

  /** One drain per class at a time (a later bump joins it; `close` interrupts them all). */
  const drains = new Map<string, Fiber.Fiber<void>>();
  let closed = false;
  /** Interrupts every drain (their removals are left to the next start's sweep). */
  const close = (): Effect.Effect<void> =>
    Effect.suspend(() => {
      if (!closed) {
        stats.stop();
        stats.flush();
      }
      closed = true;
      const all = [...drains.values()];
      drains.clear();
      return Effect.asVoid(Fiber.interruptAll(all));
    });

  /**
   * Waits until no warm process of `cls` older than its generation now is left and no job of
   * the class submitted before it runs (at most `AUTHORITY_DRAIN_MS`), then removes those older
   * generations' temp directories and the class's authority-keyed `pluginData` below it. A
   * bump during the wait moves the target on, so the newest generation's state is never
   * touched. On a timeout nothing is removed: the next start's sweep does it (as `stop` does).
   */
  const drain = (cls: string): Effect.Effect<void> =>
    Effect.gen(function* () {
      const slug = classSlug(cls);
      const older = (key: string, target: number) => {
        const a = authorityOfWarmKey(key);
        return a !== undefined && a.slug === slug && a.generation < target;
      };
      const busy = (e: JobEngine, target: number) =>
        e.workers().warm.some((w) => w.family === PLUGIN_FAMILY && older(w.key, target)) ||
        e.list({ state: ["running"], limit: 500 }).jobs.some((j) => j.family === PLUGIN_FAMILY && principalClass(j.owner, rootSessionOf) === cls && (isJobRights(j.rights) ? j.rights.generation < target : true));
      const e = engine;
      let waited = 0;
      for (;;) {
        const target = generations.of(cls);
        if (e !== undefined && busy(e, target)) {
          if (waited >= (d.drainMs ?? AUTHORITY_DRAIN_MS)) return;
          yield* Effect.sleep(50);
          waited += 50;
          continue;
        }
        // The check and the removal in one synchronous step: no bump can come between them.
        const done = yield* Effect.sync(() => {
          if (generations.of(cls) !== target) return false;
          const still = new Set(e?.workers().warm.map((w) => bareKey(w.key)) ?? []);
          for (const key of [...keys]) {
            if (!older(key, target) || still.has(key)) continue;
            keys.delete(key);
            try {
              rmSync(keyTmpDir(d.serverData, key), { recursive: true, force: true });
            } catch {}
          }
          try {
            removeClassAuthData(d.serverData, cls, target);
          } catch {}
          return true;
        });
        if (done) return;
      }
    });

  /**
   * `cls`'s policy generation moves on (spec §3.3): its warm processes of older generations are
   * retired at once (a busy one after its job), then the class's drain (`drain`) removes their
   * temp and authority data. Resolves with the new generation once that drain ended.
   */
  const authorityChanged = (cls: string): Effect.Effect<number> =>
    Effect.gen(function* () {
      const generation = generations.bump(cls);
      const slug = classSlug(cls);
      engine?.retireWarm(PLUGIN_FAMILY, (key) => {
        const a = authorityOfWarmKey(key);
        return a !== undefined && a.slug === slug && a.generation < generation;
      });
      if (closed) return generation;
      let f = drains.get(cls);
      if (f === undefined) {
        // It yields first, so it is in `drains` before it can end (and remove itself).
        const started: Fiber.Fiber<void> = Effect.runFork(
          Effect.yieldNow.pipe(
            Effect.andThen(drain(cls)),
            Effect.ensuring(
              Effect.sync(() => {
                if (drains.get(cls) === started) drains.delete(cls);
              }),
            ),
          ),
        );
        drains.set(cls, started);
        f = started;
      }
      // An interrupted drain (`close`) is not this bump's failure.
      yield* Fiber.await(f);
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

  const host = {
    /**
     * Registers the `plugin` family and the `plugin.hook` kind (before the engine starts) and
     * empties the keys' temp (the authority data is swept at `bind`).
     */
    register(r: KindRegistry): void {
      registerPluginFamily(r, {
        serverData: d.serverData,
        installed: (id) => d.tables.installedFull(id),
        rootSessionOf,
        generations,
        onKey: (k, c) => {
          keys.add(k);
          if (c !== undefined) cold.set(c.job, { key: k, plugin: c.plugin });
        },
        onCall: calls.onCall,
        onStderr,
      });
      r.registerKind(pluginHookKind({ contextOf: (job, o) => host.contextOf(job, o), source: pluginHooks }));
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
        Effect.runFork(close());
      };
    },
    /** Stops the drains (`watch`'s unsubscribe does it too); call before the engine closes. */
    close,
    /** `cls`'s generation moves on now (`watch` calls it; tests too). */
    authorityChanged,
    /** The generations (tests; Task 7 compares a call's key with the job's live one). */
    generations,
    /** The authority `job` runs under now. */
    authorityOf: (job: JobView): Authority | undefined => jobAuthority(job, principalClass(job.owner, rootSessionOf), generations),
    /** `job`'s warm key now: a worker whose key differs serves a retired authority (Task 7). */
    warmKeyOf,
    /**
     * A command job's rights (Task 9): `invoker` ∩ `grants` under its class's generation now;
     * `sessionRoot` is an agent session's root branch.
     */
    rightsOf,
    /** The plugins' hook handlers (7F Task 10): the hook engine's plugin layer. */
    pluginHooks,
    /** Serves host calls with `deps` (once the server's credentials, sessions and projects exist). */
    serve(deps: CallDeps): void {
      callDeps = deps;
    },
    /** The host calls (tests). */
    calls,
    /** The egress broker: its per-plugin counters feed `plugin.stats` (Task 15). */
    egress,
    /** `plugin.stats`' counters (tests flush them). */
    stats,
    /**
     * Task 9's `onRun` hook (once per `plugin.command.run`, every outcome): records
     * `plugin.command` and counts the run in `plugin.stats`' `command` family; never throws.
     * The event carries the validated `args`, the `txId` and the op counts; `extra` overrides
     * them. A name that did not resolve (`PLUGIN_NOT_FOUND`) is journaled but not counted: no
     * plugin ran, and a mistyped id must not take a slot of the stats window.
     */
    onCommandRun(e: CommandRunEventLike, extra?: CommandRunExtra): void {
      const run = commandRunOf(e, extra);
      journalPluginCommand(d.journal, run, (m) => serverLog("warning", m));
      if (run.code !== "PLUGIN_NOT_FOUND") stats.call(run.plugin, "command", run.durationMs * 1000, run.code, 0, 0);
    },
    /**
     * `start.extra`'s `PluginContext` for `job` (Task 9's kind puts it in): the plugin, its
     * grants, its package, the caller's `pluginData` (with `fs pluginData`) and the key's temp.
     */
    contextOf(job: JobView, opts: { readonly timeoutS: number; readonly locale?: string }): PluginContext {
      const { id, version } = splitVia(job.via?.plugin ?? "");
      const row = d.tables.installedFull(id);
      if (row === undefined || row.version !== version) throw finError("PLUGIN_NOT_FOUND", `${id}@${version} is not installed`, { plugin: id, version });
      const data = pluginDataFor({ serverData: d.serverData, rootSessionOf, generations }, job);
      return {
        hostApi: PLUGIN_HOST_API,
        plugin: { id, version },
        grants: isJobRights(job.rights) ? (job.rights.grants as PluginContext["grants"]) : [],
        dirs: { plugin: storeDirOf(d.serverData, row), ...(data === undefined ? {} : { data: data.path }), tmp: keyTmpDir(d.serverData, warmKeyOf(job)) },
        locale: opts.locale ?? "en",
        limits: { timeoutS: opts.timeoutS, messageBytes: MESSAGE_BYTES, pageBytes: PAGE_BYTES },
      };
    },
    /** `PluginContext.dirs.data` of `job` (Task 7), created on first use; only with `fs pluginData`. */
    dataDirOf: (job: JobView) => pluginDataFor({ serverData: d.serverData, rootSessionOf, generations }, job),
    /**
     * Binds the started engine: stops go to it, its job ends feed the crash loop; the classes'
     * generations are raised to their queued jobs' and authority directories', then the
     * authority data of other generations is swept.
     */
    bind(e: JobEngine): void {
      engine = e;
      pluginHooks.bind(e);
      e.onDone((job) => {
        calls.release(job.id);
        cold.delete(job.id);
        crashLoop.onDone(job);
        if (job.family === PLUGIN_FAMILY && job.via !== undefined) stats.jobEnded(job.via.plugin, job.error?.code);
      });
      if (!closed) stats.start();
      // A generation bump the store could not record must not be handed out again after a
      // restart: a job still queued under it raises its class back to it.
      for (let cursor: string | undefined, page = 0; page < 100; page++) {
        const l = e.list({ state: ["queued", "paused", "running"], limit: 500, ...(cursor === undefined ? {} : { cursor }) });
        for (const j of l.jobs) if (j.family === PLUGIN_FAMILY && isJobRights(j.rights)) generations.raise(principalClass(j.owner, rootSessionOf), j.rights.generation);
        if (l.next === undefined) break;
        cursor = l.next;
      }
      // Then the authority directories (review of Task 7): their own generations raise their
      // classes too, and only then does the sweep remove what is not current.
      raiseFromAuthData(d.serverData, generations);
      sweepAuthData(d.serverData, generations);
    },
    hooks: { stop, enabled: (id: string) => crashLoop.clear(id), confined: () => engine?.workers().caps.profiles.plugin === "enforced", enableCheck } satisfies PluginHostHooks,
  };
  return host;
}

export type PluginHost = ReturnType<typeof makePluginHost>;
