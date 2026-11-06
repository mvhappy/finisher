// Branch and history RPC payloads and results (plan 4B Task 18). The result types are the
// generated ones (`@finisher/core-types`); the Effect schemas that validate them at runtime
// are `Schema.Unknown` narrowed to those types, except `BranchViewSchema` and `ConflictSchema`,
// which `envelopes.test.ts` checks against the generated schema.
import type {
  BranchCreated,
  BranchView,
  CommittedTx,
  Conflict,
  HistoryResult,
  MergeReport,
  RecoverReport,
} from "@finisher/core-types";
import { Schema } from "effect";

export type { BranchCreated, BranchView, Conflict, HistoryResult, MergeReport, RecoverGroup, RecoverReport } from "@finisher/core-types";

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;

export const BranchStateSchema = Schema.Union([
  Schema.Literal("active"),
  Schema.Literal("archived"),
  Schema.Literal("merged"),
]);

/** fin-store's `BranchView`; `envelopes.test.ts` checks it against the generated schema. */
export const BranchViewSchema = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  display: Schema.String,
  parent: Schema.optionalKey(Schema.NullOr(Schema.String)),
  forkSeq: Schema.Number,
  head: Schema.Number,
  state: BranchStateSchema,
  createdBy: Schema.String,
});
export const branchViewMatchesGenerated: Same<typeof BranchViewSchema.Type, BranchView> = true;

/** fin-core's `Conflict`: one op that cannot be carried to the head (`CONFLICT` details.conflicts). */
export const ConflictSchema = Schema.Struct({
  txId: Schema.String,
  opIndex: Schema.Number,
  reason: Schema.String,
  againstTx: Schema.optionalKey(Schema.NullOr(Schema.String)),
  details: Schema.optionalKey(Schema.Record(Schema.String, Schema.Unknown)),
  fix: Schema.String,
});
export const conflictMatchesGenerated: Same<typeof ConflictSchema.Type, Conflict> = true;

/**
 * Every write takes `txId` (the idempotency key: a retry answers from the log), `author`,
 * `dryRun` (plan against the local database and keep nothing, on a follower too) and `argsHash`
 * (`blake3:<hex>` of the command input, stored on the record; a retry with another hash is
 * `IDEM_KEY_REUSED`).
 */
const Write = {
  projectId: Schema.String,
  txId: Schema.String,
  author: Schema.String,
  dryRun: Schema.optional(Schema.Boolean),
  argsHash: Schema.optional(Schema.String),
};

/** Where a new branch forks from its parent: a seq, or a checkpoint's name. */
export const BranchAtSchema = Schema.Union([
  Schema.Struct({ seq: Schema.Number }),
  Schema.Struct({ checkpoint: Schema.String }),
]);

/** `name`: default `try-<n>`. `from`: a branch reference (id, name or `<name>#<id>`). `at`: default the head. */
export const BranchCreatePayload = Schema.Struct({
  ...Write,
  name: Schema.optional(Schema.String),
  from: Schema.String,
  at: Schema.optional(BranchAtSchema),
});
export const BranchCreateSuccess = Schema.Unknown;
export const BranchListPayload = Schema.Struct({ projectId: Schema.String, all: Schema.optional(Schema.Boolean) });
export const BranchListSuccess = Schema.Struct({ branches: Schema.Array(BranchViewSchema) });
/** Rename (`name`), archive (`archived: true`) or restore (`archived: false`). */
export const BranchSetPayload = Schema.Struct({
  ...Write,
  branch: Schema.String,
  name: Schema.optional(Schema.String),
  archived: Schema.optional(Schema.Boolean),
});
/** `to`: undo back to the checkpoint of that name (`history.undo` only). */
export const HistoryUndoPayload = Schema.Struct({ ...Write, branch: Schema.String, to: Schema.optional(Schema.String) });
export const HistoryRedoPayload = Schema.Struct({ ...Write, branch: Schema.String });
export const HistoryCheckpointPayload = Schema.Struct({ ...Write, branch: Schema.String, name: Schema.String });
export const HistoryMergePayload = Schema.Struct({
  ...Write,
  from: Schema.String,
  into: Schema.String,
  skipConflicting: Schema.optional(Schema.Boolean),
});
export const HistoryCherryPickPayload = Schema.Struct({ ...Write, origin: Schema.String, into: Schema.String });
export const HistoryRevertPayload = Schema.Struct({ ...Write, mergeId: Schema.String, branch: Schema.String });
/** The `CommittedTx` a `branch.set` or `history.checkpoint` answers with. */
export const RecordSuccess = Schema.Unknown;
export const HistoryResultSuccess = Schema.Unknown;
export const MergeReportSuccess = Schema.Unknown;

/**
 * `create`: `"all"` or one group's id (12 hex); without it the groups are only listed.
 * `txId` is needed with `create`. `importDir`: a folder of `.fseg` files moved out of the
 * project folder, read into quarantine first.
 */
export const ProjectRecoverPayload = Schema.Struct({
  projectId: Schema.String,
  txId: Schema.optional(Schema.String),
  author: Schema.optional(Schema.String),
  create: Schema.optional(Schema.String),
  importDir: Schema.optional(Schema.String),
  dryRun: Schema.optional(Schema.Boolean),
});
export const RecoverReportSuccess = Schema.Unknown;

/** Narrowed views of the `Schema.Unknown` results. */
export type BranchCreatedPayload = BranchCreated;
export type HistoryResultPayload = HistoryResult;
export type MergeReportPayload = MergeReport;
export type RecoverReportPayload = RecoverReport;
export type RecordPayload = CommittedTx;
