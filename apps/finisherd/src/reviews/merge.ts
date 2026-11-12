// `review.merge` (5E plan Decision 14, spec §8.2): the item's reviewed transactions, and no
// other, merged through the engine (`Store::merge {txId: idemKey, include, sourceUpTo}`, checked
// inside the merge's write transaction), never through the `history.merge` RPC.
//
// - `include`: the accepted origins of the snapshot's change list, plus its pending ones with
//   `acceptPending`; with the policy's `requireReview`, a pending one refuses the merge
//   (`REVIEW_UNDECIDED`) whatever `acceptPending` says. `sourceUpTo` is the snapshot's source
//   seq, so a commit made on the source after the snapshot is never merged by it.
// - A conflict (`DEPENDS_ON_SKIPPED` included) is `REVIEW_HAS_CONFLICTS` with the list; nothing is
//   written.
// - A target that moved since the snapshot does not block it (owner decision, 2026-11-08): the
//   result says `targetMoved: true`.
// - Afterwards the item is `merged` with its `mergeId`, and the source is archived (a
//   `branchSet` by the merger) when an `agent:`/`mcp:` credential made it and nothing of it is
//   left to merge into the target.
// - A retry with the item's own `mergeId` as `idemKey` answers the merge again (fin-store
//   answers it from the log); any other write on a merged item is `REVIEW_CLOSED`.
// - The auto-merge policy's actor (`auto-merge.ts`, Decision 19) calls this too, under the gate
//   it takes itself, and writes its own `review.merge {auto: true}` row.
import type { FinError, ReviewItem } from "@finisher/contracts";
import type { CommittedTx, MergeReport, ReviewPlan } from "@finisher/core-types";
import { blake3Hex } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { type Caller, principalKey, reviewClosed, sync, wrongKind, type ReviewWorkflow } from "./workflow.ts";

export interface ReviewMergeReq {
  readonly itemId: string;
  readonly expect: NonNullable<ReviewItem["snapshot"]>;
  readonly acceptPending?: boolean | undefined;
  readonly idemKey: string;
}

/** `review.merge`'s answer (`ReviewMergeSuccess`): the item, fin-store's report, whether the target moved. */
export interface ReviewMerged {
  readonly item: ReviewItem;
  readonly merge: MergeReport;
  readonly targetMoved: boolean;
  /** The source was archived (an agent's branch with nothing left to merge). */
  readonly archived: boolean;
}

/** The prefixes of a credential's author (`authorOf`): an agent's branch is archived once merged. */
const CREDENTIAL_AUTHOR = /^(agent|mcp):/;

/** Entry statuses that a merge of the source into the target would still copy (`left_to_merge`). */
const LEFT: ReadonlySet<string> = new Set(["willMerge", "dependsOnSkipped", "conflict"]);

/**
 * The merge's `argsHash`: the item and the pair, so an `idemKey` another item's merge (or any
 * other record) used is `IDEM_KEY_REUSED`, never that merge's answer.
 */
function argsHash(item: ReviewItem): string {
  const args = { cmd: "review.merge", args: { item: item.id, from: item.source, into: item.target, sourceUpTo: item.snapshot?.sourceSeq } };
  return `blake3:${blake3Hex(Buffer.from(JSON.stringify(args)))}`;
}

/** Whether the target moved since the snapshot: the first copy's rev, else the target's head now. */
function targetMoved(w: ReviewWorkflow, item: ReviewItem, report: MergeReport): Effect.Effect<boolean, FinError> {
  const first: CommittedTx | undefined = report.committed.find((c) => c.tx.branch === item.target);
  if (first !== undefined) return Effect.succeed(first.rev !== item.snapshot!.targetSeq + 1);
  return Effect.map(w.engine.branches(item.projectId), (bs) => bs.find((b) => b.id === item.target)?.head !== item.snapshot!.targetSeq);
}

/** Runs the engine merge; a `CONFLICT` is `REVIEW_HAS_CONFLICTS` and goes to `observeMerge` too. */
function engineMerge(w: ReviewWorkflow, c: Caller, item: ReviewItem, include: ReadonlyArray<string>, idemKey: string): Effect.Effect<MergeReport, FinError> {
  const payload = {
    projectId: item.projectId,
    txId: idemKey,
    author: c.author,
    from: item.source!,
    into: item.target!,
    include: [...include],
    sourceUpTo: item.snapshot!.sourceSeq,
    argsHash: argsHash(item),
  };
  return Effect.map(w.engine.write(item.projectId, "merge", payload, c.meta), (r) => r as MergeReport).pipe(
    Effect.tapError((error) => w.observeMerge(item.projectId, item.source!, item.target!, { error }, principalKey(c.principal))),
    Effect.tap((report) => w.observeMerge(item.projectId, item.source!, item.target!, { report }, principalKey(c.principal))),
    Effect.mapError((e) =>
      e.code === "CONFLICT"
        ? finError(
            "REVIEW_HAS_CONFLICTS",
            "the reviewed transactions do not merge cleanly",
            { itemId: item.id, conflicts: (e.details as { conflicts?: unknown } | null)?.conflicts ?? [] },
            "Reject the conflicting transactions, or ask for changes and review the branch again.",
          )
        : e,
    ),
  );
}

/**
 * Archives an agent's source branch once nothing is left to merge into the target (no entry of a
 * fresh plan `willMerge|dependsOnSkipped|conflict`, on the source or an ancestor); whether it
 * did. Never fails the merge: a refused archive (a branch-limited reviewer's guard) is `false`.
 */
function archiveIfFinished(w: ReviewWorkflow, c: Caller, item: ReviewItem, idemKey: string): Effect.Effect<boolean> {
  const self = Effect.gen(function* () {
    const source = (yield* w.engine.branches(item.projectId)).find((b) => b.id === item.source);
    if (source === undefined || !CREDENTIAL_AUTHOR.test(source.createdBy)) return false;
    if (source.state === "archived") return true;
    const plan: ReviewPlan = yield* w.planner.now(item.projectId, item.source!, item.target!);
    if (plan.truncated || plan.entries.some((e) => LEFT.has(e.status))) return false;
    yield* w.engine.write(item.projectId, "branchSet", { projectId: item.projectId, txId: `${idemKey}.archive`, author: c.author, branch: source.id, archived: true }, c.meta);
    yield* w.observeArchived(item.projectId, source.id, principalKey(c.principal));
    return true;
  });
  return Effect.orElseSucceed(self, () => false);
}

const undecided = (item: ReviewItem, pending: number, reason?: string) =>
  finError(
    "REVIEW_UNDECIDED",
    reason === undefined ? `${pending} transaction(s) of the review are undecided` : "no transaction of the review is accepted",
    { itemId: item.id, pending, ...(reason === undefined ? {} : { reason }) },
    "Accept or reject the transactions the review lists, then merge.",
  );

/** `review.merge` (Decision 14). */
export function mergeReview(w: ReviewWorkflow, c: Caller, req: ReviewMergeReq): Effect.Effect<ReviewMerged, FinError> {
  return Effect.gen(function* () {
    const item = yield* w.item(req.itemId);
    if (item.kind !== "branchReview") return yield* Effect.fail(wrongKind(item));
    if (item.state === "merged" && item.mergeId === req.idemKey) {
      // A retry: fin-store answers the merge from the log (an empty include copies nothing new);
      // an archive the first call did not get to is made now.
      const report = yield* engineMerge(w, c, item, [], req.idemKey);
      return { item, merge: report, targetMoved: yield* targetMoved(w, item, report), archived: yield* archiveIfFinished(w, c, item, req.idemKey) };
    }
    if (item.state !== "open" && item.state !== "changesRequested") return yield* Effect.fail(reviewClosed(item));
    yield* w.checkExpect(item, req.expect);
    const { plan, approximate } = yield* w.planner.atSnapshot(item);
    // After a restart or an eviction the snapshot's own list is gone; when the source moved, the
    // list made now may hold records the snapshot never had, which pending must never take (S3).
    if (req.acceptPending === true && approximate) {
      return yield* Effect.fail(
        finError("REVIEW_STALE", "the review's change list must be planned again before pending transactions are merged", { itemId: item.id, reason: "replan" }, "Refresh the review, check what it lists, then merge."),
      );
    }
    // What the review lists (not on the target at the snapshot), and where each stands now: a
    // drop or undrop since the snapshot does not change the cached list (B1).
    const listed = [...new Set(plan.entries.filter((e) => e.status !== "alreadyPresent").map((e) => e.origin))];
    const status = yield* w.planner.statusNow(item);
    const live = listed.filter((o) => status.get(o) === "open");
    const decisions = w.store.decisions(item.id);
    const pending = live.filter((o) => !decisions.has(o));
    if (w.store.policy(item.projectId).requireReview && pending.length > 0) return yield* Effect.fail(undecided(item, pending.length));
    // An accepted origin someone dropped since it was accepted would be left out silently
    // (`DROPPED`): the reviewer accepts it again (which undrops it) or sets it pending.
    const accepted = listed.filter((o) => decisions.get(o)?.decision === "accept");
    const acceptedDropped = accepted.filter((o) => status.get(o) === "dropped");
    if (acceptedDropped.length > 0) {
      return yield* Effect.fail(
        finError("REVIEW_UNDECIDED", "transaction(s) the review accepted were dropped since", { itemId: item.id, reason: "acceptedDropped", txIds: acceptedDropped }, "Accept them again to bring them back, or set them pending, then merge."),
      );
    }
    // And the mirror: a rejection someone undropped since would merge later through a plain merge.
    const rejectedUndropped = listed.filter((o) => decisions.get(o)?.decision === "reject" && status.get(o) === "open");
    if (rejectedUndropped.length > 0) {
      return yield* Effect.fail(
        finError("REVIEW_UNDECIDED", "transaction(s) the review rejected were undropped since", { itemId: item.id, reason: "rejectedUndropped", txIds: rejectedUndropped }, "Reject them again to drop them, or accept them, then merge."),
      );
    }
    const include = [...accepted, ...(req.acceptPending === true ? pending : [])].sort();
    // Nothing accepted while the review still lists open transactions: not a merge (S2).
    if (include.length === 0 && live.length > 0) return yield* Effect.fail(undecided(item, pending.length, "nothingAccepted"));
    const report = yield* engineMerge(w, c, item, include, req.idemKey);
    const by = principalKey(c.principal);
    const merged = yield* sync(() => w.store.setState(item.id, "merged", by, { mergeId: report.mergeId }));
    const isProtected = w.store.policy(item.projectId).protected.includes(item.target!);
    // The auto-merge policy's actor audits its own row (`auto: true`, `auto-merge.ts`).
    if (isProtected && c.internal === undefined) {
      w.audit?.append({
        kind: "review.merge",
        principal: { kind: c.principal.kind, id: c.principal.id },
        outcome: "ok",
        details: { projectId: item.projectId, item: item.id, protected: true, auto: false, transport: c.transport },
      });
    }
    w.emit(c, merged, "merged", report.merged, "accepted");
    const moved = yield* targetMoved(w, item, report);
    return { item: merged, merge: report, targetMoved: moved, archived: yield* archiveIfFinished(w, c, item, req.idemKey) };
  });
}
