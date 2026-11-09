// Review items (5E plan Decisions 8–10, spec §8.1): TS-owned Effect Schemas, since the items
// live in `control.db` and no Rust type models them. The change list of a `branchReview` is the
// generated `ReviewPlan` (fin-core), derived at the item's snapshot and never stored.
import { Schema } from "effect";

/** Open `branchReview`/`mergeConflict`/`pluginSuggestion` items one project may hold (Decision 10; `REVIEW_LIMIT`). */
export const REVIEW_LIMIT = 256;
/** A title is 1–200 characters. */
export const REVIEW_TITLE_MAX = 200;
/** A summary is at most 8 KiB. */
export const REVIEW_SUMMARY_MAX = 8 * 1024;
/** A comment is at most 16 KiB. */
export const REVIEW_COMMENT_MAX = 16 * 1024;
/** Comments one item holds. */
export const REVIEW_COMMENTS_MAX = 1000;
/** A decision's reason is at most 2 KiB. */
export const REVIEW_REASON_MAX = 2 * 1024;
/** Decisions in one `review.decide`. */
export const REVIEW_DECISIONS_MAX = 10_000;
/** Protected branches one project's policy names. */
export const REVIEW_PROTECTED_MAX = 64;

/** `qaFinding` and `criticSummary` join with sub-project 10 (spec §8.1). */
export const REVIEW_KINDS = ["branchReview", "mergeConflict", "pluginSuggestion"] as const;
export const ReviewKindSchema = Schema.Literals(REVIEW_KINDS);
export type ReviewKind = typeof ReviewKindSchema.Type;

/** `open` ↔ `changesRequested` → `merged` | `rejected` | `dismissed` | `superseded`. */
export const REVIEW_STATES = ["open", "changesRequested", "merged", "rejected", "dismissed", "superseded"] as const;
export const ReviewStateSchema = Schema.Literals(REVIEW_STATES);
export type ReviewState = typeof ReviewStateSchema.Type;
/** The states an item can still be decided in. */
export const REVIEW_OPEN_STATES: ReadonlyArray<ReviewState> = ["open", "changesRequested"];

export const DecisionValueSchema = Schema.Literals(["accept", "reject", "pending"]);
export type DecisionValue = typeof DecisionValueSchema.Type;

/**
 * The heads a change list was planned at (Decision 9): `expect` must match the seqs and hashes
 * (`plannedAt` is ignored), so a rewind to the same seq is caught. A hash is absent at the start
 * of a branch.
 */
export const SnapshotSchema = Schema.Struct({
  sourceSeq: Schema.Number,
  sourceHash: Schema.optional(Schema.String),
  targetSeq: Schema.Number,
  targetHash: Schema.optional(Schema.String),
  plannedAt: Schema.Number,
});
export type Snapshot = typeof SnapshotSchema.Type;

/** Who asked: the principal (`kind:id`) and the actor the call carried (`actorFor`). */
export const RequestedBySchema = Schema.Struct({ principal: Schema.String, actor: Schema.optional(Schema.Unknown) });
export type RequestedBy = typeof RequestedBySchema.Type;

/** One review item. `source`/`target` are branch ids; `subject` is a conflict list or `{pluginId}`. */
export const ReviewItemSchema = Schema.Struct({
  id: Schema.String,
  projectId: Schema.String,
  kind: ReviewKindSchema,
  state: ReviewStateSchema,
  source: Schema.optional(Schema.String),
  target: Schema.optional(Schema.String),
  snapshot: Schema.optional(SnapshotSchema),
  title: Schema.String,
  summary: Schema.optional(Schema.String),
  requestedBy: RequestedBySchema,
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
  closedAt: Schema.optional(Schema.Number),
  closedBy: Schema.optional(Schema.String),
  mergeId: Schema.optional(Schema.String),
  /** Made by the server after a move or a reset, not requested (Decision 22). */
  rederived: Schema.Boolean,
  subject: Schema.optional(Schema.Unknown),
});
export type ReviewItem = typeof ReviewItemSchema.Type;

/** A decision on one origin transaction; keyed `(itemId, txId)`, so it survives a refresh. */
export const DecisionSchema = Schema.Struct({
  txId: Schema.String,
  decision: Schema.Literals(["accept", "reject"]),
  decidedBy: Schema.String,
  reason: Schema.optional(Schema.String),
  at: Schema.Number,
});
export type Decision = typeof DecisionSchema.Type;

export const CommentSchema = Schema.Struct({
  id: Schema.String,
  itemId: Schema.String,
  principal: Schema.String,
  actor: Schema.optional(Schema.Unknown),
  body: Schema.String,
  /** A rational time on the timeline, as the ops write it. */
  at: Schema.optional(Schema.String),
  txId: Schema.optional(Schema.String),
  createdAt: Schema.Number,
});
export type Comment = typeof CommentSchema.Type;

/** What `review_events` records. */
export const REVIEW_EVENT_KINDS = ["opened", "refreshed", "decided", "changesRequested", "merged", "closed", "superseded", "reopened"] as const;
export const ReviewEventKindSchema = Schema.Literals(REVIEW_EVENT_KINDS);
export type ReviewEventKind = typeof ReviewEventKindSchema.Type;

export const ReviewEventSchema = Schema.Struct({
  id: Schema.Number,
  itemId: Schema.String,
  at: Schema.Number,
  kind: ReviewEventKindSchema,
  by: Schema.String,
  details: Schema.Record(Schema.String, Schema.Unknown),
});
export type ReviewEvent = typeof ReviewEventSchema.Type;

/** The per-project policy (Decision 18); the default protects `main` and turns the rest off. */
export const BranchPolicySchema = Schema.Struct({
  /** Branch ids. */
  protected: Schema.Array(Schema.String),
  requireReview: Schema.Boolean,
  autoMerge: Schema.Struct({ enabled: Schema.Boolean }),
});
export type BranchPolicy = typeof BranchPolicySchema.Type;
export const DEFAULT_BRANCH_POLICY: BranchPolicy = { protected: ["main"], requireReview: false, autoMerge: { enabled: false } };

/** What `review.watch` carries: the project's items first, then each change. Never a re-plan. */
export const ReviewWatchEventSchema = Schema.Union([
  Schema.Struct({ type: Schema.Literal("snapshot"), items: Schema.Array(ReviewItemSchema) }),
  Schema.Struct({ type: Schema.Literal("item"), item: ReviewItemSchema }),
]);
export type ReviewWatchEvent = typeof ReviewWatchEventSchema.Type;
/** Events a `review.watch` holds for a reader that falls behind; past it the stream ends `WATCH_LAGGING`. */
export const REVIEW_WATCH_BUFFER = 256;
