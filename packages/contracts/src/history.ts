// Branch and history RPC payloads and results (plan 4B Task 18). The result types are the
// generated ones (`@finisher/core-types`), and so are their Effect schemas
// (`generated/schemas.ts`, from the same JSON Schema): exact, since a `Struct` drops any field
// it does not list. A `CommittedTx` (the records in each result) is passed through unvalidated,
// typed as the generated type (fin-core validated it on commit).
import { Schema } from "effect";
import {
  BranchCreatedSchema,
  BranchStateSchema,
  BranchViewSchema,
  CommittedTxSchema,
  ConflictSchema,
  HistoryResultSchema,
  MergeReportSchema,
  RecoverReportSchema,
} from "./generated/schemas.ts";

export type {
  BranchCreated,
  BranchView,
  Conflict,
  EntryStatus,
  HistoryResult,
  MergeReport,
  RecoverGroup,
  RecoverReport,
  ReviewEntry,
  ReviewHead,
  ReviewPlan,
} from "@finisher/core-types";

/** fin-store's `BranchView` and its `BranchState`; fin-core's `Conflict` (`CONFLICT` details.conflicts). */
export { BranchStateSchema, BranchViewSchema, ConflictSchema };

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
export const BranchCreateSuccess = BranchCreatedSchema;
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
/**
 * The most origin txIds a merge's `include` names (fin-core's `MAX_INCLUDE`): a review plan's
 * entries at most, so a review merge can include every entry it lists.
 */
export const MERGE_INCLUDE_MAX = 10_000;
/** The longest txId on the wire (fin-core's `MAX_RAW_ID`, bytes). */
export const TX_ID_MAX = 256;

/**
 * Merge `from` into `into`. `include`: copy only the candidates whose origin root is named
 * (`[]` copies nothing); `sourceUpTo`: copy no record of the source branch itself above that
 * seq (records it holds from its parent pass). Both are applied inside the merge's write
 * transaction; what they leave out is `notIncluded`, and an included record depending on one
 * left out is a `DEPENDS_ON_SKIPPED` conflict (plan 5E Decision 6).
 */
export const HistoryMergePayload = Schema.Struct({
  ...Write,
  from: Schema.String,
  into: Schema.String,
  skipConflicting: Schema.optional(Schema.Boolean),
  include: Schema.optional(Schema.Array(Schema.String.check(Schema.isMaxLength(TX_ID_MAX))).check(Schema.isMaxLength(MERGE_INCLUDE_MAX))),
  sourceUpTo: Schema.optional(
    Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(Number.MAX_SAFE_INTEGER)),
  ),
});
/** The most origin roots one `drop` record names, `drops` and `undrops` together (fin-core's `MAX_DROP_IDS`). */
export const DROP_IDS_MAX = 4096;

/**
 * `history.drop` (5E Decision 5): write a `drop` record on `into` that keeps the origin roots in
 * `drops` out of every merge of `from` (and its descendants), or lifts a drop (`undrops`). At
 * least one list is non-empty; the server always sets `argsHash`. Needs `review.decide` on a
 * protected target, `project.write` elsewhere.
 */
export const HistoryDropPayload = Schema.Struct({
  ...Write,
  from: Schema.String,
  into: Schema.String,
  drops: Schema.optional(Schema.Array(Schema.String.check(Schema.isMaxLength(TX_ID_MAX))).check(Schema.isMaxLength(DROP_IDS_MAX))),
  undrops: Schema.optional(Schema.Array(Schema.String.check(Schema.isMaxLength(TX_ID_MAX))).check(Schema.isMaxLength(DROP_IDS_MAX))),
});
export const HistoryCherryPickPayload = Schema.Struct({ ...Write, origin: Schema.String, into: Schema.String });
export const HistoryRevertPayload = Schema.Struct({ ...Write, mergeId: Schema.String, branch: Schema.String });
/** The `CommittedTx` a `branch.set` or `history.checkpoint` answers with. */
export const RecordSuccess = CommittedTxSchema;
export const HistoryResultSuccess = HistoryResultSchema;
export const MergeReportSuccess = MergeReportSchema;

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
  /** A group id that cannot be recovered: stop listing and counting it on this server. */
  dismiss: Schema.optional(Schema.String),
  dryRun: Schema.optional(Schema.Boolean),
});
export const RecoverReportSuccess = RecoverReportSchema;
