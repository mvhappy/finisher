// `jobs.watch` (5C-2 plan Decision 8): a job event stream per watcher with its own bounded buffer.
// With `ids`, each job's current view comes first (read and sent in the same turn as the
// subscription, so no event falls between), then only their events; without, every event of a
// job the watcher sees. A watcher `WATCH_BUFFER` events behind ends with `WATCH_LAGGING`: it lists
// again and watches again. The engine's `onEvent` hands over every event, so none is dropped
// silently, and one slow watcher never holds back the engine or another watcher.
import { type FinError, JOB_IDS_MAX, type JobView, type JobWatchEvent, type Principal, WATCH_BUFFER } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../errors.ts";
import { canSee, jobNotFound } from "./access.ts";
import type { JobEngine } from "./api.ts";
import type { JobEvent } from "./view.ts";

const lagging = (buffer: number): FinError =>
  finError(
    "WATCH_LAGGING",
    `this watcher fell ${buffer} job events behind and was ended`,
    { buffer },
    "read the jobs again (jobs.list or jobs.get), then watch again",
  );

const wire = (e: JobEvent): JobWatchEvent =>
  e.type === "item" ? { type: "item", job: e.job, item: e.item, ...(e.error === undefined ? {} : { error: e.error }) } : { type: e.type, job: e.job };

/** `view`: a job's current view, read synchronously (`undefined`: unknown). */
export function watchJobs(
  engine: Pick<JobEngine, "onEvent">,
  view: (id: string) => JobView | undefined,
  principal: Principal,
  req: { readonly ids?: ReadonlyArray<string> | undefined },
  buffer: number = WATCH_BUFFER,
): Stream.Stream<JobWatchEvent, FinError> {
  const ids = req.ids === undefined ? undefined : new Set(req.ids);
  if (ids !== undefined && (req.ids?.length ?? 0) > JOB_IDS_MAX) {
    return Stream.fail(finError("INVALID_REQUEST", `jobs.watch takes at most ${JOB_IDS_MAX} ids`, { field: "ids", max: JOB_IDS_MAX }));
  }
  return Stream.callback<JobWatchEvent, FinError>(
    (queue) =>
      Effect.gen(function* () {
        // One synchronous turn: the current views, then the subscription.
        const unsubscribe = yield* Effect.suspend(() => {
          const first: JobWatchEvent[] = [];
          for (const id of ids ?? []) {
            const v = view(id);
            if (v === undefined || !canSee(principal, v)) return Effect.fail(jobNotFound(id));
            first.push({ type: "state", job: v });
          }
          for (const e of first) Queue.offerUnsafe(queue, e);
          let ended = false;
          const off = engine.onEvent((e) => {
            if (ended || (ids !== undefined && !ids.has(e.job.id)) || !canSee(principal, e.job)) return;
            if (Queue.offerUnsafe(queue, wire(e))) return;
            ended = true;
            off();
            Queue.failCauseUnsafe(queue, Cause.fail(lagging(buffer)));
          });
          return Effect.succeed(off);
        });
        yield* Effect.addFinalizer(() => Effect.sync(unsubscribe));
      }),
    { bufferSize: buffer, strategy: "dropping" },
  );
}
