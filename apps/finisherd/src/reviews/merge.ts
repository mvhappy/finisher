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
import type { FinError, ReviewItem } from "@finisher/contracts";
import type { CommittedTx, MergeReport, ReviewPlan } from "@finisher/core-types";
import { blake3Hex } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { type Caller, openOrigins, principalKey, reviewClosed, sync, wrongKind, type ReviewWorkflow } from "./workflow.ts";

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

/** Archives an agent's source branch once nothing of it is left to merge into the target; never fails the merge. */
function archiveIfFinished(w: ReviewWorkflow, c: Caller, item: ReviewItem, idemKey: string): Effect.Effect<void> {
  const self = Effect.gen(function* () {
    const source = (yield* w.engine.branches(item.projectId)).find((b) => b.id === item.source);
    if (source === undefined || source.state === "archived" || !CREDENTIAL_AUTHOR.test(source.createdBy)) return;
    const plan: ReviewPlan = yield* w.planner.now(item.projectId, item.source!, item.target!);
    if (plan.truncated || plan.entries.some((e) => e.branch === item.source && LEFT.has(e.status))) return;
    yield* w.engine.write(item.projectId, "branchSet", { projectId: item.projectId, txId: `${idemKey}.archive`, author: c.author, branch: source.id, archived: true }, c.meta);
  });
  return Effect.ignore(self);
}

/** `review.merge` (Decision 14). */
export function mergeReview(w: ReviewWorkflow, c: Caller, req: ReviewMergeReq): Effect.Effect<ReviewMerged, FinError> {
  return Effect.gen(function* () {
    const item = yield* w.item(req.itemId);
    if (item.kind !== "branchReview") return yield* Effect.fail(wrongKind(item));
    if (item.state === "merged" && item.mergeId === req.idemKey) {
      // A retry: fin-store answers the merge from the log (an empty include copies nothing new).
      const report = yield* engineMerge(w, c, item, [], req.idemKey);
      return { item, merge: report, targetMoved: yield* targetMoved(w, item, report) };
    }
    if (item.state !== "open" && item.state !== "changesRequested") return yield* Effect.fail(reviewClosed(item));
    yield* w.checkExpect(item, req.expect);
    const { plan } = yield* w.planner.atSnapshot(item);
    const decisions = w.store.decisions(item.id);
    const origins = openOrigins(plan);
    const pending = origins.filter((o) => !decisions.has(o));
    if (w.store.policy(item.projectId).requireReview && pending.length > 0) {
      return yield* Effect.fail(
        finError("REVIEW_UNDECIDED", `${pending.length} transaction(s) of the review are undecided`, { itemId: item.id, pending: pending.length }, "Accept or reject every transaction the review lists, then merge."),
      );
    }
    const accepted = origins.filter((o) => decisions.get(o)?.decision === "accept");
    const include = [...accepted, ...(req.acceptPending === true ? pending : [])].sort();
    const report = yield* engineMerge(w, c, item, include, req.idemKey);
    const by = principalKey(c.principal);
    const merged = yield* sync(() => w.store.setState(item.id, "merged", by, { mergeId: report.mergeId }));
    const isProtected = w.store.policy(item.projectId).protected.includes(item.target!);
    if (isProtected) {
      w.audit?.append({
        kind: "review.merge",
        principal: { kind: c.principal.kind, id: c.principal.id },
        outcome: "ok",
        details: { projectId: item.projectId, item: item.id, protected: true, auto: false, transport: c.transport },
      });
    }
    w.emit(c, merged, "merged", report.merged, "accepted");
    const moved = yield* targetMoved(w, item, report);
    yield* archiveIfFinished(w, c, item, req.idemKey);
    return { item: merged, merge: report, targetMoved: moved };
  });
}
