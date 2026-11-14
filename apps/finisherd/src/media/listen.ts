// `media.listen` (6C plan Task 21, Decision 21; sub-project 6 spec §8.7): the RPC behind
// `fin listen`. It reads the branch head once (the rev, the timelines and their extents),
// resolves the timeline (the first when none is named) and the range (the whole timeline when
// absent), refuses a time that is not a rational (`INVALID_TIME`) and a range that is empty or
// passes the timeline's end (`INVALID_RANGE`), and submits a `media.listen` job with the head it
// read, so equal listens of one head coalesce (`listenDedupe`). With `wait` (the default) it
// answers the job's `ListenResult`; with `wait: false`, `{job}`. Everything else (the plan
// samples, the inputs, the `pcm48` entries, the measuring) is the kind's
// (`jobs/kinds/media.listen.ts`).
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { ListenParams } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { MEDIA_LISTEN } from "../jobs/kinds/media.listen.ts";
import { type MediaRpcDeps, type Rat, type TimelineJson, extent, le, rational, show } from "./look.ts";

/** How long `wait` waits for the job (an hour-long timeline decodes in about a minute). */
export const LISTEN_WAIT_MS = 10 * 60_000;

export interface ListenRequest {
  readonly projectId: string;
  readonly branch?: string | undefined;
  readonly timeline?: string | undefined;
  readonly from?: string | undefined;
  readonly to?: string | undefined;
  readonly wait?: boolean | undefined;
}

const ZERO: Rat = { n: 0n, d: 1n };

const rangeError = (message: string, details: Record<string, unknown>, fix: string): FinError => finError("INVALID_RANGE", message, details, fix);

function resultOf(job: JobView): Effect.Effect<unknown, FinError> {
  if (job.state === "succeeded") return Effect.succeed(job.result);
  const e = job.error;
  return Effect.fail(e == null ? finError("JOB_FAILED", `the listen ended ${job.state}`, { job: job.id, state: job.state }) : finError(e.code, e.message, e.details, e.fix));
}

/**
 * `media.listen`. Errors: `PROJECT_NOT_FOUND`, `BRANCH_NOT_FOUND`, `NOT_FOUND` (the timeline),
 * `INVALID_TIME`, `INVALID_RANGE` (empty, negative or past the timeline's end), then the job's own
 * (`PLAN_MISMATCH`, `WORKER_*`, …). A source that cannot be read is a note in the result, not an error.
 */
export function listen(deps: MediaRpcDeps, owner: JobOwner, req: ListenRequest): Effect.Effect<unknown, FinError> {
  return Effect.gen(function* () {
    yield* deps.registry.ensure(req.projectId);
    const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
    const snap = yield* deps.registry.snapshot(req.projectId, branch);
    const timelines = ((snap.project as { timelines?: ReadonlyArray<TimelineJson> } | null)?.timelines ?? []) as ReadonlyArray<TimelineJson>;
    const tl = req.timeline === undefined ? timelines[0] : timelines.find((t) => t.id === req.timeline);
    if (tl === undefined) {
      const what = req.timeline ?? "(first)";
      return yield* Effect.fail(finError("NOT_FOUND", `timeline ${what} does not exist`, { id: what, kind: "timeline", known: timelines.map((t) => t.id) }, "use the id of an existing timeline"));
    }
    const end = extent(tl);
    const time = (field: "from" | "to", s: string | undefined, dflt: Rat): Effect.Effect<Rat, FinError> => {
      if (s === undefined) return Effect.succeed(dflt);
      const t = rational(s);
      return t === undefined
        ? Effect.fail(finError("INVALID_TIME", `${field} ${JSON.stringify(s)} is not a rational number of seconds`, { field, value: s }, 'send seconds as "12" or "1001/30000"'))
        : Effect.succeed(t);
    };
    const from = yield* time("from", req.from, ZERO);
    const to = yield* time("to", req.to, end);
    const range = { from: show(from), to: show(to), timeline: tl.id, duration: show(end) };
    if (from.n < 0n || !le(to, end)) {
      return yield* Effect.fail(rangeError(`the range ${range.from} to ${range.to} passes timeline ${tl.id} (0 to ${range.duration} seconds)`, range, `listen to a range within 0 to ${range.duration}`));
    }
    if (le(to, from)) {
      return yield* Effect.fail(
        rangeError(
          end.n === 0n ? `timeline ${tl.id} is empty: there is nothing to listen to` : `the range must end after it starts (${range.from} to ${range.to})`,
          range,
          end.n === 0n ? "put clips on the timeline first" : "pass a range whose end is later than its start",
        ),
      );
    }
    const params: ListenParams = { projectId: req.projectId, branch, rev: snap.rev, timeline: tl.id, from: range.from, to: range.to, planSamples: [] };
    const job = yield* deps.engine.submit(owner, { kind: MEDIA_LISTEN, params, project: req.projectId });
    if (req.wait === false) return { job };
    const [done] = yield* deps.engine.wait([job.id], { timeoutMs: LISTEN_WAIT_MS }).pipe(
      Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the listen keeps running: fin job wait <job>") : e)),
    );
    return yield* resultOf(done as JobView);
  });
}
