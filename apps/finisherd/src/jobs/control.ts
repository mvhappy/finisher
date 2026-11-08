// Cancel, pause and resume (spec §6.2–6.3, 5C-1 plan Decisions 23, 37) and abandonment
// (Decision 18). A queued job is cancelled or paused at once; a running one is stopped through
// its `RunControl` (`cancel {reason}` to the worker, its group killed after the grace) and the
// call resolves once the run has ended. A job in its hook phase is past stopping: it ends as
// its hook decides. Two abandoned processes of different jobs on one `io:` pool mark the pool
// stuck until one of them exits.
import type { JobView } from "@finisher/contracts";
import { finError } from "../errors.ts";
import { data, type Engine, finished } from "./engine.ts";
import type { Pools } from "./pools.ts";
import { isResumable } from "./restart.ts";
import { jobNotFound } from "./access.ts";

export const CANCEL_GRACE_MS = 2000;
export const CANCEL_GRACE_INTERACTIVE_MS = 500;
export const PAUSE_GRACE_MS = 10_000;


/** Cancels a job; resolves with its view once it ended (or with the view of a finished one). */
export async function cancelJob(e: Engine, id: string): Promise<JobView> {
  const row = e.d.store.row(id);
  if (row === undefined) throw jobNotFound(id);
  const view = (): JobView => e.view(id) ?? row;
  if (finished(row) || (row.state === "running" && row.phase === "hooks")) return view(); // too late (Decision 40)
  if (row.state === "queued" || row.state === "paused") {
    e.queue.remove(id);
    await e.end(id, "cancelled", { error: data(finError("JOB_CANCELLED", "the job was cancelled")), pausedFrom: null });
    return view();
  }
  const control = e.runs.get(id);
  if (control !== undefined) {
    control.request("cancel", row.priority === "interactive" ? CANCEL_GRACE_INTERACTIVE_MS : CANCEL_GRACE_MS);
    await e.settled(id);
  }
  return view();
}

/**
 * Pause (Decision 37). `{job}`: a queued job is `paused` at once and keeps its place; a running
 * resumable one gets `cancel {reason: pause}` and 10 s to checkpoint, then is `paused`; a
 * running job that is not resumable finishes. `{all}`: dispatch stops, running resumable jobs
 * pause as above, the others finish.
 */
export async function pauseJobs(e: Engine, target: { readonly job: string } | { readonly all: true }): Promise<void> {
  if ("all" in target) {
    e.pausedAll = true;
    const waits: Promise<void>[] = [];
    for (const id of [...e.runs.keys()]) waits.push(pauseRunning(e, id));
    await Promise.all(waits);
    return;
  }
  const row = e.d.store.row(target.job);
  if (row === undefined) throw jobNotFound(target.job);
  if (row.state === "queued") {
    e.queue.remove(row.id);
    e.publish({ type: "state", job: e.transition(row.id, "paused", { pausedFrom: "queued" }) });
    return;
  }
  if (row.state === "running") await pauseRunning(e, row.id);
}

async function pauseRunning(e: Engine, id: string): Promise<void> {
  const row = e.d.store.row(id);
  const kind = row === undefined ? undefined : e.kinds.kind(row.kind);
  const control = e.runs.get(id);
  if (row === undefined || kind === undefined || control === undefined || row.phase !== "run") return;
  if (!isResumable(e.d.store, row, kind)) return; // it finishes and is not paused
  control.request("pause", PAUSE_GRACE_MS);
  await e.settled(id);
}

/** A paused run ended: the job is `paused`; the stopped run is not counted as an attempt. */
export function pausedAfterRun(e: Engine, id: string): void {
  const row = e.d.store.row(id);
  if (row === undefined) return;
  e.dropProgress(id);
  e.publish({ type: "state", job: e.transition(id, "paused", { pausedFrom: "running", attempts: Math.max(0, row.attempts - 1), error: null }) });
}

/** Resume: a paused job is `queued` again with its original `createdAt`; `{all}` also restarts dispatch. */
export function resumeJobs(e: Engine, target: { readonly job: string } | { readonly all: true }): void {
  const ids = "all" in target ? e.d.store.paused().map((j) => j.id) : [target.job];
  if (!("all" in target) && e.d.store.row(target.job) === undefined) throw jobNotFound(target.job);
  if ("all" in target) e.pausedAll = false;
  for (const id of ids) {
    const row = e.d.store.row(id);
    const kind = row === undefined ? undefined : e.kinds.kind(row.kind);
    if (row?.state !== "paused" || kind === undefined) continue;
    const view = e.transition(id, "queued", { pausedFrom: null });
    e.publish({ type: "state", job: view });
    e.enqueue(view, kind);
  }
  e.pump();
}

/** How many early `abandonedExit`s are remembered (9b review H2). */
const TOMBSTONES = 1024;

/**
 * Abandoned processes (Decision 18) and the `io:` pools they make stuck: two abandoned
 * processes on a pool (of one job or several, 9b review L1). A job with an abandoned process
 * is not run again until it really exits (`holds`, 9b review H1): `released` is called then.
 */
export class Abandonment {
  readonly #procs = new Map<number, { job: string; pools: ReadonlyArray<string> }>();
  /** Processes whose real exit was reported before their abandonment (a host race, 9b review H2). */
  readonly #tombstones = new Set<number>();
  readonly #pools: Pools;
  readonly #log: (level: "debug" | "warning" | "error", message: string) => void;
  readonly #pump: () => void;
  readonly #released: (job: string) => void;

  constructor(pools: Pools, log: (level: "debug" | "warning" | "error", message: string) => void, pump: () => void, released: (job: string) => void = () => {}) {
    this.#pools = pools;
    this.#log = log;
    this.#pump = pump;
    this.#released = released;
  }

  abandoned(proc: number, job: string, resources: ReadonlyArray<string>): void {
    if (this.#tombstones.delete(proc)) {
      this.#log("warning", `job ${job}: process ${proc} was abandoned after it had exited`);
      return;
    }
    this.#procs.set(proc, { job, pools: resources.filter((r) => r.startsWith("io:")) });
    this.#log("warning", `job ${job}: process ${proc} did not exit when killed: abandoned`);
    this.#recount();
  }

  /** The abandoned process `proc` has exited at last. */
  exited(proc: number): void {
    const was = this.#procs.get(proc);
    if (was === undefined || !this.#procs.delete(proc)) {
      this.#tombstones.add(proc);
      if (this.#tombstones.size > TOMBSTONES) this.#tombstones.delete(this.#tombstones.values().next().value as number);
      return;
    }
    this.#log("warning", `job ${was.job}: abandoned process ${proc} exited`);
    this.#recount();
    if (!this.holds(was.job)) this.#released(was.job);
    this.#pump();
  }

  /** Whether a process of `job` is abandoned and still alive. */
  holds(job: string): boolean {
    for (const p of this.#procs.values()) if (p.job === job) return true;
    return false;
  }

  #recount(): void {
    const procs = new Map<string, number>();
    for (const { pools } of this.#procs.values()) for (const p of pools) procs.set(p, (procs.get(p) ?? 0) + 1);
    for (const p of this.#pools.snapshot()) {
      const stuck = (procs.get(p.name) ?? 0) >= 2;
      if (stuck !== p.stuck) {
        this.#pools.markStuck(p.name, stuck);
        this.#log("warning", `resource pool ${p.name} ${stuck ? "is stuck: two abandoned processes hold it" : "is no longer stuck"}`);
      }
    }
  }
}
