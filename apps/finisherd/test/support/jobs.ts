// A job engine over a temp server data and the test-hooks `fin-workers` (5C-1 plan Task 9a).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import { Effect, Exit, Fiber, Layer, Stream } from "effect";
import { type ControlStore, ControlStoreTag, openControlStore } from "../../src/control/db.ts";
import type { JobEngine, JobEvent } from "../../src/jobs/engine.ts";
import { JobEventSinkTag, type MemorySink, memorySink } from "../../src/jobs/events.ts";
import { buildJobEngine, type JobEngineOptions } from "../../src/jobs/layer.ts";
import { JobStore } from "../../src/jobs/store.ts";

export const OWNER = { kind: "local", id: "local" } as const;
export const WAIT_MS = 20_000;

export interface TestEngine {
  readonly engine: JobEngine;
  readonly sink: MemorySink;
  readonly control: ControlStore;
  /** A second store over the same database, for reading rows as the engine wrote them. */
  readonly jobs: JobStore;
  readonly serverData: string;
  /** Every event the engine published since it started. */
  readonly events: JobEvent[];
  submit(kind: string, params: unknown, spec?: Record<string, unknown>): Promise<JobView>;
  /** Submits and waits for the job's end. */
  runJob(kind: string, params: unknown, spec?: Record<string, unknown>): Promise<JobView>;
  wait(id: string, ms?: number): Promise<JobView>;
  close(): Promise<void>;
}

const open: TestEngine[] = [];

/** Closes every engine `testEngine` opened (call from `afterEach`). */
export async function closeTestEngines(): Promise<void> {
  for (const t of open.splice(0)) await t.close();
}

export const run = <A, E>(e: Effect.Effect<A, E>): Promise<A> => Effect.runPromise(e);

/** The failure of an Effect, which must fail. */
export async function failure<A>(e: Effect.Effect<A, FinError>): Promise<FinError> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit.value)}`);
  const reason = exit.cause.reasons.find((r) => r._tag === "Fail");
  if (reason === undefined || reason._tag !== "Fail") throw new Error("expected a FinError failure");
  return reason.error;
}

/**
 * An engine with the test kinds, cgroups off and short timers. `serverData` reuses a
 * directory (a second engine over the same data); otherwise a new temp one is made and removed.
 */
export async function testEngine(opts: JobEngineOptions & { serverData?: string; keepData?: boolean } = {}): Promise<TestEngine> {
  const own = opts.serverData === undefined;
  const serverData = opts.serverData ?? mkdtempSync(join(tmpdir(), "fin-jobs-"));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const sink = memorySink();
  const { serverData: _s, keepData: _k, ...engineOpts } = opts;
  const layer = Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, sink));
  const engine = await Effect.runPromise(
    buildJobEngine(serverData, {
      testKinds: true,
      cgroups: "off",
      cores: 4,
      ...engineOpts,
      timing: { killWaitMs: 2000, exitDrainMs: 100, ...(engineOpts.timing ?? {}) },
    }).pipe(Effect.provide(layer)),
  );
  const events: JobEvent[] = [];
  const fiber = Effect.runFork(Stream.runForEach(engine.events(), (e) => Effect.sync(() => events.push(e))));
  // The subscription is made when the stream starts: let it start before any submit.
  await new Promise((r) => setTimeout(r, 20));
  let closed = false;
  const t: TestEngine = {
    engine,
    sink,
    control,
    jobs: new JobStore(control),
    serverData,
    events,
    submit: (kind, params, spec = {}) => run(engine.submit(OWNER, { kind, params, ...spec })),
    async runJob(kind, params, spec = {}) {
      const v = await t.submit(kind, params, spec);
      return t.wait(v.id);
    },
    async wait(id, ms = WAIT_MS) {
      const [v] = await run(engine.wait([id], { timeoutMs: ms }));
      return v as JobView;
    },
    async close() {
      if (closed) return;
      closed = true;
      await run(engine.close());
      await Effect.runPromise(Fiber.interrupt(fiber));
      await control.close();
      if (own && opts.keepData !== true) rmSync(serverData, { recursive: true, force: true });
    },
  };
  open.push(t);
  return t;
}

/** Polls `f` until it is truthy (≤ `ms`). */
export async function until<A>(f: () => A | undefined | false, ms = WAIT_MS): Promise<A> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() > end) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 10));
  }
}
