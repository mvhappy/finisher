// The `review.*` RPCs (5E plan Decision 23, spec §8.6), spread into `FinRpcs` (`rpc.ts`) so that
// file stays small. Their rules are in `scopes.ts`. Review text (titles, summaries, comments,
// reasons) is content: never audited, never in an error.
import type { ReviewPlan } from "@finisher/core-types";
import { Schema } from "effect";
import { Rpc } from "effect/rpc";
import { ChallengeField } from "./auth.ts";
import { FinError } from "./errors.ts";
import {
  BranchPolicySchema,
  CommentSchema,
  DecisionSchema,
  DecisionValueSchema,
  REVIEW_COMMENT_MAX,
  REVIEW_COMMENT_AT_MAX,
  REVIEW_DECISIONS_MAX,
  REVIEW_PROTECTED_MAX,
  REVIEW_REASON_MAX,
  REVIEW_SUMMARY_MAX,
  REVIEW_TITLE_MAX,
  ReviewItemSchema,
  ReviewKindSchema,
  ReviewStateSchema,
  ReviewWatchEventSchema,
  reviewTextProblem,
  SnapshotSchema,
} from "./reviews.ts";

/** Review text under the store's own rule (`reviewTextProblem`: code points for titles, UTF-8 bytes otherwise). */
const reviewText = (o: { readonly min?: number; readonly max: number; readonly bytes?: boolean }) =>
  Schema.String.check(Schema.makeFilter((s: string) => reviewTextProblem(s, o) === undefined, { expected: `review text (${o.bytes === true ? "bytes" : "characters"} ≤ ${o.max}, no control characters but newlines)` }));
const text = (max: number) => reviewText({ max, bytes: true });
const Title = reviewText({ min: 1, max: REVIEW_TITLE_MAX });
const TxId = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256));

export const ReviewIdPayload = Schema.Struct({ itemId: Schema.String });

/** `into` defaults to `main`. A second open request for the same pair answers the open item (`existing`). */
export const ReviewRequestPayload = Schema.Struct({
  projectId: Schema.String,
  source: Schema.String,
  into: Schema.optional(Schema.String),
  title: Title,
  summary: Schema.optional(text(REVIEW_SUMMARY_MAX)),
});
export const ReviewRequestSuccess = Schema.Struct({ item: ReviewItemSchema, existing: Schema.Boolean });

export const ReviewCommentPayload = Schema.Struct({
  itemId: Schema.String,
  body: reviewText({ min: 1, max: REVIEW_COMMENT_MAX, bytes: true }),
  at: Schema.optional(Schema.String.check(Schema.isMaxLength(REVIEW_COMMENT_AT_MAX))),
  txId: Schema.optional(TxId),
});
export const ReviewCommentSuccess = Schema.Struct({ comment: CommentSchema });

/**
 * An item with its change list (`plan`, the generated `ReviewPlan` at the item's snapshot), the
 * decisions made so far (keyed by origin txId) and `stale`: either head moved since.
 */
export const ReviewViewSchema = Schema.Struct({
  item: ReviewItemSchema,
  stale: Schema.Boolean,
  plan: Schema.optional(Schema.Unknown),
  decisions: Schema.Array(DecisionSchema),
  comments: Schema.Array(CommentSchema),
  /**
   * `review.decide` only: the drops another principal made that its accepts cancelled (owner
   * decision 2026-11-12, a reviewer of the target only): the origin, the drop record, its author.
   */
  undropped: Schema.optional(Schema.Array(Schema.Struct({ txId: Schema.String, dropTx: Schema.String, droppedBy: Schema.String }))),
});
export type ReviewView = Omit<typeof ReviewViewSchema.Type, "plan"> & { readonly plan?: ReviewPlan };

export const ReviewListPayload = Schema.Struct({
  projectId: Schema.String,
  state: Schema.optional(ReviewStateSchema),
  kind: Schema.optional(ReviewKindSchema),
});
export const ReviewListSuccess = Schema.Struct({ items: Schema.Array(ReviewItemSchema) });

export const ReviewWatchPayload = Schema.Struct({ projectId: Schema.String });

/** A decision to record: `pending` takes a mark back. */
export const ReviewDecisionInput = Schema.Struct({
  txId: TxId,
  decision: DecisionValueSchema,
  reason: Schema.optional(text(REVIEW_REASON_MAX)),
});

/**
 * `expect`: the item's snapshot (`REVIEW_STALE` otherwise, with the fresh change list in
 * `details`). A rejection writes a `drop` record at once; `requestChanges` hands the branch back.
 */
export const ReviewDecidePayload = Schema.Struct({
  itemId: Schema.String,
  expect: SnapshotSchema,
  decisions: Schema.optional(Schema.Array(ReviewDecisionInput).check(Schema.isMaxLength(REVIEW_DECISIONS_MAX))),
  requestChanges: Schema.optional(Schema.Struct({ reason: Schema.optional(text(REVIEW_REASON_MAX)) })),
});

/** `idemKey` is the merge's `txId`; `acceptPending` adds the snapshot's pending entries to `include`. */
export const ReviewMergePayload = Schema.Struct({
  itemId: Schema.String,
  expect: SnapshotSchema,
  acceptPending: Schema.optional(Schema.Boolean),
  idemKey: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
});
export const ReviewMergeSuccess = Schema.Struct({
  item: ReviewItemSchema,
  /** fin-store's `MergeReport`. */
  merge: Schema.Unknown,
  /** The target moved since the snapshot (Decision 14); the reviewed transactions were still the only ones copied. */
  targetMoved: Schema.Boolean,
  /** The source was archived: an agent's branch with nothing left to merge into the target. */
  archived: Schema.Boolean,
});

export const ReviewClosePayload = Schema.Struct({
  itemId: Schema.String,
  state: Schema.Literals(["rejected", "dismissed"]),
});
export const ReviewCloseSuccess = Schema.Struct({ item: ReviewItemSchema });

/**
 * `review.policy` (`auth.admin`, human-only): `protected` entries are branch references,
 * resolved to ids; turning `autoMerge.enabled` on is `AUTO_MERGE_UNAVAILABLE` (Decision 19).
 */
export const ReviewPolicyPayload = Schema.Struct({
  projectId: Schema.String,
  protected: Schema.optional(Schema.Array(Schema.String).check(Schema.isMaxLength(REVIEW_PROTECTED_MAX))),
  requireReview: Schema.optional(Schema.Boolean),
  autoMerge: Schema.optional(Schema.Struct({ enabled: Schema.Boolean })),
  challenge: ChallengeField,
});
export const ReviewPolicySuccess = Schema.Struct({ policy: BranchPolicySchema, old: BranchPolicySchema });
export const ReviewPolicyGetPayload = Schema.Struct({ projectId: Schema.String });
export const ReviewPolicyGetSuccess = Schema.Struct({ policy: BranchPolicySchema });

export const REVIEW_RPCS = [
  Rpc.make("review.request", { payload: ReviewRequestPayload, success: ReviewRequestSuccess, error: FinError }),
  Rpc.make("review.comment", { payload: ReviewCommentPayload, success: ReviewCommentSuccess, error: FinError }),
  Rpc.make("review.refresh", { payload: ReviewIdPayload, success: ReviewViewSchema, error: FinError }),
  Rpc.make("review.list", { payload: ReviewListPayload, success: ReviewListSuccess, error: FinError }),
  Rpc.make("review.get", { payload: ReviewIdPayload, success: ReviewViewSchema, error: FinError }),
  Rpc.make("review.watch", { payload: ReviewWatchPayload, success: ReviewWatchEventSchema, error: FinError, stream: true }),
  Rpc.make("review.decide", { payload: ReviewDecidePayload, success: ReviewViewSchema, error: FinError }),
  Rpc.make("review.merge", { payload: ReviewMergePayload, success: ReviewMergeSuccess, error: FinError }),
  Rpc.make("review.close", { payload: ReviewClosePayload, success: ReviewCloseSuccess, error: FinError }),
  Rpc.make("review.policy", { payload: ReviewPolicyPayload, success: ReviewPolicySuccess, error: FinError }),
  Rpc.make("review.policy.get", { payload: ReviewPolicyGetPayload, success: ReviewPolicyGetSuccess, error: FinError }),
] as const;
