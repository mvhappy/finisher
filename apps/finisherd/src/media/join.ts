// A caller's request for a cache entry the server's own background job is making joins that job
// (6C Tasks 18–19; the e2e `listen` scenario decoded one WAV twice). The engine dedupes per owner
// (5C-2 Decision 7, the authorization boundary, kept as it is), so a request that arrives while a
// first-use job of `MEDIA_SYSTEM_OWNER` is queued or running would otherwise submit a second job
// of the same key. The media services note their system jobs per key here and join them:
// - **Queued**: taken over. It runs at `background`, which the scheduler runs after every
//   `normal` job (and the engine cannot raise a job's priority), so a caller joining it would wait
//   behind the whole first-use queue. It is failed at once (`failQueued`, which leaves a job that
//   started meanwhile alone: that one is joined) and the caller submits its own job at `normal`;
//   what followed the server's job follows the caller's (`superseded`).
// - **Running or paused**: the request waits for that job (`wait: false` answers `making`; the
//   answer names no job id: the job is the server's, which the caller cannot see).
// - **Succeeded** (it ended between the cache lookup and the join) or once the wait ends so: the
//   entry is looked up again and, when it is there, the answer is made from it.
// - **Failed, cancelled, gone, or its entry not there**: the caller submits its own job, as it
//   would with no background job (a failed feed answers the caller's job's error).
import type { FinError, JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";

const MAKING: ReadonlySet<string> = new Set(["queued", "running", "paused"]);

/** The error a taken-over job ends with (not `POISON_ITEM`, nor a content failure: nothing is remembered). */
const TAKEN_OVER = finError("JOB_CANCELLED", "a caller's request took the job over at normal priority", { reason: "takenOver" });

export interface JoinOptions {
  /** The request waits for the job (false: a job still making the entry answers `{ making: <its id> }`). */
  readonly wait: boolean;
  readonly timeoutMs: number;
  /** The `fix` of a `WAIT_TIMEOUT`. */
  readonly fix: string;
  /** Whether the entry is in the cache now (and served for the caller's project). */
  readonly inCache: Effect.Effect<boolean, FinError>;
}

/** The server's job `making` is still making the entry. */
export interface Making {
  readonly making: string;
}

/** The server's job was queued and is taken over: the caller submits its own (`superseded` was its id). */
export interface Superseded {
  readonly superseded: string;
}

export type Joined = JobView | Making | Superseded | undefined;

export const isMaking = (j: Joined): j is Making => j !== undefined && "making" in j;
export const isSuperseded = (j: Joined): j is Superseded => j !== undefined && "superseded" in j;
/** The succeeded view of a joined job, its entry in the cache. */
export const joinedView = (j: Joined): JobView | undefined => (j === undefined || isMaking(j) || isSuperseded(j) ? undefined : j);

export interface SystemJobs {
  /** The server's own job `job` is making the entry of `key`. */
  readonly note: (key: string, job: string) => void;
  /**
   * Joins the server's job of `key`: its succeeded view once the entry is in the cache,
   * `{ making }` (`wait: false`), `{ superseded }` when it was queued and is taken over, or
   * `undefined` when there is none to join (in both of the last, the caller submits its own).
   */
  readonly join: (key: string, opts: JoinOptions) => Effect.Effect<Joined, FinError>;
}

export function systemJobs(engine: Pick<JobEngine, "get" | "wait" | "failQueued">): SystemJobs {
  const byKey = new Map<string, string>();
  const forget = (key: string, id: string): void => {
    if (byKey.get(key) === id) byKey.delete(key);
  };

  const join = (key: string, opts: JoinOptions): Effect.Effect<Joined, FinError> =>
    Effect.gen(function* () {
      const id = byKey.get(key);
      if (id === undefined) return undefined;
      const got = yield* Effect.result(engine.get(id));
      let view = got._tag === "Success" ? got.success : undefined;
      if (view?.state === "queued") {
        const taken = yield* Effect.result(engine.failQueued(id, TAKEN_OVER));
        view = taken._tag === "Success" ? taken.success : undefined;
        if (view === undefined || view.state === "failed" || view.state === "cancelled") {
          forget(key, id);
          return { superseded: id };
        }
      }
      if (view !== undefined && MAKING.has(view.state)) {
        if (!opts.wait) return { making: id };
        // A job that ends between the lookup and the wait is answered at once by the wait.
        const [done] = yield* engine.wait([id], { timeoutMs: opts.timeoutMs }).pipe(
          Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { timeoutMs: opts.timeoutMs }, opts.fix) : e)),
        );
        view = done;
      }
      forget(key, id);
      if (view === undefined || view.state !== "succeeded") return undefined;
      return (yield* opts.inCache) ? view : undefined;
    });

  return { note: (key, job) => void byKey.set(key, job), join };
}
