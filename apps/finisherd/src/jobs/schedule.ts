// The scheduler (spec §6.3, 5C-1 plan Decisions 33–35): a pure `schedule(state, now)` over
// the queue, the pools and the warm pools. Order: effective priority (the base level minus
// one level per 10 minutes waited, never above `interactive`), then `createdAt`, then id.
// The queue is indexed by (resource set, warm key, priority) as FIFOs, so a blocked head never
// hides a runnable job behind it: within one FIFO every job needs the same slots, so once its
// head cannot run, none of it can in this call. `schedule()` serves queued cold jobs and queued
// warm jobs (no ready worker at submit); the direct hand-off at submit does not go through it.
import type { Priority } from "@finisher/contracts";
import { type Pools, slotCounts } from "./pools.ts";

export const AGING_MS = 10 * 60_000;
const LEVEL: Readonly<Record<Priority, number>> = { interactive: 0, normal: 1, background: 2 };

export interface QueuedJob {
  readonly id: string;
  readonly priority: Priority;
  readonly createdAt: number;
  readonly nextRunAt?: number;
  readonly resources: ReadonlyArray<string>;
  /** `fresh`: the job ran before, so its retry takes a worker that has served no job (Decision 31). */
  readonly warm?: { readonly family: string; readonly key: string; readonly fresh?: boolean };
}

export interface WarmWorkerView {
  readonly key: string;
  readonly state: "spawning" | "ready" | "busy";
  readonly idleSince?: number;
  /** Jobs it has served (a `fresh` job takes only a worker with none). */
  readonly served?: number;
}

export interface SchedState {
  readonly queue: QueueIndex;
  readonly pools: Pools;
  readonly warm: ReadonlyMap<string /* family */, { readonly slots: number; readonly workers: ReadonlyArray<WarmWorkerView> }>;
}

export type Dispatch =
  | { readonly job: string; readonly mode: "cold" }
  | { readonly job: string; readonly mode: "warm"; readonly family: string; readonly key: string }
  /** Spawn a warm worker for `key`; first shut down `workers[retire]` of the family. */
  | { readonly mode: "spawnWarm"; readonly family: string; readonly key: string; readonly retire?: number };

/** The level a job competes at: 0 (interactive) … 2 (background). */
export function effectiveLevel(j: QueuedJob, now: number): number {
  const waited = Math.max(0, Math.floor((now - j.createdAt) / AGING_MS));
  return Math.max(0, LEVEL[j.priority] - waited);
}

const byAge = (a: QueuedJob, b: QueuedJob): number => a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
const byDue = (a: QueuedJob, b: QueuedJob): number => (a.nextRunAt ?? 0) - (b.nextRunAt ?? 0) || byAge(a, b);

/** The order of two jobs at `now`: negative when `a` goes first. */
export function compareJobs(a: QueuedJob, b: QueuedJob, now: number): number {
  return effectiveLevel(a, now) - effectiveLevel(b, now) || byAge(a, b);
}

function insertSorted(list: QueuedJob[], j: QueuedJob, cmp: (a: QueuedJob, b: QueuedJob) => number): void {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (cmp(list[mid] as QueuedJob, j) <= 0) lo = mid + 1;
    else hi = mid;
  }
  list.splice(lo, 0, j);
}

function removeSorted(list: QueuedJob[], j: QueuedJob, cmp: (a: QueuedJob, b: QueuedJob) => number): void {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (cmp(list[mid] as QueuedJob, j) < 0) lo = mid + 1;
    else hi = mid;
  }
  for (let i = lo; i < list.length; i++) {
    if ((list[i] as QueuedJob).id === j.id) {
      list.splice(i, 1);
      return;
    }
    if (cmp(list[i] as QueuedJob, j) > 0) return;
  }
}

/** One FIFO: jobs with the same slots, warm key and base priority. */
interface Bucket {
  /** Due now, by (createdAt, id). */
  readonly ready: QueuedJob[];
  /** With a `nextRunAt`, by (nextRunAt, createdAt, id). */
  readonly delayed: QueuedJob[];
}

function bucketKey(j: QueuedJob): string {
  const slots = [...slotCounts(j.resources)].sort(([a], [b]) => (a < b ? -1 : 1)).map(([n, c]) => `${n}*${c}`);
  return `${j.warm ? `w:${j.warm.family}:${j.warm.key}` : "c"}|${slots.join(",")}|${j.priority}`;
}

/** The queued jobs, indexed by (resource set, warm key, priority). */
export class QueueIndex {
  readonly #buckets = new Map<string, Bucket>();
  readonly #byId = new Map<string, { job: QueuedJob; key: string }>();

  add(j: QueuedJob): void {
    this.remove(j.id);
    const key = bucketKey(j);
    let b = this.#buckets.get(key);
    if (b === undefined) {
      b = { ready: [], delayed: [] };
      this.#buckets.set(key, b);
    }
    if (j.nextRunAt === undefined) insertSorted(b.ready, j, byAge);
    else insertSorted(b.delayed, j, byDue);
    this.#byId.set(j.id, { job: j, key });
  }

  remove(id: string): void {
    const at = this.#byId.get(id);
    if (at === undefined) return;
    this.#byId.delete(id);
    const b = this.#buckets.get(at.key);
    if (b === undefined) return;
    if (at.job.nextRunAt === undefined) removeSorted(b.ready, at.job, byAge);
    else removeSorted(b.delayed, at.job, byDue);
    if (b.ready.length === 0 && b.delayed.length === 0) this.#buckets.delete(at.key);
  }

  size(): number {
    return this.#byId.size;
  }

  get(id: string): QueuedJob | undefined {
    return this.#byId.get(id)?.job;
  }

  /** The first due warm job of `(family, key)`, by (createdAt, id). */
  warmHead(family: string, key: string, now: number): QueuedJob | undefined {
    const prefix = `w:${family}:${key}|`;
    let best: QueuedJob | undefined;
    for (const [k, b] of this.#buckets) {
      if (!k.startsWith(prefix)) continue;
      const first = dueStream(b, now).next();
      if (first.done !== true && (best === undefined || byAge(first.value, best) < 0)) best = first.value;
    }
    return best;
  }

  /** Each FIFO's due jobs in order, lazily (for `schedule`). */
  *streams(now: number): Generator<Iterator<QueuedJob>> {
    for (const b of this.#buckets.values()) yield dueStream(b, now);
  }
}

/** A FIFO's jobs due at `now`, by (createdAt, id): the ready list merged with the due delayed. */
function* dueStream(b: Bucket, now: number): Generator<QueuedJob> {
  const due: QueuedJob[] = [];
  for (const j of b.delayed) {
    if ((j.nextRunAt ?? 0) > now) break;
    due.push(j);
  }
  due.sort(byAge);
  let i = 0;
  let k = 0;
  while (i < b.ready.length || k < due.length) {
    const r = b.ready[i];
    const d = due[k];
    if (d === undefined || (r !== undefined && byAge(r, d) <= 0)) {
      i++;
      yield r as QueuedJob;
    } else {
      k++;
      yield d;
    }
  }
}

/**
 * The `cpu` slots a warm dispatch may take beyond the pool's size: the sum of `slots` over
 * every warm family (Decision 35). The caller takes a warm dispatch's slots with
 * `pools.take(resources, warmOverrun(state.warm))` and a cold one's with no overrun; `schedule`
 * checks exactly the same bound, so a dispatch it returns is always takeable.
 */
export function warmOverrun(warm: SchedState["warm"]): { cpu: number } {
  let cpu = 0;
  for (const w of warm.values()) cpu += w.slots;
  return { cpu };
}

type PlanWorker = WarmWorkerView & { retiring?: boolean };

/** What one call decides, applied to copies of the state as it goes. */
class Plan {
  readonly out: Dispatch[] = [];
  readonly #state: SchedState;
  readonly #overrun: number;
  readonly #used = new Map<string, number>();
  readonly #warm = new Map<string, { slots: number; workers: PlanWorker[]; spawning: Set<string> }>();
  /** Per family, the keys with a due queued job (never retired, Decision 35). */
  readonly #dueKeys = new Map<string, Set<string>>();

  constructor(state: SchedState) {
    this.#state = state;
    this.#overrun = warmOverrun(state.warm).cpu;
    for (const [family, w] of state.warm) {
      this.#warm.set(family, { slots: w.slots, workers: w.workers.map((x) => ({ ...x })), spawning: new Set() });
    }
  }

  /** Notes that a due warm job of `key` is queued. */
  due(j: QueuedJob): void {
    if (j.warm === undefined) return;
    let keys = this.#dueKeys.get(j.warm.family);
    if (keys === undefined) {
      keys = new Set();
      this.#dueKeys.set(j.warm.family, keys);
    }
    keys.add(j.warm.key);
  }

  /** As `Pools.take` would answer, with `overrun` extra `cpu` slots. */
  #fits(names: ReadonlyArray<string>, overrun: number): boolean {
    const pools = this.#state.pools;
    for (const [name, n] of slotCounts(names)) {
      const cap = pools.size(name);
      if (cap === undefined || pools.stuck(name)) return false;
      const extra = name === "cpu" ? overrun : 0;
      if (pools.used(name) + (this.#used.get(name) ?? 0) + n > cap + extra) return false;
    }
    return true;
  }

  #take(names: ReadonlyArray<string>): void {
    for (const [name, n] of slotCounts(names)) this.#used.set(name, (this.#used.get(name) ?? 0) + n);
  }

  /**
   * First pass, in global order: a cold job runs if its slots are free; a warm job runs on a
   * ready worker of its own key. `"wait"`: a warm job with no ready worker of its key, decided
   * in the second pass. `false`: the job (and so its FIFO) cannot go on now.
   */
  decide(j: QueuedJob): boolean | "wait" {
    if (j.warm === undefined) {
      if (!this.#fits(j.resources, 0)) return false;
      this.#take(j.resources);
      this.out.push({ job: j.id, mode: "cold" });
      return true;
    }
    const { family, key } = j.warm;
    const pool = this.#warm.get(family);
    if (pool === undefined || !this.#fits(j.resources, this.#overrun)) return false;
    const fresh = j.warm.fresh === true;
    const ready = pool.workers.find((w) => w.key === key && w.state === "ready" && w.retiring !== true && (!fresh || (w.served ?? 0) === 0));
    if (ready === undefined) return "wait";
    Object.assign(ready, { state: "busy" });
    this.#take(j.resources);
    this.out.push({ job: j.id, mode: "warm", family, key });
    return true;
  }

  /**
   * Second pass, for a warm job left waiting: spawn a worker of its key when the family has
   * room (one spawn per key at a time); with the pool full of other keys, retire the
   * longest-idle ready worker whose key has no due job; else it waits.
   */
  place(j: QueuedJob): void {
    if (j.warm === undefined) return;
    const { family, key } = j.warm;
    const fresh = j.warm.fresh === true;
    const pool = this.#warm.get(family);
    if (pool === undefined) return;
    if (pool.spawning.has(key) || pool.workers.some((w) => w.key === key && w.state === "spawning")) return;
    const live = pool.workers.filter((w) => w.retiring !== true).length + pool.spawning.size;
    if (live < pool.slots) {
      pool.spawning.add(key);
      this.out.push({ mode: "spawnWarm", family, key });
      return;
    }
    // A fresh retry may retire a worker of its own key that has served jobs; any other job waits for its key's workers.
    if (!fresh && pool.workers.some((w) => w.key === key && w.retiring !== true)) return;
    const busyKeys = this.#dueKeys.get(family) ?? new Set<string>();
    let retire: number | undefined;
    pool.workers.forEach((w, i) => {
      if (w.state !== "ready" || w.retiring === true) return;
      if (w.key === key ? !fresh || (w.served ?? 0) === 0 : busyKeys.has(w.key)) return;
      const best = retire === undefined ? undefined : pool.workers[retire];
      if (best === undefined || (w.idleSince ?? Infinity) < (best.idleSince ?? Infinity)) retire = i;
    });
    if (retire === undefined) return;
    (pool.workers[retire] as PlanWorker).retiring = true;
    pool.spawning.add(key);
    this.out.push({ mode: "spawnWarm", family, key, retire });
  }
}

/**
 * The dispatches for `now`, in order. Pure: it reads the state and returns what to do; the
 * caller takes the slots (`Pools.take`; a warm dispatch with `warmOverrun(state.warm)`),
 * starts the jobs, spawns and retires warm workers, and removes dispatched jobs from the queue.
 * Warm spawns and retirements come after every dispatch, so a retirement never takes a
 * worker a due job of its own key could run on.
 */
export function schedule(state: SchedState, now: number): Dispatch[] {
  const plan = new Plan(state);
  const heads: Array<{ it: Iterator<QueuedJob>; job: QueuedJob }> = [];
  for (const it of state.queue.streams(now)) {
    const first = it.next();
    if (first.done === true) continue;
    heads.push({ it, job: first.value });
    plan.due(first.value);
  }
  const waiting: QueuedJob[] = [];
  while (heads.length > 0) {
    let best = 0;
    for (let i = 1; i < heads.length; i++) {
      if (compareJobs((heads[i] as { job: QueuedJob }).job, (heads[best] as { job: QueuedJob }).job, now) < 0) best = i;
    }
    const head = heads[best] as { it: Iterator<QueuedJob>; job: QueuedJob };
    const decided = plan.decide(head.job);
    if (decided !== true) {
      if (decided === "wait") waiting.push(head.job);
      heads.splice(best, 1);
      continue;
    }
    const next = head.it.next();
    if (next.done === true) heads.splice(best, 1);
    else head.job = next.value;
  }
  for (const j of waiting) plan.place(j);
  return plan.out;
}
