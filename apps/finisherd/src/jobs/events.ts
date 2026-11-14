// The job engine's event sink (spec §6.8, 5C-1 plan Decision 42): `job.done`, `worker.exit`
// and forwarded `diag` payloads. `finisherd` gives the engine `journalSink` (`journal-sink.ts`,
// D34, Task 12); the engine's tests read `memorySink()`.
import type { JobDone, WorkerExit } from "@finisher/core-types";
import { Context, Layer } from "effect";

export type JobDoneEvent = JobDone & { readonly extra?: Readonly<Record<string, unknown>> };
export type WorkerExitEvent = Omit<WorkerExit, "plugin"> & { readonly pluginId?: string };

/** A process's lifetime in whole seconds (`worker.exit`'s `lifetimeS` is a `u64`). */
export const lifetimeSeconds = (ms: number): number => (Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0);
export interface ForwardedEvent {
  readonly origin: { readonly family: string; readonly job?: string };
  readonly kind: string;
  readonly payload: unknown;
}

export interface JobEventSink {
  /**
   * At least once per finished job (9b review M4): the engine marks the row announced only
   * after this returns, so a throw, or a crash in between, has it sent again at the next
   * start. Dedupe by `job` (the job's id; not part of the journal payload).
   */
  jobDone(e: JobDoneEvent, job: string): void;
  workerExit(e: WorkerExitEvent): void;
  forward(origin: { readonly family: string; readonly job?: string }, kind: string, payload: unknown): void;
}

export class JobEventSinkTag extends Context.Service<JobEventSinkTag, JobEventSink>()("finisherd/JobEventSink") {}

export interface MemorySink extends JobEventSink {
  readonly done: ReadonlyArray<JobDoneEvent>;
  /** The job id of each of `done`. */
  readonly doneJobs: ReadonlyArray<string>;
  readonly exits: ReadonlyArray<WorkerExitEvent>;
  readonly forwarded: ReadonlyArray<ForwardedEvent>;
}

/** Records the last `cap` (default 1,000) events of each kind, oldest first. */
export function memorySink(cap = 1000): MemorySink {
  const size = Math.max(1, Math.floor(cap));
  const done: JobDoneEvent[] = [];
  const doneJobs: string[] = [];
  const exits: WorkerExitEvent[] = [];
  const forwarded: ForwardedEvent[] = [];
  const push = <T>(list: T[], e: T): void => {
    list.push(e);
    if (list.length > size) list.splice(0, list.length - size);
  };
  return {
    done,
    doneJobs,
    exits,
    forwarded,
    jobDone: (e, job) => {
      push(done, e);
      push(doneJobs, job);
    },
    workerExit: (e) => push(exits, e),
    forward: (origin, kind, payload) => push(forwarded, { origin, kind, payload }),
  };
}

/** `memorySink(cap)` as the sink layer. */
export const layerMemorySink = (cap?: number): Layer.Layer<JobEventSinkTag> => Layer.succeed(JobEventSinkTag, memorySink(cap));
