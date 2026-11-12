// The internal auto-merge path (5E plan Decision 19; spec §8.5; agent spec §6.5): a
// `branchReview` item merged by the server-internal actor `{kind: policy, policy: autoMerge,
// enabledBy}` when the project's owner turned the policy on and the QA gate and the critic both
// passed the item's exact snapshot.
//
// - **No RPC reaches it.** No handler imports this module; its callers are a `VerdictSource`
//   (sub-project 10's, none in release builds) and the tests, which inject the verdicts and the
//   availability through `AutoMergeDeps`, never through the environment.
// - **Unavailable until sub-project 10:** with `AUTO_MERGE_AVAILABLE` false (the default of
//   `available`) nothing is read and nothing merges.
// - **Checks, in order:** the item is `open` (`changesRequested`: a person handed the branch
//   back), `policyOff` (auto merge off, or nobody recorded as having turned it on),
//   `requireReview` (a person decides every entry), `verdictMissing` (no QA or no critic
//   verdict), `verdictFailed` (the newest of either failed), `snapshotMismatch` (the newest of
//   either judged another snapshot than the item's), `replanNeeded` (the snapshot's list is gone
//   and the source moved). They run under the project's policy gate held shared, so a policy
//   write waits for an auto merge in flight, and an auto merge that starts after a policy write
//   began sees the new policy.
// - **The merge** is Decision 14's (`mergeReview`) with `acceptPending: true` and `sourceUpTo` of
//   the judged snapshot (the item's `expect`), so nothing the critic did not see is merged. Its
//   guard lets it write only the item's target and source and keeps every other protected
//   branch denied. Audited `review.merge {auto: true, item, snapshot, session?, verdicts,
//   enabledBy}`; its `review.verdict` says `by: policy`, `via: autoMerge`.
// - A refusal writes nothing: the item stays open for a person.
import { AUDIT_ID_PATTERN, AUDIT_VERDICT_ID_PATTERN, auditDetailsValid, type FinError, type ReviewItem, type Snapshot } from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Effect } from "effect";
import type { Audit } from "../control/audit.ts";
import { type PolicyActor, policyMeta } from "../diagnostics.ts";
import { finError } from "../errors.ts";
import { mergeReview } from "./merge.ts";
import { sameSnapshot } from "./plan.ts";
import { AUTO_MERGE_AVAILABLE } from "./policy.ts";
import type { Reviews } from "./service.ts";
import type { ReviewStore } from "./store.ts";
import type { Caller } from "./workflow.ts";

/** A QA-gate or critic verdict recorded on an item, with the snapshot it judged. */
export interface JudgedVerdict {
  readonly kind: "qa" | "critic";
  readonly pass: boolean;
  readonly verdictId: string;
  readonly snapshot: Snapshot;
  /** When it was recorded (ms): the newest of a kind is the one that counts. */
  readonly at: number;
}

/** Where verdicts come from: sub-project 10's QA gate and critic (`storedVerdicts`), or a test's fake. */
export interface VerdictSource {
  verdicts(itemId: string): ReadonlyArray<JudgedVerdict>;
}

/** The `review_verdicts` rows of the reviews store (Decision 8: written by sub-project 10). */
export function storedVerdicts(store: Pick<ReviewStore, "verdicts">): VerdictSource {
  return { verdicts: (itemId) => store.verdicts(itemId) };
}

export interface AutoMergeDeps {
  readonly reviews: Pick<Reviews, "workflow" | "store" | "gate">;
  readonly verdicts: VerdictSource;
  /** Sub-project 10's switch; tests inject `true`. Default `AUTO_MERGE_AVAILABLE` (false). */
  readonly available?: boolean | undefined;
  /** Where the `review.merge {auto: true}` row goes: an auto merge is never unaudited. */
  readonly audit: Pick<Audit, "append">;
}

/**
 * Why nothing merged. Besides Decision 19's five checks: `unavailable` (before sub-project 10),
 * `changesRequested` (a person handed the branch back; only an `open` item is merged), and
 * `replanNeeded` (the change list of the judged snapshot is gone, after a restart or an
 * eviction, and the source moved since, so it cannot be planned again exactly: a person
 * refreshes the review and it is judged again), and `dropsChanged` (an accepted origin was
 * dropped, or a rejected one undropped, since the decision: the review merge's
 * `REVIEW_UNDECIDED {reason: acceptedDropped | rejectedUndropped}`; a person decides again).
 */
export type AutoMergeRefusal =
  | "unavailable"
  | "changesRequested"
  | "policyOff"
  | "requireReview"
  | "verdictMissing"
  | "verdictFailed"
  | "snapshotMismatch"
  | "replanNeeded"
  | "dropsChanged";

export type AutoMergeResult = { readonly merged: true; readonly mergeId: string } | { readonly merged: false; readonly reason: AutoMergeRefusal };

const refused = (reason: AutoMergeRefusal): AutoMergeResult => ({ merged: false, reason });

/** The review merge's refusal for drops that changed since the decisions (Task 7 round 2). */
const dropsChanged = (e: FinError): boolean =>
  e.code === "REVIEW_UNDECIDED" && ["acceptedDropped", "rejectedUndropped"].includes(String((e.details as { reason?: unknown } | null)?.reason));

/** The audit principal of the policy actor (it has no connection). */
const POLICY_PRINCIPAL = { kind: "policy", id: "autoMerge" } as const;

/**
 * The newest verdict of each kind (a failing one wins a tie at the same `at`), judged: missing,
 * failed, or for another snapshot than the item's, in that order; else the two verdicts the
 * merge relies on. A verdict whose id the audit cannot record does not count (it is missing).
 */
export function judge(verdicts: ReadonlyArray<JudgedVerdict>, snapshot: Snapshot): AutoMergeRefusal | ReadonlyArray<JudgedVerdict> {
  const newer = (a: JudgedVerdict, v: JudgedVerdict) => v.at > a.at || (v.at === a.at && a.pass && !v.pass);
  const newest = (kind: JudgedVerdict["kind"]) =>
    verdicts
      .filter((v) => v.kind === kind && AUDIT_VERDICT_ID_PATTERN.test(v.verdictId))
      .reduce<JudgedVerdict | undefined>((a, v) => (a === undefined || newer(a, v) ? v : a), undefined);
  const both = [newest("qa"), newest("critic")];
  if (both.some((v) => v === undefined)) return "verdictMissing";
  const judged = both as ReadonlyArray<JudgedVerdict>;
  if (judged.some((v) => !v.pass)) return "verdictFailed";
  if (judged.some((v) => !sameSnapshot(v.snapshot, snapshot))) return "snapshotMismatch";
  return judged;
}

/**
 * The policy actor's guard: it may write the target (protected or not) and the source (its
 * archive after the merge), nothing else; every other protected branch stays denied.
 */
export function policyGuard(item: ReviewItem, protectedIds: ReadonlyArray<string>): WriteGuard {
  const deny = protectedIds.filter((id) => id !== item.target);
  return { allowBranches: [...new Set([item.target!, item.source!])], ...(deny.length === 0 ? {} : { denyBranches: deny }) };
}

/** The `Caller` of the policy actor: records authored `policy:autoMerge`, journalled as `system`. */
export function policyCaller(actor: PolicyActor, guard: WriteGuard, session: string | undefined): Caller {
  return { principal: POLICY_PRINCIPAL, meta: policyMeta(actor, guard, session), author: "policy:autoMerge", transport: "internal", internal: actor };
}

/** The merge's `idemKey`: one per judged snapshot, so a replay after a crash finds fin-store's stored merge. */
const idemKeyOf = (item: ReviewItem) => `auto-${item.id}-${item.snapshot!.sourceSeq}`;

/** The audited form of the judged snapshot. */
const snapshotWord = (s: Snapshot) => ({
  sourceSeq: s.sourceSeq,
  ...(s.sourceHash === undefined ? {} : { sourceHash: s.sourceHash }),
  targetSeq: s.targetSeq,
  ...(s.targetHash === undefined ? {} : { targetHash: s.targetHash }),
});

/**
 * Merges `itemId` for the auto-merge policy when every check passes (Decision 19). `session` is
 * the agent session whose branch it is (audited; a ULID, else `INVALID_REQUEST`). A refusal
 * answers its reason and writes nothing.
 *
 * **Never call it while holding the project's policy gate:** it takes the gate shared itself,
 * and a policy write queued in between would deadlock it.
 *
 * A replay after a crash finishes the item: the merge's `idemKey` is fixed per judged snapshot,
 * so fin-store answers the merge it already made and the item is marked merged; an item this
 * path already merged answers `merged: true` again (not audited twice).
 *
 * Errors are the human path's: `REVIEW_NOT_FOUND`, `INVALID_REQUEST {reason: kind}`,
 * `REVIEW_CLOSED`, `REVIEW_HAS_CONFLICTS`, `REVIEW_UNDECIDED {reason: nothingAccepted}` (the
 * review lists open transactions and none would be merged), and the engine's.
 */
export function tryAutoMerge(itemId: string, session: string | undefined, deps: AutoMergeDeps): Effect.Effect<AutoMergeResult, FinError> {
  return Effect.gen(function* () {
    if (!(deps.available ?? AUTO_MERGE_AVAILABLE)) return refused("unavailable");
    if (session !== undefined && !AUDIT_ID_PATTERN.test(session)) {
      return yield* Effect.fail(finError("INVALID_REQUEST", "the auto merge's session is not an id", { field: "session", reason: "format" }));
    }
    const { workflow: w, store, gate } = deps.reviews;
    const first = yield* w.item(itemId);
    return yield* gate.shared(
      first.projectId,
      Effect.gen(function* () {
        // Read again under the gate: the policy and the item as they are now.
        const current = yield* w.item(itemId);
        if (current.kind === "branchReview" && current.state === "merged" && current.snapshot !== undefined && current.mergeId === idemKeyOf(current)) {
          return { merged: true, mergeId: current.mergeId } as const;
        }
        const item = yield* w.openBranchReview(itemId);
        if (item.state !== "open") return refused("changesRequested");
        const policy = store.policy(item.projectId);
        const enabledBy = store.autoMergeEnabledBy(item.projectId);
        if (!policy.autoMerge.enabled || enabledBy === undefined) return refused("policyOff");
        if (policy.requireReview) return refused("requireReview");
        const snapshot = item.snapshot!;
        const judged = judge(deps.verdicts.verdicts(item.id), snapshot);
        if (typeof judged === "string") return refused(judged);
        // The judged snapshot's list: planned again when the source has not moved; otherwise gone.
        if ((yield* w.planner.atSnapshot(item)).approximate) return refused("replanNeeded");
        const details = {
          projectId: item.projectId,
          item: item.id,
          protected: policy.protected.includes(item.target!),
          auto: true,
          snapshot: snapshotWord(snapshot),
          ...(session === undefined ? {} : { session }),
          verdicts: judged.map((v) => ({ kind: v.kind, verdictId: v.verdictId })),
          enabledBy,
        };
        // Checked before the merge: a row the audit would empty must not stand for an auto merge.
        if (!auditDetailsValid("review.merge", details)) {
          return yield* Effect.fail(finError("INTERNAL", "the auto merge's audit row is not recordable", { reason: "auditDetails" }));
        }
        const actor: PolicyActor = { kind: "policy", policy: "autoMerge", enabledBy };
        const caller = policyCaller(actor, policyGuard(item, policy.protected), session);
        const merged = yield* mergeReview(w, caller, { itemId: item.id, expect: snapshot, acceptPending: true, idemKey: idemKeyOf(item) }).pipe(
          Effect.map((m) => ({ ok: m }) as const),
          Effect.catchIf(dropsChanged, () => Effect.succeed({ ok: undefined })),
        );
        if (merged.ok === undefined) return refused("dropsChanged");
        deps.audit.append({ kind: "review.merge", principal: POLICY_PRINCIPAL, outcome: "ok", details });
        return { merged: true, mergeId: merged.ok.merge.mergeId } as const;
      }),
    );
  });
}
