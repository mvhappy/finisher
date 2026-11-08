// The job engine (spec §6.0–6.5, 5C-1 plan Tasks 9a, 9b): submit (Decision 27), the dispatch
// loop over the pools and the scheduler, the transitions of a cold job's attempts, the hook
// phase, `job.done` and the bus. Cancel, pause, resume and abandonment are in `control.ts`;
// start-up recovery, retention and close in `restart.ts`. Until later tasks: a `warm` kind runs
// cold and `warm(family)` does nothing (Task 11); a failure ends the job unless the worker
// asked for a retry (Task 10).
import { type FinError, type FinErrorData, type JobOwner, type JobProgress, type JobState, type JobView, type JobVia, type Priority } from "@finisher/contracts";
import type { JobHostCaps } from "@finisher/fin-node";
import { Context, Effect, type Fiber, PubSub, Stream } from "effect";
import { finError, toFinError } from "../errors.ts";
import type { RunFork } from "./calls.ts";
import { Abandonment, cancelJob, pauseJobs, pausedAfterRun, resumeJobs } from "./control.ts";
import type { JobEventSink } from "./events.ts";
import { removeJobDir, jobDirs } from "./files.ts";
import type { EngineHost, ProcView } from "./host.ts";
import { type KindRegistry, type KindSpec, type PoolName, resolveOrFn } from "./kinds.ts";
import { type PoolView, type Pools } from "./pools.ts";
import { closeEngine, crashEngine, interruptRow, recover, runParams } from "./restart.ts";
import { submitJob, waitJobs } from "./submit.ts";
import { type AttemptOutcome, ALIVE_MS, RunControl, type RunnerEnv, runAttempt, runOnSucceeded } from "./runner.ts";
import { QueueIndex, type QueuedJob, schedule } from "./schedule.ts";
import type { JobQuery, JobRow, JobStore, TransitionPatch } from "./store.ts";
import type { Timers } from "./timers.ts";
import { type JobEvent, type RunStats, doneEvent, newStats, withProgress } from "./view.ts";

export type { JobEvent } from "./view.ts";

export const QUEUE_LIMIT = 50_000;
export const MAX_ATTEMPTS_LIMIT = 10;
const PROGRESS_EVERY_MS = 250;

export interface SubmitSpec {
  readonly kind: string;
  readonly params: unknown;
  readonly priority?: Priority;
  readonly idemKey?: string;
  readonly project?: string;
  readonly library?: string;
  readonly parent?: string;
  readonly via?: JobVia;
  readonly rights?: unknown;
  readonly maxAttempts?: number;
}

export interface WorkersView {
  readonly pools: ReadonlyArray<PoolView>;
  /** Warm workers (Task 11). */
  readonly warm: ReadonlyArray<never>;
  readonly processes: ReadonlyArray<ProcView>;
  readonly stuck: ReadonlyArray<string>;
  readonly caps: JobHostCaps;
  /** `diag` messages dropped (diagnostics off, or a kind the family does not forward). */
  readonly diagDropped: number;
}

export interface JobEngine {
  readonly kinds: KindRegistry;
  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError>;
  get(id: string): Effect.Effect<JobView, FinError>;
  list(q: JobQuery): { readonly jobs: JobView[]; readonly next?: string };
  wait(ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError>;
  cancel(id: string): Effect.Effect<JobView, FinError>;
  pause(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  resume(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  warm(family: string): Effect.Effect<void, FinError>;
  definePool(name: PoolName, size: number): void;
  workers(): WorkersView;
  events(): Stream.Stream<JobEvent>;
  onDone(listener: (job: JobView) => void): () => void;
  close(): Effect.Effect<void>;
}

export class JobEngineTag extends Context.Service<JobEngineTag, JobEngine>()("finisherd/JobEngine") {}

/** What `makeJobEngine` is built from (`layer.ts` assembles it). */
export interface EngineDeps {
  readonly host: EngineHost;
  readonly store: JobStore;
  readonly registry: KindRegistry;
  readonly pools: Pools;
  readonly sink: JobEventSink;
  readonly root: string;
  readonly totalMem: number;
  readonly now: () => number;
  readonly run: RunFork;
  readonly timers: Timers;
  readonly log: RunnerEnv["log"];
  readonly diagnostics: () => boolean;
  readonly testHooks: boolean;
  readonly queueLimit: number;
}

export const finished = (v: JobView): boolean => v.finishedAt !== undefined;

/** A `FinError`'s stored form. */
export function data(e: FinError): FinErrorData {
  return e.fix === undefined ? { code: e.code, message: e.message, details: e.details } : { code: e.code, message: e.message, details: e.details, fix: e.fix };
}

const promise = <A>(f: () => Promise<A>): Effect.Effect<A, FinError> => Effect.tryPromise({ try: f, catch: toFinError });

/**
 * The engine. Its members without `#` are internal: `control.ts` and `restart.ts` use them;
 * callers see only `JobEngine`.
 */
export class Engine implements JobEngine {
  readonly kinds: KindRegistry;
  readonly d: EngineDeps;
  readonly env: RunnerEnv;
  readonly queue = new QueueIndex();
  /** The running attempts' controls (cancel, pause, close). */
  readonly runs = new Map<string, RunControl>();
  /** Runs and hook re-runs in flight. */
  readonly active = new Set<Promise<void>>();
  readonly resources = new Map<string, ReadonlyArray<string>>();
  readonly abandonment: Abandonment;
  readonly #stats = new Map<string, RunStats>();
  readonly #progress = new Map<string, { p: JobProgress; last: number; cancel?: (() => void) | undefined }>();
  readonly #listeners = new Set<(job: JobView) => void>();
  /** Called after every end and every run's end (`wait`, `settled`). */
  readonly waiters = new Set<() => void>();
  readonly #bus: PubSub.PubSub<JobEvent>;
  #wake: { at: number; cancel: () => void } | undefined;
  /** `pause {all}`: nothing is dispatched until `resume {all}`. */
  pausedAll = false;
  closing = false;
  /** A simulated crash (tests): nothing more is written. */
  crashed = false;
  stopRetention: (() => void) | undefined;
  /** Unfinished jobs, kept in memory (9a review L3). */
  unfinished = 0;
  #diagDropped = 0;

  constructor(d: EngineDeps) {
    this.d = d;
    this.kinds = d.registry;
    this.#bus = Effect.runSync(PubSub.sliding<JobEvent>(4096));
    this.abandonment = new Abandonment(d.pools, d.log, () => this.pump());
    d.host.onAbandonedExit = (proc) => this.abandonment.exited(proc);
    this.env = {
      host: d.host,
      store: d.store,
      registry: d.registry,
      sink: d.sink,
      root: d.root,
      totalMem: d.totalMem,
      now: d.now,
      run: d.run,
      timers: d.timers,
      diagnostics: d.diagnostics,
      testHooks: d.testHooks,
      aliveMs: ALIVE_MS,
      log: d.log,
      hooks: new Set<Fiber.Fiber<unknown, unknown>>(),
      dead: () => this.crashed,
      progress: (job, p) => this.#onProgress(job, p),
      item: (job, item, outcome) => {
        const view = d.store.get(job);
        if (view !== undefined) this.publish({ type: "item", job: view, item, ...("error" in outcome ? { error: { code: outcome.error.code } } : {}) });
      },
      diagDropped: () => {
        this.#diagDropped++;
      },
      abandoned: (proc, job) => this.abandonment.abandoned(proc, job, this.resources.get(job) ?? []),
    };
  }

  /** Start-up (Decision 28): recovery, then the queue, then dispatch. */
  async start(): Promise<void> {
    await recover(this);
    this.unfinished = this.d.store.unfinishedCount();
    this.pump();
  }

  publish(e: JobEvent): void {
    if (!this.crashed) Effect.runSync(PubSub.publish(this.#bus, e));
  }

  shutdownBus(): void {
    Effect.runSync(PubSub.shutdown(this.#bus));
  }

  view(id: string): JobView | undefined {
    const v = this.d.store.get(id);
    return v === undefined ? undefined : withProgress(v, this.#progress.get(id)?.p);
  }

  /** A transition, keeping the in-memory count of unfinished jobs (9a review L3). */
  transition(id: string, to: JobState, patch: TransitionPatch = {}): JobView {
    const before = this.d.store.row(id);
    const view = this.d.store.transition(id, to, patch);
    if (before !== undefined) this.unfinished += (finished(view) ? 0 : 1) - (finished(before) ? 0 : 1);
    return view;
  }

  #onProgress(job: string, p: JobProgress): void {
    const held = this.#progress.get(job) ?? { p, last: -Infinity };
    held.p = p;
    this.#progress.set(job, held);
    const emit = (): void => {
      held.cancel = undefined;
      held.last = this.d.now();
      const view = this.view(job);
      if (view !== undefined && !finished(view)) this.publish({ type: "progress", job: view });
    };
    const wait = held.last + PROGRESS_EVERY_MS - this.d.now();
    if (wait <= 0 && held.cancel === undefined) emit();
    else held.cancel ??= this.d.timers.after(Math.max(0, wait), emit);
  }

  dropProgress(job: string): void {
    this.#progress.get(job)?.cancel?.();
    this.#progress.delete(job);
  }

  dropAllProgress(): void {
    for (const id of [...this.#progress.keys()]) this.dropProgress(id);
  }

  enqueue(view: JobView, kind: KindSpec): void {
    const resources = resolveOrFn(kind.resources, view.params as never);
    this.resources.set(view.id, resources);
    const q: QueuedJob = {
      id: view.id,
      priority: view.priority,
      createdAt: view.createdAt,
      resources,
      ...(view.nextRunAt === undefined ? {} : { nextRunAt: view.nextRunAt }),
    };
    this.queue.add(q);
    if (q.nextRunAt !== undefined) this.#wakeAt(q.nextRunAt);
  }

  #wakeAt(at: number): void {
    if (this.#wake !== undefined && this.#wake.at <= at) return;
    this.#wake?.cancel();
    const cancel = this.d.timers.after(at - this.d.now(), () => {
      this.#wake = undefined;
      this.pump();
    });
    this.#wake = { at, cancel };
  }

  cancelWake(): void {
    this.#wake?.cancel();
    this.#wake = undefined;
  }

  /** Dispatches what can run now (synchronously: a submit's job starts in its own turn). */
  pump(): void {
    if (this.closing || this.pausedAll) return;
    for (const d of schedule({ queue: this.queue, pools: this.d.pools, warm: new Map() }, this.d.now())) {
      if (d.mode === "cold") this.#dispatch(d.job);
    }
  }

  #dispatch(id: string): void {
    const resources = this.resources.get(id) ?? [];
    this.queue.remove(id);
    const row = this.d.store.row(id);
    if (row === undefined || row.state !== "queued" || !this.d.pools.take(resources)) {
      this.d.log("error", `job ${id}: dispatched but not runnable (${row?.state ?? "gone"})`);
      return;
    }
    const view = this.transition(id, "running", { attempts: row.attempts + 1, nextRunAt: null });
    this.publish({ type: "state", job: view });
    const control = new RunControl();
    this.runs.set(id, control);
    this.track(this.#run(id, resources, control));
  }

  /** Keeps `p` among the runs `close` waits for. */
  track(p: Promise<void>): void {
    const run = p.catch((e) => this.d.log("error", `job engine: a run failed: ${(e as Error).stack ?? String(e)}`));
    this.active.add(run);
    void run.finally(() => this.active.delete(run));
  }

  /** One attempt and what follows it. Whatever throws, the slots go back and the job ends (9a review H3). */
  async #run(id: string, resources: ReadonlyArray<string>, control: RunControl): Promise<void> {
    let given = false;
    const give = (): void => {
      if (given) return;
      given = true;
      this.d.pools.give(resources);
    };
    try {
      const row = this.d.store.row(id);
      const kind = row === undefined ? undefined : this.kinds.kind(row.kind);
      const family = kind === undefined ? undefined : this.kinds.family(kind.family);
      if (row === undefined || kind === undefined || family === undefined) {
        give();
        if (row !== undefined) await this.end(id, "failed", { error: data(finError("JOB_KIND_UNKNOWN", `no kind ${row.kind}`, { kind: row.kind })) });
        return;
      }
      const outcome = await runAttempt(this.env, row, kind, family, runParams(this.d.store, row, kind), control);
      const stats = this.#statsOf(id);
      stats.runMicros += outcome.runMicros;
      stats.helpers += outcome.helpers;
      give();
      if (this.crashed) return;
      await this.#afterAttempt(id, row, kind, outcome, control);
    } catch (e) {
      give();
      if (this.crashed) return;
      this.d.log("error", `job ${id}: the runner failed: ${(e as Error).stack ?? String(e)}`);
      const row = this.d.store.row(id);
      if (row !== undefined && !finished(row)) {
        await this.end(id, "failed", { error: data(finError("INTERNAL", `the job's runner failed: ${toFinError(e).message}`)) });
      }
    } finally {
      this.runs.delete(id);
      this.wakeWaiters();
    }
  }

  async #afterAttempt(id: string, row: JobRow, kind: KindSpec, outcome: AttemptOutcome, control: RunControl): Promise<void> {
    if (outcome.ok) return this.#succeed(id, kind, outcome.value);
    switch (control.requested) {
      case "cancel":
        await this.end(id, "cancelled", { error: data(finError("JOB_CANCELLED", "the job was cancelled")) });
        return;
      case "pause":
        pausedAfterRun(this, id);
        return;
      case "close":
        await interruptRow(this, this.d.store.row(id) ?? row, kind);
        return;
    }
    if (this.#retryable(row.attempts, row.maxAttempts, outcome)) {
      const view = this.transition(id, "queued", { error: data(outcome.error), nextRunAt: null });
      this.dropProgress(id);
      this.publish({ type: "state", job: view });
      this.enqueue(view, kind);
      this.pump();
      return;
    }
    await this.end(id, "failed", { error: data(outcome.error) });
  }

  /** Task 9a's minimal policy (Task 10 replaces it): only a worker's own retry hint retries, at once. */
  #retryable(attempts: number, maxAttempts: number, o: AttemptOutcome): boolean {
    return !o.ok && o.cause === "worker" && o.retry !== undefined && o.retry !== "never" && attempts < maxAttempts;
  }

  #statsOf(id: string): RunStats {
    let s = this.#stats.get(id);
    if (s === undefined) {
      s = newStats();
      this.#stats.set(id, s);
    }
    return s;
  }

  /** The worker succeeded: result and `phase: hooks` persisted first, then `onSucceeded` (Decision 40). */
  async #succeed(id: string, kind: KindSpec, value: unknown): Promise<void> {
    this.publish({ type: "state", job: this.d.store.enterHooks(id, value) });
    const row = this.d.store.row(id);
    if (row !== undefined) await this.runHooks(row, kind, value, false);
  }

  /** `onSucceeded`, then the job's end (also a restart's re-run of a `phase: hooks` row, `rerun`). */
  async runHooks(row: JobRow, kind: KindSpec, value: unknown, rerun: boolean): Promise<void> {
    const out = await runOnSucceeded(this.env, row, kind, value, rerun);
    if (this.crashed) return;
    if (out.ok) await this.end(row.id, "succeeded", { result: out.result, error: null });
    else await this.end(row.id, "failed", { error: data(out.error) });
  }

  /**
   * The terminal transition, marked `announce`; then at once `job.done`, the mark cleared, the
   * bus, listeners and waiters (9a review H2); then the directory deleted (a leftover is swept
   * at the next start).
   */
  async end(id: string, to: JobState, patch: TransitionPatch): Promise<void> {
    const view = this.transition(id, to, { ...patch, phase: "announce" });
    this.dropProgress(id);
    this.resources.delete(id);
    const stats = this.#stats.get(id) ?? newStats();
    this.#stats.delete(id);
    this.announce(view, stats);
    try {
      await removeJobDir(jobDirs(this.d.root, id).job);
    } catch (e) {
      this.d.log("warning", `job ${id}: its directory could not be deleted: ${toFinError(e).message}`);
    }
  }

  /** `job.done` for a terminal row (once; a restart re-announces one whose mark is still set). */
  announce(view: JobView, stats: RunStats = newStats()): void {
    try {
      this.d.sink.jobDone(doneEvent(view, this.kinds.kind(view.kind), stats, (m) => this.d.log("warning", m)));
    } catch (e) {
      this.d.log("error", `job ${view.id}: job.done failed: ${toFinError(e).message}`);
    }
    if (!this.crashed) this.d.store.announced(view.id);
    this.publish({ type: "state", job: view });
    for (const l of [...this.#listeners]) {
      try {
        l(view);
      } catch (e) {
        this.d.log("error", `a job onDone listener threw: ${(e as Error).stack ?? String(e)}`);
      }
    }
    this.wakeWaiters();
    this.pump();
  }

  /** Resolves once the job is finished or no run of it is in flight (it paused or was re-queued). */
  settled(id: string): Promise<void> {
    return new Promise((resolve) => {
      const on = (): void => {
        const v = this.d.store.get(id);
        if (v !== undefined && !finished(v) && this.runs.has(id)) return;
        this.waiters.delete(on);
        resolve();
      };
      this.waiters.add(on);
      on();
    });
  }

  /** Wakes `settled` and `wait` callers. */
  wakeWaiters(): void {
    for (const w of [...this.waiters]) w();
  }

  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError> {
    return Effect.try({ try: () => submitJob(this, owner, spec), catch: toFinError });
  }

  get(id: string): Effect.Effect<JobView, FinError> {
    return Effect.suspend(() => {
      const v = this.view(id);
      return v === undefined ? Effect.fail(finError("JOB_NOT_FOUND", `no job ${id}`, { job: id })) : Effect.succeed(v);
    });
  }

  list(q: JobQuery): { readonly jobs: JobView[]; readonly next?: string } {
    const page = this.d.store.list(q);
    const jobs = page.jobs.map((j) => withProgress(j, this.#progress.get(j.id)?.p));
    return page.next === undefined ? { jobs } : { jobs, next: page.next };
  }

  wait(ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError> {
    return waitJobs(this, ids, opts);
  }

  cancel(id: string): Effect.Effect<JobView, FinError> {
    return promise(() => cancelJob(this, id));
  }

  pause(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError> {
    return promise(() => pauseJobs(this, target));
  }

  resume(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError> {
    return Effect.try({ try: () => resumeJobs(this, target), catch: toFinError });
  }

  warm(): Effect.Effect<void, FinError> {
    return Effect.void;
  }

  definePool(name: PoolName, size: number): void {
    this.d.pools.define(name, size);
    this.pump();
  }

  workers(): WorkersView {
    const pools = this.d.pools.snapshot();
    return {
      pools,
      warm: [],
      processes: this.d.host.processes(),
      stuck: pools.filter((p) => p.stuck).map((p) => p.name),
      caps: this.d.host.caps(),
      diagDropped: this.#diagDropped,
    };
  }

  events(): Stream.Stream<JobEvent> {
    return Stream.fromPubSub(this.#bus);
  }

  onDone(listener: (job: JobView) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Graceful close (Decision 29): no `running` row in `phase: run` survives it. */
  close(): Effect.Effect<void> {
    return Effect.promise(() => closeEngine(this));
  }
}

/** Builds the engine and runs its start-up (recovery, queue). */
export async function makeJobEngine(deps: EngineDeps): Promise<JobEngine> {
  const engine = new Engine(deps);
  await engine.start();
  return engine;
}

/**
 * Tests only: stops the engine as if `finisherd` died now. Nothing more is written (rows stay
 * as they are, held items and checkpoints are lost), hooks are interrupted, the host is shut
 * down at once. A second engine over the same server data then runs Decision 28's recovery.
 */
export function crashJobEngineForTest(engine: JobEngine): Promise<void> {
  if (!(engine instanceof Engine)) throw new Error("not a job engine");
  return crashEngine(engine);
}
