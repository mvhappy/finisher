// The job engine (spec §6.0–6.5, 5C-1 plan Tasks 9a, 9b): submit (Decision 27), the dispatch
// loop over the pools and the scheduler, the transitions of a cold job's attempts, the hook
// phase, `job.done` and the bus. Its public types are in `api.ts`; cancel, pause, resume,
// abandonment and the hold of a job with an abandoned process in `control.ts`; start-up
// recovery, retention and close in `restart.ts`. Until later tasks: a `warm` kind runs cold and
// `warm(family)` does nothing (Task 11). The retry policy and isolation (Decisions 31, 32) are
// in `retry.ts` over the pure `isolation.ts`.
import type { FinError, FinErrorData, JobOwner, JobProgress, JobState, JobView } from "@finisher/contracts";
import { Effect, type Fiber, PubSub, Stream } from "effect";
import { finError, toFinError } from "../errors.ts";
import { Abandonment, cancelJob, pauseJobs, pausedAfterRun, resumeJobs } from "./control.ts";
import { removeJobDir, jobDirs } from "./files.ts";
import { type KindRegistry, type KindSpec, type PoolName, resolveOrFn } from "./kinds.ts";
import { closeEngine, crashEngine, interruptRow, recover } from "./restart.ts";
import { afterFailure, afterSuccess, completeIsolation, runPlan } from "./retry.ts";
import { submitJob, waitJobs } from "./submit.ts";
import { runOnSucceeded } from "./hooks.ts";
import { RunControl } from "./run-control.ts";
import { type AttemptOutcome, ALIVE_MS, type RunnerEnv, runAttempt } from "./runner.ts";
import { QueueIndex, type QueuedJob, schedule } from "./schedule.ts";
import { type JobQuery, type JobRow, type JobStore, type TransitionPatch, viewOf } from "./store.ts";
import { type JobEvent, type RunStats, doneEvent, newStats, withProgress } from "./view.ts";

export type { JobEvent } from "./view.ts";
export { type EngineDeps, type JobEngine, JobEngineTag, type SubmitSpec, type WorkersView } from "./api.ts";
import type { EngineDeps, JobEngine, SubmitSpec, WorkersView } from "./api.ts";

export const QUEUE_LIMIT = 50_000;
/** How long a job waits queued for an abandoned process of its last attempt to exit (9b review H1). */
export const ABANDONED_HOLD_MS = 10 * 60_000;
/** How long after start `onDone` listeners and `events()` subscribers get the start's re-announced jobs (9b review M5). */
export const REPLAY_WINDOW_MS = 30_000;
export const MAX_ATTEMPTS_LIMIT = 10;
const PROGRESS_EVERY_MS = 250;

export const finished = (v: JobView): boolean => v.finishedAt !== undefined;

/** A `FinError`'s stored form. */
export function data(e: FinError): FinErrorData {
  return e.fix === undefined ? { code: e.code, message: e.message, details: e.details } : { code: e.code, message: e.message, details: e.details, fix: e.fix };
}

/**
 * Whether a run stopped by a pause or close ended because of the stop: stopped before the
 * worker started, or after the stop reached it without its own answer (an exit, a helper
 * killed with the group, or the worker's `JOB_CANCELLED`).
 */
export function stoppedRun(o: AttemptOutcome): boolean {
  if (o.ok) return false;
  if (o.cause === "stopped") return true;
  return o.stopSent && (o.cause === "exit" || o.cause === "helper" || (o.cause === "worker" && o.error.code === "JOB_CANCELLED"));
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
  /** Queued jobs held back while a process of their last attempt is abandoned and alive (9b review H1). */
  readonly held = new Map<string, () => void>();
  /** Views re-announced at start, for listeners that attach just after it (9b review M5). */
  #replay: JobView[] | undefined = [];
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
  /** The first `close` (a second one waits for it). */
  closeDone: Promise<void> | undefined;
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
    this.abandonment = new Abandonment(d.pools, d.log, () => this.pump(), (job) => this.#release(job));
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
    // Real time: listeners attach once the engine is returned, whatever clock it runs on.
    if (this.#replay !== undefined && this.#replay.length > 0) setTimeout(() => (this.#replay = undefined), REPLAY_WINDOW_MS).unref();
    else this.#replay = undefined;
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
    if (this.abandonment.holds(view.id)) {
      this.#hold(view.id);
      return;
    }
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

  /** Holds a queued job until its abandoned process exits; past `ABANDONED_HOLD_MS` it fails `RESOURCE_STUCK`. */
  #hold(id: string): void {
    this.held.get(id)?.();
    this.d.log("warning", `job ${id}: held until an abandoned process of its last attempt exits`);
    const cancel = this.d.timers.after(ABANDONED_HOLD_MS, () => {
      this.held.delete(id);
      if (this.d.store.row(id)?.state !== "queued") return;
      const error = finError("RESOURCE_STUCK", "a process of the job's last attempt did not exit", { job: id, reason: "abandoned", waitedMs: ABANDONED_HOLD_MS });
      this.track(this.end(id, "failed", { error: data(error) }));
    });
    this.held.set(id, cancel);
  }

  /** The job's last abandoned process exited: a held job is queued for real. */
  #release(id: string): void {
    const cancel = this.held.get(id);
    if (cancel === undefined) return;
    cancel();
    this.held.delete(id);
    const row = this.d.store.row(id);
    const kind = row === undefined ? undefined : this.kinds.kind(row.kind);
    if (row?.state === "queued" && kind !== undefined) this.enqueue(row, kind);
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
      const plan = runPlan(this, row, kind);
      if ("complete" in plan) {
        give();
        await completeIsolation(this, row, kind);
        return;
      }
      const outcome = await runAttempt(this.env, row, kind, family, plan.params, control);
      const stats = this.#statsOf(id);
      stats.runMicros += outcome.runMicros;
      stats.helpers += outcome.helpers;
      give();
      if (this.crashed) return;
      await this.#afterAttempt(id, row, kind, outcome, control, plan.items ?? []);
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

  async #afterAttempt(id: string, row: JobRow, kind: KindSpec, outcome: AttemptOutcome, control: RunControl, items: ReadonlyArray<string>): Promise<void> {
    if (outcome.ok) {
      await afterSuccess(this, row, kind, outcome, items);
      return this.#keepPause(id, control);
    }
    if (control.requested === "cancel") {
      await this.end(id, "cancelled", { error: data(finError("JOB_CANCELLED", "the job was cancelled")) });
      return;
    }
    // A pause or close takes over only a run it stopped; a definitive outcome (the worker's own
    // error, a crash before the stop reached it) follows the policy (9b review M1).
    if (control.requested !== undefined && stoppedRun(outcome)) {
      if (control.pauseRequested) pausedAfterRun(this, id); // a pause asked before a close holds (9b review M2)
      else await interruptRow(this, this.d.store.row(id) ?? row, kind, true);
      return;
    }
    await afterFailure(this, row, kind, outcome, items);
    this.#keepPause(id, control);
  }

  /** A pause asked during a run the policy re-queued (a retry, the next isolation run) still pauses the job. */
  #keepPause(id: string, control: RunControl): void {
    if (!control.pauseRequested || control.requested === "cancel" || this.closing || this.d.store.row(id)?.state !== "queued") return;
    this.queue.remove(id);
    this.held.get(id)?.();
    this.held.delete(id);
    this.publish({ type: "state", job: this.transition(id, "paused", { pausedFrom: "queued" }) });
  }

  /** The job ran isolation (`job.done`'s `isolated`). */
  markIsolated(id: string): void {
    this.#statsOf(id).isolated = true;
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
  async succeed(id: string, kind: KindSpec, value: unknown): Promise<void> {
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
    const stats = this.#stats.get(id) ?? newStats();
    this.#stats.delete(id);
    // The stats go with the terminal row: a `job.done` re-sent at the next start carries them (9b review M5).
    const view = this.transition(id, to, { ...patch, phase: "announce", stats: { ...stats } });
    this.dropProgress(id);
    this.resources.delete(id);
    this.held.get(id)?.();
    this.held.delete(id);
    this.announce(view, stats);
    try {
      await removeJobDir(jobDirs(this.d.root, id).job);
    } catch (e) {
      this.d.log("warning", `job ${id}: its directory could not be deleted: ${toFinError(e).message}`);
    }
  }

  /**
   * `job.done` for a terminal row. The row is marked announced only once the sink took it
   * (9b review M4): a throwing sink leaves the mark, and the next start sends it again (at
   * least once; the sink dedupes by job id).
   */
  #sendDone(view: JobView, stats: RunStats): void {
    try {
      this.d.sink.jobDone(doneEvent(view, this.kinds.kind(view.kind), stats, (m) => this.d.log("warning", m)), view.id);
      if (!this.crashed) this.d.store.announced(view.id);
    } catch (e) {
      this.d.log("error", `job ${view.id}: job.done failed (sent again at the next start): ${toFinError(e).message}`);
    }
  }

  /** `job.done`, then the bus, `onDone` listeners, waiters and the next dispatch. */
  announce(view: JobView, stats: RunStats = newStats()): void {
    this.#sendDone(view, stats);
    this.publish({ type: "state", job: view });
    for (const l of [...this.#listeners]) this.#tell(l, view);
    this.wakeWaiters();
    this.pump();
  }

  /**
   * At start, a terminal row whose `job.done` may not have gone out: `job.done` again with
   * its stored stats; the bus and `onDone` get it once they can attach (9b review M5).
   */
  reannounce(row: JobRow): void {
    const view = viewOf(row);
    this.#sendDone(view, { ...newStats(), ...((row.stats as Partial<RunStats> | undefined) ?? {}) });
    this.#replay?.push(view);
  }

  #tell(l: (job: JobView) => void, view: JobView): void {
    try {
      l(view);
    } catch (e) {
      this.d.log("error", `a job onDone listener threw: ${(e as Error).stack ?? String(e)}`);
    }
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

  /** The bus; a subscriber within `REPLAY_WINDOW_MS` of the start first gets the start's re-announced jobs. */
  events(): Stream.Stream<JobEvent> {
    const live = Stream.fromPubSub(this.#bus);
    return Stream.suspend(() => {
      const replay = (this.#replay ?? []).map((job): JobEvent => ({ type: "state", job }));
      return replay.length === 0 ? live : Stream.concat(Stream.fromIterable(replay), live);
    });
  }

  /** A listener added within `REPLAY_WINDOW_MS` of the start is first told of the start's re-announced jobs. */
  onDone(listener: (job: JobView) => void): () => void {
    this.#listeners.add(listener);
    const replay = [...(this.#replay ?? [])];
    if (replay.length > 0) {
      queueMicrotask(() => {
        for (const v of replay) if (this.#listeners.has(listener)) this.#tell(listener, v);
      });
    }
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
