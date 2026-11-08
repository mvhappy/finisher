// The `jobs.*` handlers (5C-2 plan, spec §6.7). The chain has checked the scopes (`RPC_SCOPES`:
// `jobs.submit` with the kind's own, `jobs.read`, and by ownership `jobs.submit` or
// `jobs.control`) and the reach of a `project` the call names; these handlers check, per job,
// that the caller sees it (`access.ts`), and answer a job it does not see as an unknown one.
// Bounds (5C-2 review): `JOB_CALLS_MAX` open `jobs.wait`/`jobs.watch` calls per principal
// (`TOO_MANY_WATCHES`); a submit's params and `idemKey` sizes; `interactive` priority for people only.
import {
  type FinError,
  IDEM_KEY_MAX,
  JOB_CALLS_MAX,
  JOB_IDS_MAX,
  JOB_LIST_MAX,
  JOB_PARAMS_MAX_BYTES,
  JOB_STATES_MAX,
  JOB_WAIT_DEFAULT_MS,
  JOB_WAIT_MAX_MS,
  type JobState,
  type JobView,
  type Principal,
  type Priority,
  type Scope,
} from "@finisher/contracts";
import { Effect, Exit, type Scope as EffectScope, Stream } from "effect";
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
  /** Open waits and watches per principal (default: `JOB_CALLS_MAX` each). */
  readonly slots?: CallSlots;
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

/** Open `jobs.wait` and `jobs.watch` calls per principal (`kind:id`), at most `max` each (review M1). */
export class CallSlots {
  readonly #open = new Map<string, number>();
  readonly #max: number;

  constructor(max: number = JOB_CALLS_MAX) {
    this.#max = max;
  }

  /** One slot of `p`'s, given back when the scope closes; `TOO_MANY_WATCHES` when it has none left. */
  hold(p: Principal): Effect.Effect<void, FinError, EffectScope.Scope> {
    const key = `${p.kind}:${p.id}`;
    const take = Effect.suspend(() => {
      const n = this.#open.get(key) ?? 0;
      if (n >= this.#max) {
        return Effect.fail(
          finError("TOO_MANY_WATCHES", `this ${p.kind} already has ${this.#max} jobs.wait and jobs.watch calls open`, { limit: this.#max }, "end one of them, then retry"),
        );
      }
      this.#open.set(key, n + 1);
      return Effect.void;
    });
    const give = Effect.sync(() => {
      const n = (this.#open.get(key) ?? 1) - 1;
      if (n > 0) this.#open.set(key, n);
      else this.#open.delete(key);
    });
    return Effect.acquireRelease(take, () => give);
  }

  /** Open calls of `p` (tests). */
  open(p: Principal): number {
    return this.#open.get(`${p.kind}:${p.id}`) ?? 0;
  }
}

/** Why a submit's own fields are refused (review M3–M4), or `undefined`. */
function submitRefusal(p: Principal, req: { readonly params: unknown; readonly idemKey?: string | undefined; readonly project?: string | undefined }): FinError | undefined {
  // A job without a project is in every principal's reach: one limited to some names one (M3).
  if (p.projects !== "*" && req.project === undefined) {
    return invalid("project", "a credential limited to some projects names the job's project", { tag: "jobs.submit" });
  }
  if (req.idemKey !== undefined && req.idemKey.length > IDEM_KEY_MAX) {
    return invalid("idemKey", `idemKey is at most ${IDEM_KEY_MAX} characters, not ${req.idemKey.length}`, { max: IDEM_KEY_MAX });
  }
  const bytes = Buffer.byteLength(JSON.stringify(req.params ?? null) ?? "null");
  if (bytes > JOB_PARAMS_MAX_BYTES) {
    const errors = [`params: at most ${JOB_PARAMS_MAX_BYTES} bytes as JSON, not ${bytes}`];
    return finError("JOB_PARAMS_INVALID", `the job's ${errors[0] as string}`, { errors, max: JOB_PARAMS_MAX_BYTES });
  }
  return undefined;
}

/** `{job} | {all: true}`, exactly one. */
function target(req: { readonly job?: string | undefined; readonly all?: boolean | undefined }, tag: string) {
  const all = req.all === true;
  if (all === (req.job !== undefined)) {
    return Effect.fail(invalid("job", `${tag} takes either a job or all: true`, { tag }));
  }
  return Effect.succeed(all ? ({ all: true } as const) : { job: req.job as string });
}

export function jobHandlers({ engine, guard, slots = new CallSlots() }: JobRpcDeps) {
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
          const p = principalOf(options);
          const refused = submitRefusal(p, req);
          if (refused !== undefined) return Effect.fail(refused);
          // `interactive` jumps the queue: a person's only; anyone else gets the kind's default (M4).
          const priority = req.priority === "interactive" && !p.human ? undefined : req.priority;
          return engine.submit(ownerOf(p), {
            kind: req.kind,
            params: req.params,
            ...(priority === undefined ? {} : { priority }),
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
          // A state filter is a short set (review L3): deduped, at most `JOB_STATES_MAX` given.
          if (req.state !== undefined && req.state.length > JOB_STATES_MAX) {
            return Effect.fail(invalid("state", `state lists at most ${JOB_STATES_MAX} states, not ${req.state.length}`, { max: JOB_STATES_MAX }));
          }
          const filter = req.state === undefined ? req : { ...req, state: [...new Set(req.state)] };
          return Effect.try({ try: () => engine.list(listQuery(principalOf(options), filter)), catch: toFinError });
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
          const wait = Effect.andThen(slots.hold(p), engine.wait(req.ids, { timeoutMs: t, ...(req.any === true ? { any: true } : {}) }));
          const jobs = yield* Effect.scoped(wait);
          return { jobs: [...jobs] };
        }),
      ),
    "jobs.watch": (req: { ids?: ReadonlyArray<string> | undefined }, options: object) => {
      const p = principalOf(options);
      // The slot is held for the stream's life and given back when it ends, however it ends.
      return Stream.unwrap(Effect.as(slots.hold(p), watchJobs(engine, (id) => viewOf(engine, id), p, req))).pipe(Stream.catchDefect((d) => Stream.fail(toFinError(d))));
    },
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
