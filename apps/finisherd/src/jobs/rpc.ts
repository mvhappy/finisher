// The `jobs.*` handlers (5C-2 plan, spec §6.7). The chain has checked the scopes (`RPC_SCOPES`:
// `jobs.submit` with the kind's own, `jobs.read`, and by ownership `jobs.submit` or
// `jobs.control`) and the reach of a `project` the call names; these handlers check, per job,
// that the caller sees it (`access.ts`), and answer a job it does not see as an unknown one.
import {
  type FinError,
  JOB_IDS_MAX,
  JOB_LIST_MAX,
  JOB_WAIT_DEFAULT_MS,
  JOB_WAIT_MAX_MS,
  type JobState,
  type JobView,
  type Principal,
  type Priority,
  type Scope,
} from "@finisher/contracts";
import { Effect, Exit, Stream } from "effect";
import { chainOf } from "../chain/chain.ts";
import type { ChainJobs } from "../chain/resolve.ts";
import { finError, toFinError } from "../errors.ts";
import { ANONYMOUS } from "../principal.ts";
import { canSee, jobNotFound, listQuery, ownerOf, relationOf } from "./access.ts";
import type { JobEngine } from "./api.ts";
import { watchJobs } from "./watch.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface JobRpcDeps {
  readonly engine: JobEngine;
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
}

/** The chain's principal of a call (`anonymous` outside it: fail closed, it sees nothing). */
const principalOf = (options: object): Principal => chainOf(options)?.principal ?? ANONYMOUS;

const invalid = (field: string, message: string, details: Record<string, unknown> = {}): FinError =>
  finError("INVALID_REQUEST", message, { field, ...details });

/** A job's current view, read synchronously; `undefined` when unknown. */
function viewOf(engine: JobEngine, id: string): JobView | undefined {
  const exit = Effect.runSyncExit(engine.get(id));
  return Exit.isSuccess(exit) ? exit.value : undefined;
}

/** The job, when `p` sees it; else `JOB_NOT_FOUND` as for an unknown id. */
function visible(engine: JobEngine, p: Principal, id: string): Effect.Effect<JobView, FinError> {
  return Effect.suspend(() => {
    const v = viewOf(engine, id);
    return v !== undefined && canSee(p, v) ? Effect.succeed(v) : Effect.fail(jobNotFound(id));
  });
}

/** What the chain's `job` and `jobKind` resolvers read (5C-2 Decision 4). */
export function chainJobs(engine: JobEngine): ChainJobs {
  return {
    relation: (p, id) => relationOf(p, viewOf(engine, id)),
    kindScopes: (kind) => {
      const k = engine.kinds.kind(kind);
      // A kind's scopes are names of `SCOPES`; one that is not is held by no principal (fail closed).
      return k?.rpc === true ? (k.scopes as ReadonlyArray<Scope>) : undefined;
    },
  };
}

/** `{job} | {all: true}`, exactly one. */
function target(req: { readonly job?: string | undefined; readonly all?: boolean | undefined }, tag: string) {
  const all = req.all === true;
  if (all === (req.job !== undefined)) {
    return Effect.fail(invalid("job", `${tag} takes either a job or all: true`, { tag }));
  }
  return Effect.succeed(all ? ({ all: true } as const) : { job: req.job as string });
}

export function jobHandlers({ engine, guard }: JobRpcDeps) {
  return {
    "jobs.submit": (
      req: { kind: string; params: unknown; priority?: Priority | undefined; idemKey?: string | undefined; project?: string | undefined },
      options: object,
    ) =>
      guard(
        "jobs.submit",
        Effect.suspend(() => {
          const kind = engine.kinds.kind(req.kind);
          // A kind clients may not submit answers as an unknown one (Decision 6).
          if (kind?.rpc !== true) return Effect.fail(finError("JOB_KIND_UNKNOWN", `no job kind ${req.kind}`, { kind: req.kind }));
          return engine.submit(ownerOf(principalOf(options)), {
            kind: req.kind,
            params: req.params,
            ...(req.priority === undefined ? {} : { priority: req.priority }),
            ...(req.idemKey === undefined ? {} : { idemKey: req.idemKey }),
            ...(req.project === undefined ? {} : { project: req.project }),
          });
        }),
      ),
    "jobs.get": ({ job }: { job: string }, options: object) => guard("jobs.get", visible(engine, principalOf(options), job)),
    "jobs.list": (
      req: {
        state?: ReadonlyArray<JobState> | undefined;
        kind?: string | undefined;
        project?: string | undefined;
        mine?: boolean | undefined;
        cursor?: string | undefined;
        limit?: number | undefined;
      },
      options: object,
    ) =>
      guard(
        "jobs.list",
        Effect.suspend(() => {
          const l = req.limit;
          if (l !== undefined && !(Number.isInteger(l) && l >= 1 && l <= JOB_LIST_MAX)) {
            return Effect.fail(invalid("limit", `limit is 1 to ${JOB_LIST_MAX}, not ${l}`, { max: JOB_LIST_MAX }));
          }
          return Effect.try({ try: () => engine.list(listQuery(principalOf(options), req)), catch: toFinError });
        }),
      ),
    "jobs.wait": (req: { ids: ReadonlyArray<string>; any?: boolean | undefined; timeoutMs?: number | undefined }, options: object) =>
      guard(
        "jobs.wait",
        Effect.gen(function* () {
          if (req.ids.length < 1 || req.ids.length > JOB_IDS_MAX) {
            return yield* Effect.fail(invalid("ids", `jobs.wait takes 1 to ${JOB_IDS_MAX} ids, not ${req.ids.length}`, { max: JOB_IDS_MAX }));
          }
          const t = req.timeoutMs ?? JOB_WAIT_DEFAULT_MS;
          if (!(Number.isInteger(t) && t >= 1 && t <= JOB_WAIT_MAX_MS)) {
            return yield* Effect.fail(invalid("timeoutMs", `timeoutMs is 1 to ${JOB_WAIT_MAX_MS}, not ${t}`, { max: JOB_WAIT_MAX_MS }));
          }
          const p = principalOf(options);
          for (const id of req.ids) yield* visible(engine, p, id);
          const jobs = yield* engine.wait(req.ids, { timeoutMs: t, ...(req.any === true ? { any: true } : {}) });
          return { jobs: [...jobs] };
        }),
      ),
    "jobs.watch": (req: { ids?: ReadonlyArray<string> | undefined }, options: object) =>
      watchJobs(engine, (id) => viewOf(engine, id), principalOf(options), req).pipe(Stream.catchDefect((d) => Stream.fail(toFinError(d)))),
    "jobs.cancel": ({ job }: { job: string }, options: object) =>
      guard("jobs.cancel", Effect.flatMap(visible(engine, principalOf(options), job), () => engine.cancel(job))),
    "jobs.pause": (req: { job?: string | undefined; all?: boolean | undefined }, options: object) =>
      guard(
        "jobs.pause",
        Effect.gen(function* () {
          const t = yield* target(req, "jobs.pause");
          if ("all" in t) return yield* Effect.as(engine.pause(t), {});
          yield* visible(engine, principalOf(options), t.job);
          yield* engine.pause(t);
          return { job: yield* engine.get(t.job) };
        }),
      ),
    "jobs.resume": (req: { job?: string | undefined; all?: boolean | undefined }, options: object) =>
      guard(
        "jobs.resume",
        Effect.gen(function* () {
          const t = yield* target(req, "jobs.resume");
          if ("all" in t) return yield* Effect.as(engine.resume(t), {});
          yield* visible(engine, principalOf(options), t.job);
          yield* engine.resume(t);
          return { job: yield* engine.get(t.job) };
        }),
      ),
    "jobs.workers": () => guard("jobs.workers", Effect.sync(() => engine.workers())),
  };
}
