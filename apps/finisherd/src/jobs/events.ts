// The job engine's event sink (spec §6.8, 5C-1 plan Decision 42): `job.done`, `worker.exit`
// and forwarded `diag` payloads. Until diagnostics D4 wires the journal into finisherd the
// engine is given `memorySink()`; D4 replaces it with a journal sink, which hashes a
// `pluginId` into `worker.exit`'s `plugin: Hashed`.
import type { JobDone, WorkerExit } from "@finisher/core-types";
import { Context, Layer } from "effect";

export type JobDoneEvent = JobDone & { readonly extra?: Readonly<Record<string, unknown>> };
export type WorkerExitEvent = Omit<WorkerExit, "plugin"> & { readonly pluginId?: string };
export interface ForwardedEvent {
  readonly origin: { readonly family: string; readonly job?: string };
  readonly kind: string;
  readonly payload: unknown;
}

export interface JobEventSink {
  jobDone(e: JobDoneEvent): void;
  workerExit(e: WorkerExitEvent): void;
  forward(origin: { readonly family: string; readonly job?: string }, kind: string, payload: unknown): void;
}

export class JobEventSinkTag extends Context.Service<JobEventSinkTag, JobEventSink>()("finisherd/JobEventSink") {}

export interface MemorySink extends JobEventSink {
  readonly done: ReadonlyArray<JobDoneEvent>;
  readonly exits: ReadonlyArray<WorkerExitEvent>;
  readonly forwarded: ReadonlyArray<ForwardedEvent>;
}

/** Records the last `cap` (default 1,000) events of each kind, oldest first. */
export function memorySink(cap = 1000): MemorySink {
  const size = Math.max(1, Math.floor(cap));
  const done: JobDoneEvent[] = [];
  const exits: WorkerExitEvent[] = [];
  const forwarded: ForwardedEvent[] = [];
  const push = <T>(list: T[], e: T): void => {
    list.push(e);
    if (list.length > size) list.splice(0, list.length - size);
  };
  return {
    done,
    exits,
    forwarded,
    jobDone: (e) => push(done, e),
    workerExit: (e) => push(exits, e),
    forward: (origin, kind, payload) => push(forwarded, { origin, kind, payload }),
  };
}

/** `memorySink(cap)` as the sink layer. */
export const layerMemorySink = (cap?: number): Layer.Layer<JobEventSinkTag> => Layer.succeed(JobEventSinkTag, memorySink(cap));
