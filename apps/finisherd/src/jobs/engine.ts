// The job engine (spec §6.0–6.5, 5C-1 plan Tasks 9a, 9b): submit (Decision 27), the dispatch
// loop over the pools and the scheduler, the hook phase, `job.done` and the bus. A dispatched
// job's run is in `attempt-flow.ts`; its public types are in `api.ts`; cancel, pause, resume,
// abandonment and the hold of a job with an abandoned process in `control.ts`; start-up
// recovery, retention and close in `restart.ts`; warm workers, their direct hand-off at submit
// and `warm(family)` in `warm.ts` (Task 11). The retry policy and isolation (Decisions 31, 32)
// are in `retry.ts` over the pure `isolation.ts`.
import type { FinError, FinErrorData, JobOwner, JobProgress, JobState, JobView } from "@finisher/contracts";
import { Effect, type Fiber, PubSub, Stream } from "effect";
import { finError, toFinError } from "../errors.ts";
import { Abandonment, cancelJob, pauseJobs, resumeJobs } from "./control.ts";
import { runDispatched, type WarmLease } from "./attempt-flow.ts";
import { removeJobDir, jobDirs } from "./files.ts";
import { type KindRegistry, type KindSpec, type PoolName, resolveOrFn } from "./kinds.ts";
import { closeEngine, crashEngine, recover } from "./restart.ts";
import { submitJob, waitJobs } from "./submit.ts";
import { runOnSucceeded } from "./hooks.ts";
import { RunControl } from "./run-control.ts";
import { ALIVE_MS, type RunnerEnv } from "./runner.ts";
import { QueueIndex, type QueuedJob, schedule } from "./schedule.ts";
import { WarmPools } from "./warm.ts";
import { type JobQuery, type JobRow, type JobStore, type TransitionPatch, viewOf } from "./store.ts";
import { type JobEvent, type RunStats, doneEvent, newStats, withProgress } from "./view.ts";

export type { JobEvent } from "./view.ts";
export { stoppedRun, type WarmLease } from "./attempt-flow.ts";
export { type EngineDeps, type JobEngine, JobEngineTag, type SubmitSpec, type WorkersView } from "./api.ts";
import type { EngineDeps, JobEngine, SubmitSpec, WorkersView } from "./api.ts";
import { jobNotFound, ownerKey, ownerLimited } from "./access.ts";

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
  readonly warmPools: WarmPools;
  /** The scheduler (a test may spy on it: a direct hand-off never calls it). */
  schedule: typeof schedule = schedule;
  /** Queued jobs held back while a process of their last attempt is abandoned and alive (9b review H1). */
  readonly held = new Map<string, () => void>();
  /** Views re-announced at start, for listeners that attach just after it (9b review M5). */
  #replay: JobView[] | undefined = [];
  readonly #stats = new Map<string, RunStats>();
  readonly #progress = new Map<string, { p: JobProgress; last: number; cancel?: (() => void) | undefined }>();
  readonly #listeners = new Set<(job: JobView) => void>();
  readonly #eventListeners = new Set<(e: JobEvent) => void>();
  /**
   * By job id, called after that job's end and each of its runs' end (`wait`, `settled`): an
   * ending job wakes only its own waiters (5C-2 review M1).
   */
  readonly waiters = new Map<string, Set<() => void>>();
  /** Unfinished jobs per limited owner (`ownerKey`; `ownerLimited`), kept in memory (5C-2 review M4). */
  readonly owned = new Map<string, number>();
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
  #pumping = false;
  #pumpAgain = false;

  constructor(d: EngineDeps) {
    this.d = d;
    this.kinds = d.registry;
    this.#bus = Effect.runSync(PubSub.sliding<JobEvent>(4096));
    this.abandonment = new Abandonment(d.pools, d.log, () => this.pump(), (job) => this.#release(job));
    d.host.onAbandonedExit = (proc) => this.abandonment.exited(proc);
    this.warmPools = new WarmPools(this);
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
      heldFilesCheck: d.heldFilesCheck,
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
    this.owned.clear();
    for (const { owner, n } of this.d.store.unfinishedByOwner()) if (ownerLimited(owner)) this.owned.set(ownerKey(owner), n);
    // Real time: listeners attach once the engine is returned, whatever clock it runs on.
    if (this.#replay !== undefined && this.#replay.length > 0) setTimeout(() => (this.#replay = undefined), REPLAY_WINDOW_MS).unref();
    else this.#replay = undefined;
    this.pump();
  }

  publish(e: JobEvent): void {
    if (this.crashed) return;
    Effect.runSync(PubSub.publish(this.#bus, e));
    for (const l of this.#eventListeners) {
      try {
        l(e);
      } catch (err) {
        this.d.log("warning", `a job event listener threw: ${String(err)}`);
      }
    }
  }

  onEvent(listener: (e: JobEvent) => void): () => void {
    this.#eventListeners.add(listener);
    return () => {
      this.#eventListeners.delete(listener);
    };
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
    if (before !== undefined) {
      const delta = (finished(view) ? 0 : 1) - (finished(before) ? 0 : 1);
      this.unfinished += delta;
      if (delta !== 0) this.countOwned(before.owner, delta);
    }
    return view;
  }

  /** Adds `delta` to `owner`'s unfinished jobs, when its owner kind is limited. */
  countOwned(owner: JobOwner, delta: number): void {
    if (!ownerLimited(owner)) return;
    const key = ownerKey(owner);
    const n = (this.owned.get(key) ?? 0) + delta;
    if (n > 0) this.owned.set(key, n);
    else this.owned.delete(key);
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
    const warm = this.warmPools.tag(view, kind);
    const q: QueuedJob = {
      id: view.id,
      priority: view.priority,
      createdAt: view.createdAt,
      resources,
      ...(view.nextRunAt === undefined ? {} : { nextRunAt: view.nextRunAt }),
      ...(warm === undefined ? {} : { warm }),
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

  /**
   * Dispatches what can run now (synchronously: a submit's job starts in its own turn). A
   * `pump` asked while one runs (a run that ends synchronously gives its slots back) makes the
   * running one go round again instead of nesting: a nested pass would act on a schedule the
   * outer one has half applied (Task 11 review M3).
   */
  pump(): void {
    if (this.#pumping) {
      this.#pumpAgain = true;
      return;
    }
    this.#pumping = true;
    try {
      do {
        this.#pumpAgain = false;
        if (this.closing || this.pausedAll || this.crashed) return;
        for (const d of this.schedule({ queue: this.queue, pools: this.d.pools, warm: this.warmPools.schedState() }, this.d.now())) {
          if (d.mode === "cold") this.dispatch(d.job);
          else if (d.mode === "warm") this.warmPools.dispatchQueued(d.job, d.family, d.key);
          else this.warmPools.spawnFor(d.family, d.key, d.retire);
        }
      } while (this.#pumpAgain);
    } finally {
      this.#pumping = false;
    }
  }

  /** Starts queued job `id`, cold or on the warm worker `lease` lends; false (the lease given back) when it cannot run. */
  dispatch(id: string, lease?: WarmLease): boolean {
    const resources = this.resources.get(id) ?? [];
    const row = this.d.store.row(id);
    const runnable = row?.state === "queued";
    if (!runnable || !this.d.pools.take(resources, lease === undefined ? undefined : this.warmPools.overrun())) {
      // A warm job whose slots are taken waits in the queue; anything else is a bug.
      if (runnable && lease !== undefined) this.d.log("debug", `job ${id}: its slots are taken; it waits`);
      else {
        this.queue.remove(id);
        this.d.log("error", `job ${id}: dispatched but not runnable (${row?.state ?? "gone"})`);
      }
      lease?.release();
      return false;
    }
    this.queue.remove(id);
    const view = this.transition(id, "running", { attempts: row.attempts + 1, nextRunAt: null, warm: lease !== undefined });
    this.publish({ type: "state", job: view });
    const control = new RunControl();
    this.runs.set(id, control);
    this.track(runDispatched(this, id, resources, control, lease));
    return true;
  }

  /** Keeps `p` among the runs `close` waits for. */
  track(p: Promise<void>): void {
    const run = p.catch((e) => this.d.log("error", `job engine: a run failed: ${(e as Error).stack ?? String(e)}`));
    this.active.add(run);
    void run.finally(() => this.active.delete(run));
  }

  /** The job ran isolation (`job.done`'s `isolated`). */
  markIsolated(id: string): void {
    this.statsOf(id).isolated = true;
  }

  /** The run statistics gathered for `job.done`. */
  statsOf(id: string): RunStats {
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
    this.warmPools.forget(id);
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
    this.wakeWaiters(view.id);
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
        this.unwait(id, on);
        resolve();
      };
      this.addWaiter(id, on);
      on();
    });
  }

  /** Calls `w` after `id` ends or one of its runs ends, until `unwait`. */
  addWaiter(id: string, w: () => void): void {
    const set = this.waiters.get(id);
    if (set === undefined) this.waiters.set(id, new Set([w]));
    else set.add(w);
  }

  unwait(id: string, w: () => void): void {
    const set = this.waiters.get(id);
    if (set === undefined) return;
    set.delete(w);
    if (set.size === 0) this.waiters.delete(id);
  }

  /** Wakes the `settled` and `wait` callers of `id`. */
  wakeWaiters(id: string): void {
    const set = this.waiters.get(id);
    if (set !== undefined) for (const w of [...set]) w();
  }

  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError> {
    return Effect.try({ try: () => submitJob(this, owner, spec), catch: toFinError });
  }

  get(id: string): Effect.Effect<JobView, FinError> {
    return Effect.suspend(() => {
      const v = this.view(id);
      return v === undefined ? Effect.fail(jobNotFound(id)) : Effect.succeed(v);
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

  /** Pre-warming (Decision 36): one worker of the family now if its pool has none. */
  warm(family: string): Effect.Effect<void, FinError> {
    return Effect.suspend(() => {
      const f = this.kinds.family(family);
      if (f === undefined) return Effect.fail(finError("JOB_KIND_UNKNOWN", `no worker family ${family}`, { family }));
      if (!this.warmPools.families().has(family)) return Effect.fail(finError("WORKER_UNAVAILABLE", `the ${family} family serves no warm kind`, { family, reason: "notWarm" }));
      if (this.closing) return Effect.fail(finError("WORKER_UNAVAILABLE", "the job engine is closing", { reason: "closing" }));
      this.warmPools.prewarm(f);
      return Effect.void;
    });
  }

  definePool(name: PoolName, size: number): void {
    this.d.pools.define(name, size);
    this.pump();
  }

  workers(): WorkersView {
    const pools = this.d.pools.snapshot();
    return {
      pools,
      warm: this.warmPools.list(),
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
