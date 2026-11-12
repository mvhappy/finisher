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
//   `notAuto` (the session is not, or no longer, an `auto` session), `policyOff`,
//   `roleNotAdmitted` (`branch_policy.autoMerge.roles`; absent: every role), `approvalOpen` (an
//   open approval of the session), `overBudget` (a ledger kind used up), `hookBlocked` (7F's
//   `PreMerge`, a hook point that allows today), then the newest critic verdict (the one 5E
//   judges): `criticUnknown` (it names no session this server knows), `criticNotDistinct` (the
//   authoring session or one in its tree: the same `rootSession`), `criticStale` (the critic did
//   not start from the reviewed branch at the head it judged). 5E then judges exactly the
//   verdicts checked here (read once), so a newer one cannot slip past these checks.
// - **Recorded once:** 5E's `review.verdict` says `merged`, `by: policy`, `via: autoMerge`; the
//   outcome ledger records the transactions it carried out of their subtree the same way, not
//   emitted (`OutcomeTracker.observeAutoMerge`), so a later delete does not call them abandoned.
import type { FinError, Session } from "@finisher/contracts";
import { Effect } from "effect";
import type { ChainProjects } from "../chain/resolve.ts";
import type { Audit } from "../control/audit.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { type AutoMergeRefusal, newestVerdict, storedVerdicts, tryAutoMerge, type VerdictSource } from "../reviews/auto-merge.ts";
import { AUTO_MERGE_AVAILABLE } from "../reviews/policy.ts";
import type { Reviews } from "../reviews/service.ts";
import { type BudgetLimits, DEFAULT_BUDGET } from "./budget.ts";
import { BUDGET_KINDS, type BudgetKind } from "./ledger.ts";
import type { OutcomeTracker } from "./outcomes.ts";
import type { BranchReady } from "./review-link.ts";
import { limitsOf } from "./runtime-deps.ts";
import type { AgentStore } from "./store.ts";

export type { VerdictSource };

/** Why the trigger did not merge: its own checks, then 5E's refusals. */
export type AutoMergeReason =
  | "noReview"
  | "notAuto"
  | "roleNotAdmitted"
  | "approvalOpen"
  | "overBudget"
  | "hookBlocked"
  | "criticUnknown"
  | "criticNotDistinct"
  | "criticStale"
  | AutoMergeRefusal;

export type AutoMergeOutcome = { readonly merged: true; readonly mergeId: string } | { readonly merged: false; readonly reason: AutoMergeReason };

/** 7F's `PreMerge` hook point: false blocks the merge. None today: every merge is allowed. */
export type PreMergeHook = (ev: BranchReady, session: Session) => Effect.Effect<boolean>;

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
  readonly store: Pick<AgentStore, "get" | "approvals" | "ledger">;
  readonly reviews: Pick<Reviews, "workflow" | "store" | "gate">;
  readonly audit: Pick<Audit, "append">;
  readonly outcomes: Pick<OutcomeTracker, "observeAutoMerge">;
  /** For the outcome ledger's branch lineage. */
  readonly projects: ChainProjects;
  /** The server's budget defaults (a session's own limits are in its ledger once it ran). */
  readonly defaults?: BudgetLimits;
}

const refused = (reason: AutoMergeReason): AutoMergeOutcome => ({ merged: false, reason });

/** The ledger's limit field of each kind. */
const LIMIT: Record<BudgetKind, "tokens" | "moneyMicros" | "wallMs"> = { tokens: "tokens", money: "moneyMicros", wallMs: "wallMs" };

/** Whether every ledger kind of `s` is below its limit (as `Budget` stops at `used >= limit`). */
function withinBudget(store: AutoMergeTriggerDeps["store"], s: Session, defaults: BudgetLimits): boolean {
  const limits = limitsOf(s.budget, defaults);
  const ledger = store.ledger();
  return BUDGET_KINDS.every((kind) => {
    const l = ledger.read(s.id, kind);
    return l.used < (l.limit ?? limits[LIMIT[kind]]);
  });
}

/** The session at the top of `s`'s tree (a root is its own). */
const treeOf = (s: Session) => s.rootSession ?? s.id;

/**
 * The trigger: one `branchReady` event in, what became of it out. It fails only with what 5E's
 * path fails with (a conflict, the engine's errors); the item stays open either way.
 */
export function autoMergeTrigger(deps: AutoMergeTriggerDeps): (ev: BranchReady) => Effect.Effect<AutoMergeOutcome, FinError> {
  const decide = (ev: BranchReady): Effect.Effect<AutoMergeOutcome, FinError> =>
    Effect.gen(function* () {
      const itemId = ev.reviewId;
      if (itemId === undefined) return refused("noReview");
      if (!(deps.available ?? AUTO_MERGE_AVAILABLE)) return refused("unavailable");
      const s = deps.store.get(ev.sessionId);
      if (s === undefined || s.mode !== "auto") return refused("notAuto");
      const policy = deps.reviews.store.policy(s.projectId).autoMerge;
      if (!policy.enabled) return refused("policyOff");
      if (policy.roles !== undefined && !policy.roles.includes(s.role)) return refused("roleNotAdmitted");
      if (deps.store.approvals(s.id).some((a) => a.status === "open")) return refused("approvalOpen");
      if (!withinBudget(deps.store, s, deps.defaults ?? DEFAULT_BUDGET)) return refused("overBudget");
      if (deps.preMerge !== undefined && !(yield* deps.preMerge(ev, s))) return refused("hookBlocked");
      // Read once: 5E judges exactly these verdicts.
      const verdicts = [...(deps.verdicts ?? storedVerdicts(deps.reviews.store)).verdicts(itemId)];
      const critic = newestVerdict(verdicts, "critic");
      if (critic !== undefined) {
        const by = critic.session === undefined ? undefined : deps.store.get(critic.session);
        if (by === undefined) return refused("criticUnknown");
        if (by.id === s.id || treeOf(by) === treeOf(s)) return refused("criticNotDistinct");
        const item = yield* deps.reviews.workflow.item(itemId);
        if (by.base.branch !== item.source || by.base.seq !== critic.snapshot.sourceSeq) return refused("criticStale");
      }
      const result = yield* tryAutoMerge(itemId, s.id, {
        reviews: deps.reviews,
        verdicts: { verdicts: () => verdicts },
        available: deps.available ?? AUTO_MERGE_AVAILABLE,
        audit: deps.audit,
      });
      if (!result.merged) return refused(result.reason);
      if (result.review !== undefined) yield* deps.outcomes.observeAutoMerge(result.review.item.projectId, result.review.merge, deps.projects);
      return { merged: true, mergeId: result.mergeId } as const;
    });
  return (ev) => Effect.tap(decide(ev), (out) => Effect.sync(() => deps.observe?.(ev, out)));
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
