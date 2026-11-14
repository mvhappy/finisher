// `media.look` (6A plan Task 13): the RPC that submits a `media.look` job for a timeline at some
// times and, with `wait`, answers its `LookResult` once the frames are in their final place.
//
// The handler reads the branch head once (`rev`, the timelines, their extents), resolves the
// timeline to its id, refuses times outside `[0, extent]` (`INVALID_RANGE`: `eval_frame` past
// the end answers an empty plan, which is not an error there), and submits the job with the
// head it read, so equal looks of one head coalesce (`lookDedupe`). Everything else (the plan
// hashes, the inputs, the render, moving the PNGs) is the kind's (`jobs/kinds/media.look.ts`).
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { LookParams } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_LOOK } from "../jobs/kinds/media.look.ts";
import type { ProjectRegistryShape } from "../projects.ts";

/** At most this many times in one look (`LOOK_MAX_TIMES`). */
export const LOOK_MAX_TIMES = 64;
/** The longest side of a frame (`LOOK_MAX_SIDE`). */
export const LOOK_MAX_SIDE = 8192;
/** How long `wait` waits for the job. */
export const LOOK_WAIT_MS = 60_000;

export interface MediaRpcDeps {
  readonly registry: Pick<ProjectRegistryShape, "ensure" | "snapshot" | "resolveBranch">;
  readonly engine: Pick<JobEngine, "submit" | "wait">;
}

export interface LookRequest {
  readonly projectId: string;
  readonly branch?: string | undefined;
  readonly timeline?: string | undefined;
  readonly at: ReadonlyArray<string>;
  readonly out?: string | undefined;
  readonly size?: { readonly width: number; readonly height: number } | undefined;
  readonly wait?: boolean | undefined;
  /** Read originals only (`fin look --originals`): a layer never reads a proxy. */
  readonly originals?: boolean | undefined;
}

interface Rat {
  readonly n: bigint;
  readonly d: bigint;
}

/** `"n"` or `"n/d"` (any spelling: it is sent on in lowest terms), else `undefined`. */
function rational(s: string): Rat | undefined {
  const m = /^(-?\d{1,38})(?:\/(\d{1,38}))?$/.exec(s);
  if (m === null) return undefined;
  const d = m[2] === undefined ? 1n : BigInt(m[2]);
  return d === 0n ? undefined : { n: BigInt(m[1] as string), d };
}

/** `a ≤ b`, both denominators positive. */
const le = (a: Rat, b: Rat): boolean => a.n * b.d <= b.n * a.d;

interface Item {
  readonly start: string;
  readonly duration: string;
}
interface TimelineJson {
  readonly id: string;
  readonly tracks?: ReadonlyArray<{ readonly items?: ReadonlyArray<Item> }>;
}

/** The end of the last item of `tl` (0 for an empty timeline). */
function extent(tl: TimelineJson): Rat {
  let end: Rat = { n: 0n, d: 1n };
  for (const track of tl.tracks ?? []) {
    for (const item of track.items ?? []) {
      const s = rational(item.start);
      const d = rational(item.duration);
      if (s === undefined || d === undefined) continue;
      const e: Rat = { n: s.n * d.d + d.n * s.d, d: s.d * d.d };
      if (le(end, e)) end = e;
    }
  }
  return end;
}

const gcd = (a: bigint, b: bigint): bigint => {
  let [x, y] = [a < 0n ? -a : a, b];
  while (y !== 0n) [x, y] = [y, x % y];
  return x;
};

/** `r` in lowest terms, as `fin-core` spells a rational: `n`, or `n/d` with `d > 1`. */
const show = (r: Rat): string => {
  const g = gcd(r.n, r.d) || 1n;
  const [n, d] = [r.n / g, r.d / g];
  return d === 1n ? `${n}` : `${n}/${d}`;
};

const rangeError = (field: string, message: string, details: Record<string, unknown>, fix: string): FinError => finError("INVALID_RANGE", message, { field, ...details }, fix);

/** The checks that need no project. */
function checkRequest(r: LookRequest): FinError | undefined {
  if (r.at.length < 1 || r.at.length > LOOK_MAX_TIMES) {
    return rangeError("at", `a look takes 1 to ${LOOK_MAX_TIMES} times, not ${r.at.length}`, { count: r.at.length, min: 1, max: LOOK_MAX_TIMES }, "send fewer times, or look in several calls");
  }
  if (r.size !== undefined) {
    const { width, height } = r.size;
    for (const [name, v] of [["width", width], ["height", height]] as const) {
      if (!Number.isInteger(v) || v < 1 || v > LOOK_MAX_SIDE) {
        return rangeError("size", `size.${name} ${v} is outside 1 to ${LOOK_MAX_SIDE}`, { [name]: v, min: 1, max: LOOK_MAX_SIDE }, `each side of a look is 1 to ${LOOK_MAX_SIDE} pixels`);
      }
    }
  }
  return undefined;
}

/** A finished job's result, or its error. */
function resultOf(job: JobView): Effect.Effect<unknown, FinError> {
  if (job.state === "succeeded") return Effect.succeed(job.result);
  const e = job.error;
  return Effect.fail(
    e === undefined
      ? finError("JOB_FAILED", `the look ended ${job.state}`, { job: job.id, state: job.state })
      : finError(e.code, e.message, e.details, e.fix),
  );
}

/**
 * `media.look`. Errors: `PROJECT_NOT_FOUND`, `BRANCH_NOT_FOUND`, `NOT_FOUND` (the timeline),
 * `INVALID_RANGE` (a time outside the timeline, too many times, a side out of range),
 * `INVALID_TIME` (a time that is not a rational), `EXPORT_DESTINATION`, then the job's own
 * (`CODEC_UNAVAILABLE` is not one: it travels in `skipped`; `GPU_UNAVAILABLE`, `WORKER_*`, …).
 */
export function look(deps: MediaRpcDeps, owner: JobOwner, req: LookRequest): Effect.Effect<unknown, FinError> {
  return Effect.gen(function* () {
    const bad = checkRequest(req);
    if (bad !== undefined) return yield* Effect.fail(bad);
    yield* deps.registry.ensure(req.projectId);
    const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
    const snap = yield* deps.registry.snapshot(req.projectId, branch);
    const timelines = ((snap.project as { timelines?: ReadonlyArray<TimelineJson> } | null)?.timelines ?? []) as ReadonlyArray<TimelineJson>;
    const tl = req.timeline === undefined ? timelines[0] : timelines.find((t) => t.id === req.timeline);
    if (tl === undefined) {
      const what = req.timeline ?? "(first)";
      return yield* Effect.fail(
        finError("NOT_FOUND", `timeline ${what} does not exist`, { id: what, kind: "timeline", known: timelines.map((t) => t.id) }, "use the id of an existing timeline"),
      );
    }
    const end = extent(tl);
    const times: string[] = [];
    for (const at of req.at) {
      const t = rational(at);
      if (t === undefined) {
        return yield* Effect.fail(
          finError("INVALID_TIME", `at ${JSON.stringify(at)} is not a rational number of seconds`, { field: "at", value: at }, 'send seconds as "12" or "1001/30000"'),
        );
      }
      if (t.n < 0n || !le(t, end)) {
        return yield* Effect.fail(
          rangeError("at", `time ${show(t)} is outside timeline ${tl.id} (0 to ${show(end)} seconds)`, { at: show(t), timeline: tl.id, duration: show(end) }, `look at a time from 0 to ${show(end)}`),
        );
      }
      times.push(show(t));
    }
    const params: LookParams = {
      projectId: req.projectId,
      branch,
      rev: snap.rev,
      timeline: tl.id,
      at: times,
      outName: "look",
      planSamples: [],
      ...(req.size === undefined ? {} : { size: { width: req.size.width, height: req.size.height } }),
      ...(req.out === undefined ? {} : { outDir: req.out }),
      ...(req.originals === true ? { originals: true } : {}),
    };
    const job = yield* deps.engine.submit(owner, { kind: MEDIA_LOOK, params, project: req.projectId });
    if (req.wait === false) return { job };
    const [done] = yield* deps.engine.wait([job.id], { timeoutMs: LOOK_WAIT_MS }).pipe(
      Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the look keeps running: ask for its result with its job id") : e)),
    );
    return yield* resultOf(done as JobView);
  });
}
