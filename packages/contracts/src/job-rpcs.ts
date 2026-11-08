// The `jobs.*` RPCs' payloads and answers (5C-2 plan Decision 1, spec §6.7). Ownership and reach
// are the server's: a job another owner holds (without `jobs.control`) or outside the caller's
// reach answers `JOB_NOT_FOUND`, exactly as an unknown id.
import { Schema } from "effect";
import { ChallengeField } from "./auth.ts";
import { JobStateSchema, JobViewSchema, PrioritySchema } from "./jobs.ts";

/** At most this many jobs per `jobs.list` page (the store's cap). */
export const JOB_LIST_MAX = 500;
/** At most this many ids in one `jobs.wait` or `jobs.watch`. */
export const JOB_IDS_MAX = 100;
/** The longest `jobs.wait`, and its default. */
export const JOB_WAIT_MAX_MS = 600_000;
export const JOB_WAIT_DEFAULT_MS = 60_000;
/** Events a `jobs.watch` holds for a reader that falls behind; past it the stream ends `WATCH_LAGGING`. */
export const WATCH_BUFFER = 256;
/** Open `jobs.wait` and `jobs.watch` calls one principal may hold at once; past it `TOO_MANY_WATCHES`. */
export const JOB_CALLS_MAX = 16;
/** The largest `jobs.submit` params, serialised (bytes of JSON). */
export const JOB_PARAMS_MAX_BYTES = 256 * 1024;
/** The longest `jobs.submit` `idemKey`, in characters. */
export const IDEM_KEY_MAX = 128;
/** At most this many states in one `jobs.list` filter (there are fewer distinct ones). */
export const JOB_STATES_MAX = 8;
/** Unfinished jobs one owner other than `local`, `console`, a device or `system` may hold (the engine's default). */
export const JOB_OWNER_LIMIT = 1000;

/**
 * `jobs.submit` (`jobs.submit` and the kind's own scopes): only kinds that opt in to RPC
 * submission (`KindSpec.rpc`); any other is `JOB_KIND_UNKNOWN`. `project` must be in the caller's
 * reach (`PROJECT_NOT_FOUND`), and a caller limited to some projects must name one
 * (`INVALID_REQUEST`). `params` at most `JOB_PARAMS_MAX_BYTES` serialised, `idemKey` at most
 * `IDEM_KEY_MAX` characters; `priority: interactive` is a person's only (anyone else gets the
 * kind's default). `challenge`: the console's typed code, needed only for a kind whose scopes
 * include a human-only one.
 */
export const JobsSubmitPayload = Schema.Struct({
  kind: Schema.String,
  params: Schema.Unknown,
  priority: Schema.optional(PrioritySchema),
  idemKey: Schema.optional(Schema.String),
  project: Schema.optional(Schema.String),
  challenge: ChallengeField,
});

export const JobIdPayload = Schema.Struct({ job: Schema.String });

/**
 * `jobs.list`: newest first. `mine`: only the caller's own (a `jobs.control` principal sees every
 * owner's otherwise). `project` must be in reach. `cursor`: a page's `next`.
 */
export const JobsListPayload = Schema.Struct({
  state: Schema.optional(Schema.Array(JobStateSchema)),
  kind: Schema.optional(Schema.String),
  project: Schema.optional(Schema.String),
  mine: Schema.optional(Schema.Boolean),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
});
export const JobsListSuccess = Schema.Struct({ jobs: Schema.Array(JobViewSchema), next: Schema.optional(Schema.String) });

/**
 * `jobs.wait`: until all (`any`: one) of `ids` finished, else `WAIT_TIMEOUT`. With `jobs.watch`,
 * at most `JOB_CALLS_MAX` open per principal (`TOO_MANY_WATCHES`).
 */
export const JobsWaitPayload = Schema.Struct({
  ids: Schema.Array(Schema.String),
  any: Schema.optional(Schema.Boolean),
  timeoutMs: Schema.optional(Schema.Number),
});
export const JobsListOfViews = Schema.Struct({ jobs: Schema.Array(JobViewSchema) });

/** `jobs.watch`: `ids` (1 to `JOB_IDS_MAX`) first sends each job's current view, then only their events. */
export const JobsWatchPayload = Schema.Struct({ ids: Schema.optional(Schema.Array(Schema.String)) });

/** One `jobs.watch` item: a job's new state, its progress, or one of its items. */
export const JobWatchEventSchema = Schema.Union([
  Schema.Struct({ type: Schema.Literals(["state", "progress"]), job: JobViewSchema }),
  Schema.Struct({
    type: Schema.Literal("item"),
    job: JobViewSchema,
    item: Schema.String,
    error: Schema.optional(Schema.Struct({ code: Schema.String })),
  }),
]);
export type JobWatchEvent = typeof JobWatchEventSchema.Type;

/**
 * `jobs.pause` / `jobs.resume`: one job (`jobs.submit` for one's own, `jobs.control` for
 * another's) or every job (`jobs.control`, and a principal that reaches every project: else
 * `SCOPE_DENIED {reason: reach}`).
 */
export const JobsTargetPayload = Schema.Struct({
  job: Schema.optional(Schema.String),
  all: Schema.optional(Schema.Boolean),
});
/** The job's view after a pause or resume of one job; `{}` for `{all}`. */
export const JobsTargetSuccess = Schema.Struct({ job: Schema.optional(JobViewSchema) });

/** `jobs.workers` (`jobs.control`, and every project in reach): pools, warm workers, live processes, stuck pools, the host's caps. */
export const JobsWorkersSuccess = Schema.Unknown;
