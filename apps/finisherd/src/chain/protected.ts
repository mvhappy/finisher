// Protected branches at the policy stage (5E plan Decision 16, spec §8.5). For every RPC that
// writes or acts on a branch (its rule's `writes` or `targets`), a principal without
// `branch.protected` may not touch a branch the project's policy protects: `BRANCH_PROTECTED`.
// A rule with `protectedNeeds` (`history.drop`, `review.decide|merge|close`: they write into a
// review's target) takes that scope instead for its own targets; stage 4 has asked for it
// already. For such a principal every write carries the protected ids (less the targets its
// `protectedNeeds` scope reaches) as the guard's `denyBranches`, so the store refuses inside the
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

export const protectedBranches: PolicyRule = ({ tag, rule, principal, payload, projects, reviews }) =>
  Effect.gen(function* () {
    if (rule.writes === undefined && rule.targets === undefined) return undefined;
    if (principal.scopes.includes("branch.protected")) return undefined;
    // Every write names its project; without one there is no policy to apply (agent branch
    // limits refuse a branch-limited principal's).
    const projectId = projectOf(payload);
    if (projectId === undefined) return undefined;
    const protectedIds = reviews?.protectedBranches(projectId) ?? DEFAULT_BRANCH_POLICY.protected;
    if (protectedIds.length === 0) return { allow: true, guard: { denyBranches: [] } } satisfies Outcome;
    const targets = yield* resolveWrites(rule, payload, projects);
    const stands = rule.protectedNeeds !== undefined && principal.scopes.includes(rule.protectedNeeds as Scope);
    const reached = new Set(stands ? targets : []);
    const hit = targets.find((t) => protectedIds.includes(t) && !reached.has(t));
    if (hit !== undefined) return { deny: branchProtected(tag, hit) };
    return { allow: true, guard: { denyBranches: protectedIds.filter((b) => !reached.has(b)) } } satisfies Outcome;
  });
