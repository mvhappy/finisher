// Protected branches at the policy stage (5E plan Decision 16, spec §8.5). For every RPC that
// writes or acts on a branch (its rule's `writes` or `targets`), a principal without
// `branch.protected` may not touch a branch the project's policy protects: `BRANCH_PROTECTED`.
// A rule with `protectedNeeds` (`history.drop`, `review.decide|merge|close`: they write into a
// review's target) takes that scope instead for its own targets, whether or not the principal
// holds `branch.protected`. Every write carries the protected ids its principal cannot reach
// as the guard's `denyBranches`, so the store refuses inside the
// write transaction too: a rename or a re-resolution between this stage and the write cannot
// slip past it. `local` holds `branch.protected` (owner decision 5); minted credentials do not.
//
// Only branch ids are compared; names resolve through the store (`resolveWrites`), after
// authorisation, as agent branch limits do.
import { DEFAULT_BRANCH_POLICY, type Scope } from "@finisher/contracts";
import { Effect } from "effect";
import { branchProtected } from "./fixes.ts";
import type { Outcome, PolicyRule } from "./policy.ts";
import { resolveWrites } from "./resolve.ts";

const projectOf = (payload: unknown): string | undefined => {
  const v = (payload as { projectId?: unknown } | null)?.projectId;
  return typeof v === "string" ? v : undefined;
};

/**
 * The outcome for a write of `principal` into `targets` (branch ids) under `protectedIds`: a
 * protected target is reached with `needs` when the rule names one (its own targets only), else
 * with `branch.protected`; every protected branch the principal cannot reach is denied in the
 * write transaction. Shared with the writes that do not pass the policy stage with their
 * branches (`agent.session.rollback`).
 */
export function protectedOutcome(
  tag: string,
  principal: { readonly scopes: ReadonlyArray<Scope> },
  protectedIds: ReadonlyArray<string>,
  targets: ReadonlyArray<string>,
  needs?: Scope,
): Outcome {
  const holds = principal.scopes.includes("branch.protected");
  const reachable = (b: string) => (needs !== undefined && targets.includes(b) ? principal.scopes.includes(needs) : holds);
  const hit = targets.find((t) => protectedIds.includes(t) && !reachable(t));
  if (hit !== undefined) return { deny: branchProtected(tag, hit) };
  return { allow: true, guard: { denyBranches: protectedIds.filter((b) => !reachable(b)) } };
}

export const protectedBranches: PolicyRule = ({ tag, rule, principal, payload, projects, reviews }) =>
  Effect.gen(function* () {
    if (rule.writes === undefined && rule.targets === undefined) return undefined;
    // A rule with `protectedNeeds` is checked whatever else the principal holds: that scope, not
    // `branch.protected`, reaches its targets (Decision 16). Stage 4 asks for it only when the
    // resolver saw the project open (Task 5 review, S1), and the policy may have changed since.
    const needs = rule.protectedNeeds as Scope | undefined;
    if (needs === undefined && principal.scopes.includes("branch.protected")) return undefined;
    // Every write names its project; without one there is no policy to apply (agent branch
    // limits refuse a branch-limited principal's).
    const projectId = projectOf(payload);
    if (projectId === undefined) return undefined;
    const protectedIds = reviews?.protectedBranches(projectId) ?? DEFAULT_BRANCH_POLICY.protected;
    if (protectedIds.length === 0) return { allow: true, guard: { denyBranches: [] } } satisfies Outcome;
    const targets = yield* resolveWrites(rule, payload, projects);
    return protectedOutcome(tag, principal, protectedIds, targets, needs);
  });
