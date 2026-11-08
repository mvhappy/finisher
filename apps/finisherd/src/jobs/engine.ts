// The job engine (spec §6.0–6.5, 5C-1 plan Task 9a): submit (Decision 27), the dispatch loop
// over the pools and the scheduler, the transitions of a cold job's attempts, the hook phase,
// `job.done` and the bus. Until later tasks: helpers, pause and abandonment answer
// `WORKER_UNAVAILABLE {notYet}` (Task 9b); a `warm` kind runs cold and `warm(family)` does
// nothing (Task 11); a failure ends the job unless the worker asked for a retry (Task 10).
import { type FinError, type FinErrorData, type JobOwner, type JobProgress, type JobState, type JobView, type JobVia, type Priority } from "@finisher/contracts";
import { type JobHostCaps, validateJobParams } from "@finisher/fin-node";
import { Context, Effect, PubSub, Stream } from "effect";
import { finError, toFinError } from "../errors.ts";
import type { RunFork } from "./calls.ts";
import type { JobEventSink } from "./events.ts";
import { removeJobDir, jobDirs, sweep } from "./files.ts";
import type { EngineHost, ProcView } from "./host.ts";
import { type KindRegistry, type KindSpec, type PoolName, resolveOrFn, validateMappedFiles } from "./kinds.ts";
import { type PoolView, type Pools } from "./pools.ts";
import { type AttemptOutcome, ALIVE_MS, type RunnerEnv, runAttempt, runOnSucceeded } from "./runner.ts";
import { QueueIndex, type QueuedJob, schedule } from "./schedule.ts";
import type { JobQuery, JobStore, TransitionPatch } from "./store.ts";
import { type JobEvent, type RunStats, doneEvent, newStats, withProgress } from "./view.ts";

export type { JobEvent } from "./view.ts";

export const QUEUE_LIMIT = 50_000;
const PROGRESS_EVERY_MS = 250;
const CLOSE_WAIT_MS = 5000;

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
  readonly log: RunnerEnv["log"];
  readonly diagnostics: () => boolean;
  readonly testHooks: boolean;
  readonly queueLimit: number;
}

const notYet = (what: string): FinError =>
  finError("WORKER_UNAVAILABLE", `${what} is not available yet`, { notYet: what }, "this server's job engine does not do this yet");

const finished = (v: JobView): boolean => v.finishedAt !== undefined;

/** A `FinError`'s stored form. */
function data(e: FinError): FinErrorData {
  return e.fix === undefined ? { code: e.code, message: e.message, details: e.details } : { code: e.code, message: e.message, details: e.details, fix: e.fix };
}

class Engine implements JobEngine {
  readonly kinds: KindRegistry;
  readonly #d: EngineDeps;
  readonly #env: RunnerEnv;
  readonly #queue = new QueueIndex();
  readonly #resources = new Map<string, ReadonlyArray<string>>();
  readonly #stats = new Map<string, RunStats>();
  readonly #progress = new Map<string, { p: JobProgress; last: number; timer?: ReturnType<typeof setTimeout> | undefined }>();
  readonly #listeners = new Set<(job: JobView) => void>();
  readonly #waiters = new Set<() => void>();
  readonly #active = new Set<Promise<void>>();
  readonly #bus: PubSub.PubSub<JobEvent>;
  #wake: { at: number; timer: ReturnType<typeof setTimeout> } | undefined;
  #closing = false;
  #diagDropped = 0;

  constructor(d: EngineDeps) {
    this.#d = d;
    this.kinds = d.registry;
    this.#bus = Effect.runSync(PubSub.sliding<JobEvent>(4096));
    this.#env = {
      host: d.host,
      store: d.store,
      registry: d.registry,
      sink: d.sink,
      root: d.root,
      totalMem: d.totalMem,
      now: d.now,
      run: d.run,
      diagnostics: d.diagnostics,
      testHooks: d.testHooks,
      aliveMs: ALIVE_MS,
      log: d.log,
      progress: (job, p) => this.#onProgress(job, p),
      item: (job, item, outcome) => {
        const view = d.store.get(job);
        if (view !== undefined) this.#publish({ type: "item", job: view, item, ...("error" in outcome ? { error: { code: outcome.error.code } } : {}) });
      },
      diagDropped: () => {
        this.#diagDropped++;
      },
    };
  }

  /** Start-up: leftover job directories swept (but those of `phase: hooks` jobs), queued jobs indexed. */
  async start(): Promise<void> {
    const { store } = this.#d;
    const keep = new Set(store.inHooks().map((j) => j.id));
    const removed = await sweep(this.#d.root, keep);
    if (removed.length > 0) this.#d.log("debug", `job engine: swept ${removed.length} leftover job directories`);
    for (const j of store.queued()) {
      const kind = this.kinds.kind(j.kind);
      if (kind === undefined) {
        this.#d.log("warning", `job ${j.id}: its kind ${j.kind} is not registered; it stays queued`);
        continue;
      }
      this.#enqueue(j, kind);
    }
    const leftover = store.running().length;
    if (leftover > 0) this.#d.log("warning", `job engine: ${leftover} job(s) left running by the last run (restart recovery: Task 9b)`);
    this.#pump();
  }

  #publish(e: JobEvent): void {
    Effect.runSync(PubSub.publish(this.#bus, e));
  }

  #view(id: string): JobView | undefined {
    const v = this.#d.store.get(id);
    return v === undefined ? undefined : withProgress(v, this.#progress.get(id)?.p);
  }

  #onProgress(job: string, p: JobProgress): void {
    const held = this.#progress.get(job) ?? { p, last: -Infinity };
    held.p = p;
    this.#progress.set(job, held);
    const emit = (): void => {
      held.timer = undefined;
      held.last = this.#d.now();
      const view = this.#view(job);
      if (view !== undefined && !finished(view)) this.#publish({ type: "progress", job: view });
    };
    const wait = held.last + PROGRESS_EVERY_MS - this.#d.now();
    if (wait <= 0) emit();
    else held.timer ??= setTimeout(emit, wait);
  }

  #dropProgress(job: string): void {
    const held = this.#progress.get(job);
    if (held?.timer !== undefined) clearTimeout(held.timer);
    this.#progress.delete(job);
  }

  #enqueue(view: JobView, kind: KindSpec): void {
    const resources = resolveOrFn(kind.resources, view.params as never);
    this.#resources.set(view.id, resources);
    const q: QueuedJob = {
      id: view.id,
      priority: view.priority,
      createdAt: view.createdAt,
      resources,
      ...(view.nextRunAt === undefined ? {} : { nextRunAt: view.nextRunAt }),
    };
    this.#queue.add(q);
    if (q.nextRunAt !== undefined) this.#wakeAt(q.nextRunAt);
  }

  #wakeAt(at: number): void {
    if (this.#wake !== undefined && this.#wake.at <= at) return;
    if (this.#wake !== undefined) clearTimeout(this.#wake.timer);
    const timer = setTimeout(() => {
      this.#wake = undefined;
      this.#pump();
    }, Math.max(0, at - this.#d.now()));
    this.#wake = { at, timer };
  }

  /** Dispatches what can run now (synchronously: a submit's job starts in its own turn). */
  #pump(): void {
    if (this.#closing) return;
    for (const d of schedule({ queue: this.#queue, pools: this.#d.pools, warm: new Map() }, this.#d.now())) {
      if (d.mode === "cold") this.#dispatch(d.job);
    }
  }

  #dispatch(id: string): void {
    const resources = this.#resources.get(id) ?? [];
    this.#queue.remove(id);
    const row = this.#d.store.row(id);
    if (row === undefined || row.state !== "queued" || !this.#d.pools.take(resources)) {
      this.#d.log("error", `job ${id}: dispatched but not runnable (${row?.state ?? "gone"})`);
      return;
    }
    const view = this.#d.store.transition(id, "running", { attempts: row.attempts + 1, nextRunAt: null });
    this.#publish({ type: "state", job: view });
    const run = this.#run(id).catch((e) => this.#d.log("error", `job ${id}: the runner failed: ${(e as Error).stack ?? String(e)}`));
    this.#active.add(run);
    void run.finally(() => this.#active.delete(run));
  }

  async #run(id: string): Promise<void> {
    const { store } = this.#d;
    const row = store.row(id);
    const kind = row === undefined ? undefined : this.kinds.kind(row.kind);
    const family = kind === undefined ? undefined : this.kinds.family(kind.family);
    if (row === undefined || kind === undefined || family === undefined) {
      this.#d.pools.give(this.#resources.get(id) ?? []);
      if (row !== undefined) await this.#end(id, "failed", { error: data(finError("JOB_KIND_UNKNOWN", `no kind ${row.kind}`, { kind: row.kind })) });
      return;
    }
    const outcome = await runAttempt(this.#env, row, kind, family, row.params);
    this.#statsOf(id).runMicros += outcome.runMicros;
    this.#d.pools.give(this.#resources.get(id) ?? []);
    if (this.#closing) return; // the row stays as it is; the next start recovers it (Task 9b)
    if (outcome.ok) return this.#succeed(id, kind, outcome.value);
    if (this.#retryable(row.attempts, row.maxAttempts, outcome)) {
      const view = store.transition(id, "queued", { error: data(outcome.error), nextRunAt: null });
      this.#dropProgress(id);
      this.#publish({ type: "state", job: view });
      this.#enqueue(view, kind);
      this.#pump();
      return;
    }
    await this.#end(id, "failed", { error: data(outcome.error) });
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
    const { store } = this.#d;
    this.#publish({ type: "state", job: store.enterHooks(id, value) });
    const row = store.row(id);
    if (row === undefined) return;
    const out = await runOnSucceeded(this.#env, row, kind, value, false);
    if (this.#closing) return;
    if (out.ok) await this.#end(id, "succeeded", { result: out.result });
    else await this.#end(id, "failed", { error: data(out.error) });
  }

  /** The terminal transition, the directory deleted, then `job.done`, the bus and waiters. */
  async #end(id: string, to: JobState, patch: TransitionPatch): Promise<void> {
    const view = this.#d.store.transition(id, to, patch);
    this.#dropProgress(id);
    this.#resources.delete(id);
    try {
      await removeJobDir(jobDirs(this.#d.root, id).job);
    } catch (e) {
      this.#d.log("warning", `job ${id}: its directory could not be deleted: ${toFinError(e).message}`);
    }
    const stats = this.#stats.get(id) ?? newStats();
    this.#stats.delete(id);
    this.#d.sink.jobDone(doneEvent(view, this.kinds.kind(view.kind), stats, (m) => this.#d.log("warning", m)));
    this.#publish({ type: "state", job: view });
    for (const l of [...this.#listeners]) {
      try {
        l(view);
      } catch (e) {
        this.#d.log("error", `a job onDone listener threw: ${(e as Error).stack ?? String(e)}`);
      }
    }
    for (const w of [...this.#waiters]) w();
    this.#pump();
  }

  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError> {
    return Effect.suspend(() => {
      try {
        return Effect.succeed(this.#submit(owner, spec));
      } catch (e) {
        return Effect.fail(toFinError(e));
      }
    });
  }

  #submit(owner: JobOwner, spec: SubmitSpec): JobView {
    const { store } = this.#d;
    const kind = this.kinds.kind(spec.kind);
    if (kind === undefined) throw finError("JOB_KIND_UNKNOWN", `no job kind ${spec.kind}`, { kind: spec.kind });
    const checked = validateJobParams(kind.kind, kind.version, spec.params);
    if (!checked.ok) throw finError("JOB_PARAMS_INVALID", `the params of ${kind.kind} are invalid: ${checked.errors.join("; ")}`, { errors: checked.errors });
    const params = checked.params;
    if (kind.files !== undefined && typeof kind.files === "function") {
      const errors = validateMappedFiles(kind.files(params as never));
      if (errors.length > 0) throw finError("JOB_PARAMS_INVALID", `the job's files are invalid: ${errors.join("; ")}`, { errors });
    }
    for (const pool of resolveOrFn(kind.resources, params as never)) {
      if (this.#d.pools.size(pool) === undefined) throw finError("WORKER_UNAVAILABLE", `no resource pool ${pool}`, { pool });
    }
    if (spec.idemKey !== undefined) {
      const same = store.byIdem(owner, spec.idemKey);
      if (same !== undefined) return this.#view(same.id) ?? same;
    }
    const dedupeKey = kind.dedupe?.(params as never);
    if (dedupeKey !== undefined) {
      const active = store.activeByDedupe(kind.kind, dedupeKey) ?? (kind.rememberPoison === true ? store.poisonedByDedupe(kind.kind, dedupeKey) : undefined);
      if (active !== undefined) return this.#view(active.id) ?? active;
    }
    if (store.unfinishedCount() >= this.#d.queueLimit) {
      throw finError("JOB_QUEUE_FULL", "the job queue is full", { limit: this.#d.queueLimit }, "wait for jobs to finish, or cancel some");
    }
    const priority = spec.priority ?? kind.defaultPriority;
    const view = store.insert({
      kind: kind.kind,
      kindVersion: kind.version,
      family: kind.family,
      params,
      owner,
      priority,
      maxAttempts: spec.maxAttempts ?? kind.maxAttempts ?? (priority === "interactive" ? 2 : 3),
      ...(spec.via === undefined ? {} : { via: spec.via }),
      ...(spec.rights === undefined ? {} : { rights: spec.rights }),
      ...(spec.project === undefined ? {} : { project: spec.project }),
      ...(spec.library === undefined ? {} : { library: spec.library }),
      ...(spec.parent === undefined ? {} : { parent: spec.parent }),
      ...(spec.idemKey === undefined ? {} : { idemKey: spec.idemKey }),
      ...(dedupeKey === undefined ? {} : { dedupeKey }),
    });
    this.#publish({ type: "state", job: view });
    this.#enqueue(view, kind);
    this.#pump();
    return this.#view(view.id) ?? view;
  }

  get(id: string): Effect.Effect<JobView, FinError> {
    return Effect.suspend(() => {
      const v = this.#view(id);
      return v === undefined ? Effect.fail(finError("JOB_NOT_FOUND", `no job ${id}`, { job: id })) : Effect.succeed(v);
    });
  }

  list(q: JobQuery): { readonly jobs: JobView[]; readonly next?: string } {
    const page = this.#d.store.list(q);
    const jobs = page.jobs.map((j) => withProgress(j, this.#progress.get(j.id)?.p));
    return page.next === undefined ? { jobs } : { jobs, next: page.next };
  }

  wait(ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError> {
    return Effect.suspend(() => {
      for (const id of ids) if (this.#d.store.get(id) === undefined) return Effect.fail(finError("JOB_NOT_FOUND", `no job ${id}`, { job: id }));
      const check = (): JobView[] | undefined => {
        const views = ids.map((id) => this.#view(id) as JobView);
        const n = views.filter(finished).length;
        return (opts.any === true ? n > 0 : n === views.length) ? views : undefined;
      };
      const now = check();
      if (now !== undefined) return Effect.succeed(now);
      return Effect.callback<ReadonlyArray<JobView>, FinError>((resume) => {
        const cleanup = (): void => {
          clearTimeout(timer);
          this.#waiters.delete(onDone);
        };
        const onDone = (): void => {
          const views = check();
          if (views === undefined) return;
          cleanup();
          resume(Effect.succeed(views));
        };
        const timer = setTimeout(() => {
          cleanup();
          resume(Effect.fail(finError("WAIT_TIMEOUT", `the jobs did not finish within ${opts.timeoutMs} ms`, { jobs: ids, timeoutMs: opts.timeoutMs })));
        }, Math.max(0, opts.timeoutMs));
        this.#waiters.add(onDone);
        return Effect.sync(cleanup);
      });
    });
  }

  cancel(id: string): Effect.Effect<JobView, FinError> {
    return Effect.suspend(() => {
      const row = this.#d.store.row(id);
      if (row === undefined) return Effect.fail(finError("JOB_NOT_FOUND", `no job ${id}`, { job: id }));
      if (finished(row) || (row.state === "running" && row.phase === "hooks")) {
        // Too late: a job in its hook phase ends `succeeded` (Decision 40).
        return Effect.succeed(this.#view(id) ?? row);
      }
      if (row.state === "queued") {
        this.#queue.remove(id);
        return Effect.promise(() => this.#end(id, "cancelled", { error: data(finError("JOB_CANCELLED", "the job was cancelled")) })).pipe(
          Effect.map(() => this.#view(id) as JobView),
        );
      }
      return Effect.fail(notYet("cancel"));
    });
  }

  pause(): Effect.Effect<void, FinError> {
    return Effect.fail(notYet("pause"));
  }

  resume(): Effect.Effect<void, FinError> {
    return Effect.fail(notYet("resume"));
  }

  warm(): Effect.Effect<void, FinError> {
    return Effect.void;
  }

  definePool(name: PoolName, size: number): void {
    this.#d.pools.define(name, size);
    this.#pump();
  }

  workers(): WorkersView {
    const pools = this.#d.pools.snapshot();
    return {
      pools,
      warm: [],
      processes: this.#d.host.processes(),
      stuck: pools.filter((p) => p.stuck).map((p) => p.name),
      caps: this.#d.host.caps(),
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

  /**
   * Stops dispatch and shuts the host down; running jobs keep their rows for the next start
   * (Task 9b's graceful close pauses them first and recovers them at restart).
   */
  close(): Effect.Effect<void> {
    return Effect.promise(async () => {
      if (this.#closing) return;
      this.#closing = true;
      if (this.#wake !== undefined) clearTimeout(this.#wake.timer);
      for (const id of [...this.#progress.keys()]) this.#dropProgress(id);
      await this.#d.host.shutdown(2000).catch((e) => this.#d.log("warning", `job host shutdown: ${toFinError(e).message}`));
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([Promise.allSettled([...this.#active]), new Promise((r) => (timer = setTimeout(r, CLOSE_WAIT_MS)))]);
      clearTimeout(timer);
      Effect.runSync(PubSub.shutdown(this.#bus));
    });
  }
}

/** Builds the engine and runs its start-up (sweep, queue). */
export async function makeJobEngine(deps: EngineDeps): Promise<JobEngine> {
  const engine = new Engine(deps);
  await engine.start();
  return engine;
}
