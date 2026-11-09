// The job engine's TS-owned records (spec §6.1, 5C-1 plan Task 7): the job row's view, its
// owner and progress, mapped files and helper specs, as Effect Schemas. Their TS types are
// `Schema.Type` of them. The envelope, the params of each kind, `JobDone` and `WorkerExit`
// are Rust types (`@finisher/core-types`).
import { Schema } from "effect";

export const PRIORITIES = ["interactive", "normal", "background"] as const;
export const PrioritySchema = Schema.Literals(PRIORITIES);
export type Priority = typeof PrioritySchema.Type;

export const JOB_STATES = ["queued", "running", "paused", "succeeded", "failed", "cancelled", "interrupted"] as const;
export const JobStateSchema = Schema.Literals(JOB_STATES);
export type JobState = typeof JobStateSchema.Type;

/** The states a job ends in (`finishedAt` set). An `interrupted` job may be re-queued at restart. */
export const TERMINAL_JOB_STATES: ReadonlyArray<JobState> = ["succeeded", "failed", "cancelled", "interrupted"];

/**
 * A job's owner: the submitting principal's `{kind, id}` (`local` runs share `local`; each
 * credential, device and console is its own) or `system` for callers inside `finisherd`.
 */
export const JobOwnerSchema = Schema.Struct({ kind: Schema.String, id: Schema.String });
export type JobOwner = typeof JobOwnerSchema.Type;

/** The plugin a job was submitted through (sub-project 8). */
export const JobViaSchema = Schema.Struct({ plugin: Schema.String });
export type JobVia = typeof JobViaSchema.Type;

/** The invoker ∩ grants snapshot; opaque until 8B. */
export const JobRightsSchema = Schema.Unknown;
export type JobRights = typeof JobRightsSchema.Type;

export const ProgressUnitSchema = Schema.Literals(["bytes", "items", "frames", "seconds"]);

/** A job's latest progress (in memory only, spec §6.5). */
export const ProgressSchema = Schema.Struct({
  done: Schema.Number,
  total: Schema.optionalKey(Schema.Number),
  unit: ProgressUnitSchema,
  phase: Schema.optionalKey(Schema.String),
  at: Schema.Number,
});
export type Progress = typeof ProgressSchema.Type;
/** The same as `Progress` (whose name the envelope's `Progress` also has). */
export type JobProgress = Progress;

export const MappedFileModeSchema = Schema.Literals(["read", "write", "readWrite"]);

/** A file (or, with a trailing `/`, a directory) 5C creates in the job directory (spec §6.4). */
export const MappedFileSchema = Schema.Struct({
  name: Schema.String,
  bytes: Schema.optionalKey(Schema.Number),
  mode: MappedFileModeSchema,
});
export type MappedFile = typeof MappedFileSchema.Type;

/** A helper process started beside the job's worker (spec §6.4). `files` names the mapped files it sees. */
export const HelperSpecSchema = Schema.Struct({
  kind: Schema.Literals(["codec", "plugin"]),
  id: Schema.String,
  profile: Schema.Literals(["worker", "plugin"]),
  files: Schema.Array(Schema.String),
});
export type HelperSpec = typeof HelperSpecSchema.Type;

/**
 * An error as stored and shown: `{code, message, details, fix?}`, a `FinError`'s data.
 *
 * Two forms on purpose. `FinError` (errors.ts) is the Effect error class: it is what code
 * fails with and catches (`Effect.fail(new FinError(…))`, `onCall`, `prepare`, an item's
 * `ItemOutcome.error` handed to a kind's hooks). `FinErrorData` is the plain JSON of one, which
 * is what a job row stores and what `JobView.error` / `itemErrors` carry, so a view
 * round-trips through `control.db` and over the wire unchanged. A `FinError` is assignable to
 * `FinErrorData`; the store turns stored data back into `FinError`s only where an `Effect`
 * consumer expects one (`JobStore.items`).
 */
export const FinErrorDataSchema = Schema.Struct({
  code: Schema.String,
  message: Schema.String,
  details: Schema.Unknown,
  fix: Schema.optionalKey(Schema.String),
});
export type FinErrorData = typeof FinErrorDataSchema.Type;

export const JobItemErrorSchema = Schema.Struct({ item: Schema.String, error: FinErrorDataSchema });
export type JobItemError = typeof JobItemErrorSchema.Type;

/** A job as the engine and (5C-2) the RPCs show it. */
export const JobViewSchema = Schema.Struct({
  id: Schema.String,
  kind: Schema.String,
  kindVersion: Schema.Number,
  family: Schema.String,
  params: Schema.Unknown,
  owner: JobOwnerSchema,
  via: Schema.optionalKey(JobViaSchema),
  /** Kept for hooks, host calls and the bus (sub-project 8); checked by 8B. */
  rights: Schema.optionalKey(JobRightsSchema),
  project: Schema.optionalKey(Schema.String),
  library: Schema.optionalKey(Schema.String),
  parent: Schema.optionalKey(Schema.String),
  priority: PrioritySchema,
  state: JobStateSchema,
  attempts: Schema.Number,
  maxAttempts: Schema.Number,
  idemKey: Schema.optionalKey(Schema.String),
  dedupeKey: Schema.optionalKey(Schema.String),
  progress: Schema.optionalKey(ProgressSchema),
  result: Schema.optionalKey(Schema.Unknown),
  error: Schema.optionalKey(FinErrorDataSchema),
  itemErrors: Schema.optionalKey(Schema.Array(JobItemErrorSchema)),
  warm: Schema.Boolean,
  createdAt: Schema.Number,
  startedAt: Schema.optionalKey(Schema.Number),
  finishedAt: Schema.optionalKey(Schema.Number),
  nextRunAt: Schema.optionalKey(Schema.Number),
});
export type JobView = typeof JobViewSchema.Type;

/**
 * The job engine's error codes and their `fin` exit class (5C-1 plan, Decision 41; the classes
 * live in `crates/fin/src/output.rs`): 3 the request (`JOBHOST_STATE`: a job host call on a
 * group or host the caller should know is gone), 4 a job that did not succeed and the
 * result codes it carries, 5 unavailable for now (`WATCH_LAGGING`: a `jobs.watch` reader fell
 * `WATCH_BUFFER` events behind; list again, then watch again. `TOO_MANY_WATCHES`: the principal
 * already has `JOB_CALLS_MAX` `jobs.wait`/`jobs.watch` calls open; end one, then retry), 6 internal (`UNKNOWN_MESSAGE` and
 * `CALL_LIMIT` appear only in `reply` errors to a worker).
 */
export const JOB_ERROR_CODES = {
  INVALID_REQUEST: 3,
  JOB_KIND_UNKNOWN: 3,
  JOB_PARAMS_INVALID: 3,
  JOB_NOT_FOUND: 3,
  JOBHOST_STATE: 3,
  JOB_FAILED: 4,
  JOB_CANCELLED: 4,
  JOB_INTERRUPTED: 4,
  WORKER_CRASHED: 4,
  WORKER_HUNG: 4,
  JOB_STALLED: 4,
  JOB_TIMEOUT: 4,
  POISON_ITEM: 4,
  HELPER_FAILED: 4,
  PLUGIN_CRASHED: 4,
  PLUGIN_TIMEOUT: 4,
  PLUGIN_PROTOCOL: 4,
  JOB_QUEUE_FULL: 5,
  WORKER_UNAVAILABLE: 5,
  WORKER_PROTOCOL_MISMATCH: 5,
  WAIT_TIMEOUT: 5,
  RESOURCE_STUCK: 5,
  WORKER_OOM: 5,
  WATCH_LAGGING: 5,
  TOO_MANY_WATCHES: 5,
  UNKNOWN_MESSAGE: 6,
  CALL_LIMIT: 6,
} as const satisfies Record<string, 3 | 4 | 5 | 6>;
export type JobErrorCode = keyof typeof JOB_ERROR_CODES;
