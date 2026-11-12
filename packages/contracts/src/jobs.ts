// The job engine's TS-owned records (spec §6.1, 5C-1 plan Task 7): the job row's view, its
// owner and progress, mapped files and helper specs, as Effect Schemas. Their TS types are
// `Schema.Type` of them. The envelope, the params of each kind, `JobDone` and `WorkerExit`
// are Rust types (`@finisher/core-types`).
import type { JobRights as GeneratedJobRights } from "@finisher/core-types";
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

/**
 * A plugin job's rights, invoker ∩ grants (sp8b Decision 8): fin-plugin-api's generated
 * `JobRights`. Only the host builds them (a plugin kind is never submitted by RPC), so the wire
 * carries them as given; `isJobRights` is the shape check the engine applies at submit.
 */
export const JobRightsSchema = Schema.Unknown;
export type JobRights = GeneratedJobRights;

const PLUGIN_SCOPE_NAMES: ReadonlySet<string> = new Set(["project.read", "project.write", "library.read", "jobs.submit"]);
const RIGHTS_KEYS: ReadonlySet<string> = new Set(["principal", "class", "scopes", "projects", "branch", "libraries", "grants", "rightsHash", "generation"]);
/** The authority's rights hash: 32 lower-case hex characters (sp8 spec §3.3, Decision 28). */
const RIGHTS_HASH = /^[0-9a-f]{32}$/;
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isReach = (v: unknown): boolean => v === "all" || (Array.isArray(v) && v.every(isStr));
/** fin-plugin-api's `Permission` tags; `net` must name its origins (the egress broker reads them). */
const PERMS: ReadonlySet<string> = new Set(["project:read", "project:write", "media:read", "media:file", "jobs:submit", "gpu", "net", "secrets", "fs", "hooks:observe", "hooks:modify"]);
const isGrant = (g: unknown): boolean => isObj(g) && isStr(g.perm) && PERMS.has(g.perm) && (g.perm !== "net" || (Array.isArray(g.origins) && g.origins.every(isStr)));

/**
 * Whether `v` has `JobRights`' shape: no unknown field, scopes from the four a plugin may hold,
 * known permissions (a `net` grant with its origins), a branch for an `agent` principal, and
 * the authority's `rightsHash` (32 hex) and `generation` (a non-negative integer). Whether the
 * hash matches the rights is the plugin family's check (`checkRights`).
 */
export function isJobRights(v: unknown): v is GeneratedJobRights {
  if (!isObj(v) || Object.keys(v).some((k) => !RIGHTS_KEYS.has(k))) return false;
  const p = v.principal;
  if (!isObj(p) || !isStr(p.kind) || !isStr(p.id) || Object.keys(p).length !== 2) return false;
  if (!isStr(v.class) || !Array.isArray(v.scopes) || !v.scopes.every((s) => isStr(s) && PLUGIN_SCOPE_NAMES.has(s))) return false;
  if (!isReach(v.projects) || !isReach(v.libraries)) return false;
  const b = v.branch;
  if (b !== undefined && b !== null && !(isObj(b) && isStr(b.root) && typeof b.subtree === "boolean" && Object.keys(b).length === 2)) return false;
  // An agent session's rights always carry its branch reach: without it reads would be unbounded.
  if (p.kind === "agent" && !isObj(b)) return false;
  // The authority the job runs under: its rights hash and the class's policy generation.
  if (!isStr(v.rightsHash) || !RIGHTS_HASH.test(v.rightsHash)) return false;
  if (typeof v.generation !== "number" || !Number.isSafeInteger(v.generation) || v.generation < 0) return false;
  return Array.isArray(v.grants) && v.grants.every(isGrant);
}

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
  PLUGIN_OUTPUT_INVALID: 4,
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
