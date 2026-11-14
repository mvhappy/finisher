// The `auto` merge policy's session-side trigger (plan 7B Task 12, Decision 27; spec §6.5;
// control-plane spec §8.5). On `branchReady` (`review-link.ts`) it decides, from the session's
// side, whether 5E's internal path (`reviews/auto-merge.ts` `tryAutoMerge`) may be asked to
// merge the item; 5E then checks the policy, `requireReview`, the QA and critic verdicts and the
// snapshot itself, merges as the `policy` actor and audits. Any refusal leaves the item open for
// a person.
//
// - **Never from an RPC.** `main.ts` builds the one trigger and subscribes it to `branchReady`,
//   which only a session's `review.request` (the chain's after stage) and a session's end emit;
//   no handler imports this module. It runs after the call that caused it has answered: what it
//   fails with is the link's to report, never the caller's.
// - **Unavailable until sub-project 10:** `available` defaults to `AUTO_MERGE_AVAILABLE` (false):
//   nothing is read and nothing merges. Tests inject `available` and the `VerdictSource` through
//   the server's options (`AgentServeOptions.autoMerge`), never the environment; the default
//   source is `review_verdicts` (`storedVerdicts`).
// - **Checks, cheapest first:** `noReview` (an end without a review: no item), `unavailable`,
//   then the session side (`sessionSide`): `notAuto` (not, or no longer, an `auto` session),
//   `policyOff`, `roleNotAdmitted` (`branch_policy.autoMerge.roles`; absent: every role),
//   `notOwnBranch` (the item's source must be the event's branch and the session's own root:
//   never a child's or a critic's branch), `targetNotAllowed` (the item's target must be the
//   policy's `into`, default `main`), `approvalOpen`, `overBudget` (the runtime's `Budget`:
//   held amounts and the day's money count); then `hookBlocked` (7F's `PreMerge` and `BranchReady`
//   hooks, `hooks/review.ts`: a `BranchReady` block, then a `PreMerge` block, with the item) and
//   the newest critic verdict (the one 5E judges): `criticUnknown` (it
//   names no session this server knows), `criticNotCritic` (that session's role is not
//   `critic`), `criticNotDistinct` (the author or a session of its tree: the same root session),
//   `criticStale` (the critic did not start from the item's source at the judged seq and hash).
// - **Again under the gate:** 5E runs the session side once more (`admit`) with the item and the
//   policy it re-reads under the project's policy gate, so an owner's policy write or a mode,
//   approval or branch change that lands meanwhile is seen. 5E judges exactly the verdicts
//   checked here (read once), so a newer one cannot slip past.
// - **One at a time per item:** a second event for an item while one runs waits for it (it then
//   finds the item merged, or decides afresh): no double merge, no second audit row.
// - **Recorded once:** 5E's `review.verdict` says `merged`, `by: policy`, `via: autoMerge`; the
//   outcome ledger records the transactions it carried out of their subtree the same way, not
//   emitted (`OutcomeTracker.observeAutoMerge`), also after a replay, so a later delete does not
//   call them abandoned.
import { AUTO_MERGE_DEFAULT_INTO, type BranchPolicy, type FinError, type ReviewItem, type Session } from "@finisher/contracts";
import { Effect, Semaphore } from "effect";
import type { ChainProjects } from "../chain/resolve.ts";
import type { Audit } from "../control/audit.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { type AutoMergeRefusal, newestVerdict, storedVerdicts, tryAutoMerge, type VerdictSource } from "../reviews/auto-merge.ts";
import { AUTO_MERGE_AVAILABLE } from "../reviews/policy.ts";
import type { Reviews } from "../reviews/service.ts";
import { CRITIC_ROLE, type OutcomeTracker } from "./outcomes.ts";
import type { BranchReady } from "./review-link.ts";
import type { AgentStore } from "./store.ts";

export type { VerdictSource };

/** The trigger's own refusals: the session side, rechecked under the gate. */
export type SessionRefusal = "notAuto" | "policyOff" | "roleNotAdmitted" | "notOwnBranch" | "targetNotAllowed" | "approvalOpen" | "overBudget";

/** Why the trigger did not merge: its own checks, then 5E's refusals. */
export type AutoMergeReason =
  | "noReview"
  | SessionRefusal
  | "hookBlocked"
  | "criticUnknown"
  | "criticNotCritic"
  | "criticNotDistinct"
  | "criticStale"
  | AutoMergeRefusal;

export type AutoMergeOutcome = { readonly merged: true; readonly mergeId: string } | { readonly merged: false; readonly reason: AutoMergeReason };

/** 7F's `PreMerge` and `BranchReady` hooks (`hooks/review.ts`): false blocks the merge. Absent: allowed. */
export type PreMergeHook = (ev: BranchReady, session: Session, item: ReviewItem) => Effect.Effect<boolean>;

/** What the server's options may set (tests); none of it comes from the environment. */
export interface AutoMergeTriggerOptions {
  /** Sub-project 10's switch; default `AUTO_MERGE_AVAILABLE` (false). */
  readonly available?: boolean;
  /** Where verdicts come from; default the `review_verdicts` rows. */
  readonly verdicts?: VerdictSource;
  /** Told each outcome (tests). */
  readonly observe?: (ev: BranchReady, outcome: AutoMergeOutcome) => void;
  readonly preMerge?: PreMergeHook;
}

export interface AutoMergeTriggerDeps extends AutoMergeTriggerOptions {
  readonly store: Pick<AgentStore, "get" | "approvals">;
  readonly reviews: Pick<Reviews, "workflow" | "store" | "gate">;
  readonly audit: Pick<Audit, "append">;
  readonly outcomes: Pick<OutcomeTracker, "observeAutoMerge">;
  /** For the outcome ledger's branch lineage. */
  readonly projects: ChainProjects;
  /** The runtime's budget check (`Runtime.budgetSpent`): what `s` has used up, or null. */
  readonly budgetSpent: (s: Session) => unknown;
}

const refused = (reason: AutoMergeReason): AutoMergeOutcome => ({ merged: false, reason });

/** The session at the top of `s`'s tree (a root is its own). */
const treeOf = (s: Session) => s.rootSession ?? s.id;

/**
 * The session side of `ev` for `item` under `policy`, read now: the first refusal, or none.
 * Run before 5E is asked, and again by 5E under the policy gate (`admit`).
 */
function sessionSide(deps: AutoMergeTriggerDeps, ev: BranchReady, item: ReviewItem, policy: BranchPolicy): SessionRefusal | undefined {
  const s = deps.store.get(ev.sessionId);
  if (s === undefined || s.mode !== "auto") return "notAuto";
  if (!policy.autoMerge.enabled) return "policyOff";
  const roles = policy.autoMerge.roles;
  if (roles !== undefined && !roles.includes(s.role)) return "roleNotAdmitted";
  if (item.projectId !== s.projectId || s.rootBranch === undefined || item.source !== s.rootBranch || ev.branch !== item.source) return "notOwnBranch";
  if (item.kind !== "branchReview" || item.target !== (policy.autoMerge.into ?? AUTO_MERGE_DEFAULT_INTO)) return "targetNotAllowed";
  if (deps.store.approvals(s.id).some((a) => a.status === "open")) return "approvalOpen";
  if (deps.budgetSpent(s) !== null) return "overBudget";
  return undefined;
}

/**
 * The trigger: one `branchReady` event in, what became of it out. It fails only with what 5E's
 * path fails with (a conflict, the engine's errors); the item stays open either way.
 */
export function autoMergeTrigger(deps: AutoMergeTriggerDeps): (ev: BranchReady) => Effect.Effect<AutoMergeOutcome, FinError> {
  const decide = (ev: BranchReady, itemId: string): Effect.Effect<AutoMergeOutcome, FinError> =>
    Effect.gen(function* () {
      const s = deps.store.get(ev.sessionId);
      if (s === undefined || s.mode !== "auto") return refused("notAuto");
      const policy = deps.reviews.store.policy(s.projectId);
      if (!policy.autoMerge.enabled) return refused("policyOff");
      const item = yield* deps.reviews.workflow.item(itemId);
      const side = sessionSide(deps, ev, item, policy);
      if (side !== undefined) return refused(side);
      if (deps.preMerge !== undefined && !(yield* deps.preMerge(ev, s, item))) return refused("hookBlocked");
      // Read once: 5E judges exactly these verdicts.
      const verdicts = [...(deps.verdicts ?? storedVerdicts(deps.reviews.store)).verdicts(itemId)];
      const critic = newestVerdict(verdicts, "critic");
      if (critic !== undefined) {
        const by = critic.session === undefined ? undefined : deps.store.get(critic.session);
        if (by === undefined) return refused("criticUnknown");
        if (by.role !== CRITIC_ROLE) return refused("criticNotCritic");
        if (by.id === s.id || treeOf(by) === treeOf(s)) return refused("criticNotDistinct");
        const judged = critic.snapshot;
        const sameHead = by.base.branch === item.source && by.base.seq === judged.sourceSeq && (by.base.hash ?? null) === (judged.sourceHash ?? null);
        if (!sameHead) return refused("criticStale");
      }
      const result = yield* tryAutoMerge(itemId, s.id, {
        reviews: deps.reviews,
        verdicts: { verdicts: () => verdicts },
        available: deps.available ?? AUTO_MERGE_AVAILABLE,
        audit: deps.audit,
        admit: (now, policyNow) => sessionSide(deps, ev, now, policyNow),
      });
      if (!result.merged) return refused(result.reason);
      if (result.review !== undefined) yield* deps.outcomes.observeAutoMerge(result.review.item.projectId, result.review.merge, deps.projects);
      return { merged: true, mergeId: result.mergeId } as const;
    });
  // One decision per item at a time; the map holds an item only while an event for it runs.
  const running = new Map<string, { readonly lock: Semaphore.Semaphore; n: number }>();
  const serial = <A, E>(itemId: string, f: Effect.Effect<A, E>): Effect.Effect<A, E> =>
    Effect.suspend(() => {
      const held = running.get(itemId) ?? { lock: Semaphore.makeUnsafe(1), n: 0 };
      held.n++;
      running.set(itemId, held);
      return Effect.ensuring(
        held.lock.withPermits(1)(f),
        Effect.sync(() => {
          if (--held.n === 0) running.delete(itemId);
        }),
      );
    });
  return (ev) => {
    const itemId = ev.reviewId;
    const out: Effect.Effect<AutoMergeOutcome, FinError> =
      itemId === undefined
        ? Effect.succeed(refused("noReview"))
        : !(deps.available ?? AUTO_MERGE_AVAILABLE)
          ? Effect.succeed(refused("unavailable"))
          : serial(itemId, decide(ev, itemId));
    return Effect.tap(out, (o) => Effect.sync(() => deps.observe?.(ev, o)));
  };
}

/** The branch lineage's view of the project registry (only `branches` is read). */
export function registryProjects(registry: Pick<ProjectRegistryShape, "isOpen" | "dirOf" | "resolveBranch" | "branches">): ChainProjects {
  return {
    isOpen: registry.isOpen,
    dirOf: registry.dirOf,
    resolveBranch: registry.resolveBranch,
    branches: (projectId) => Effect.map(registry.branches(projectId, true), (r) => r.branches),
  };
}
