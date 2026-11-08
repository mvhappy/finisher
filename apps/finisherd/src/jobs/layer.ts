// The job engine as a layer (5C-1 plan Task 9a): `<server-data>/jobs/` made private, fin-node's
// job host started, the store and the registry (the test kinds with `testKinds`), the engine
// built and started; released by `engine.close()`, before the control store closes.
import { availableParallelism, totalmem } from "node:os";
import { type JobHostOptions, testHooks, workersBinaryPath } from "@finisher/fin-node";
import { Clock, Context, Effect, Layer } from "effect";
import { dirname } from "node:path";
import { type ServerStartError, startFailure } from "../config.ts";
import { ControlStoreTag } from "../control/db.ts";
import { type JobEngine, JobEngineTag, makeJobEngine, QUEUE_LIMIT } from "./engine.ts";
import { JobEventSinkTag } from "./events.ts";
import { ensureJobsRoot } from "./files.ts";
import { EngineHost } from "./host.ts";
import { type FamilySpec, type KindSpec, makeKindRegistry } from "./kinds.ts";
import { registerTestKinds } from "./kinds/test.ts";
import { defaultPools } from "./pools.ts";
import { JobStore } from "./store.ts";

export { JobEngineTag } from "./engine.ts";

export interface JobEngineOptions {
  /** Where `fin-workers` lives (default: beside the loaded fin-node binary). */
  readonly binaryDir?: string;
  readonly timing?: JobHostOptions["timing"];
  /**
   * Registers the `test`, `testbad` and `testplugin` families. `overrides` replaces fields of
   * test kinds by name (a short `stallS`, a hook), for the engine's own tests.
   */
  readonly testKinds?:
    | boolean
    | { readonly overrides?: Readonly<Record<string, Partial<KindSpec>>>; readonly families?: Readonly<Record<string, Partial<FamilySpec>>> };
  readonly cores?: number;
  readonly totalMem?: number;
  readonly cgroups?: "auto" | "off";
  /** The Unix watchdog (default on). */
  readonly watchdog?: boolean;
  /** Default: the Clock of the layer's context (a `TestClock` in tests that provide one). */
  readonly clock?: Clock.Clock;
  /** Whether `start.diagnostics` is on (Decision 7; D4 reads the privacy setting). Default off. */
  readonly diagnostics?: () => boolean;
  /** The unfinished-job limit (50,000; tests lower it). */
  readonly queueLimit?: number;
}

/** The engine over `serverData`, its host started; the caller closes it. */
export const buildJobEngine = (serverData: string, opts: JobEngineOptions = {}): Effect.Effect<JobEngine, ServerStartError, ControlStoreTag | JobEventSinkTag> =>
  Effect.gen(function* () {
    const control = yield* ControlStoreTag;
    const sink = yield* JobEventSinkTag;
    const clock = opts.clock ?? (yield* Clock.Clock);
    const context = Context.add(yield* Effect.context<never>(), Clock.Clock, clock);
    const run = Effect.runForkWith(context);
    const now = (): number => clock.currentTimeMillisUnsafe();
    const log = (level: "debug" | "warning" | "error", message: string): void => {
      run(level === "debug" ? Effect.logDebug(message) : level === "warning" ? Effect.logWarning(message) : Effect.logError(message));
    };
    const binaryDir = opts.binaryDir ?? dirname(workersBinaryPath());
    const registry = makeKindRegistry();
    if (opts.testKinds !== undefined && opts.testKinds !== false) {
      const t = opts.testKinds === true ? {} : opts.testKinds;
      registerTestKinds(registry, { binaryDir, overrides: t.overrides ?? {}, families: t.families ?? {} });
    }
    const root = yield* Effect.try({ try: () => ensureJobsRoot(serverData), catch: startFailure });
    const host = yield* Effect.tryPromise({
      try: () =>
        EngineHost.start(
          {
            binaryDir,
            ...(opts.timing === undefined ? {} : { timing: opts.timing }),
            ...(opts.cgroups === undefined ? {} : { cgroups: opts.cgroups }),
            ...(opts.watchdog === undefined ? {} : { watchdog: opts.watchdog }),
          },
          now,
          log,
        ),
      catch: startFailure,
    });
    const caps = host.caps();
    if (!caps.sandboxed) log("warning", `job engine: workers run unsandboxed (spawner ${caps.sandbox}); kill tree ${caps.killTree}`);
    return yield* Effect.tryPromise({
      try: () =>
        makeJobEngine({
          host,
          store: new JobStore(control, now),
          registry,
          pools: defaultPools(opts.cores ?? availableParallelism()),
          sink,
          root,
          totalMem: opts.totalMem ?? totalmem(),
          now,
          run,
          log,
          diagnostics: opts.diagnostics ?? (() => false),
          testHooks: testHooks !== undefined,
          queueLimit: opts.queueLimit ?? QUEUE_LIMIT,
        }),
      catch: (e) => {
        void host.shutdown(0);
        return startFailure(e);
      },
    });
  });

/** The engine for the enclosing scope: built and started at build, closed at release. */
export const layerJobEngine = (serverData: string, opts: JobEngineOptions = {}): Layer.Layer<JobEngineTag, ServerStartError, ControlStoreTag | JobEventSinkTag> =>
  Layer.effect(JobEngineTag, Effect.acquireRelease(buildJobEngine(serverData, opts), (engine) => engine.close()));
