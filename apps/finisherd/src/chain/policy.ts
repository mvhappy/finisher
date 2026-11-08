// Stage 5 of the chain (spec §4.4, plan 5A Decision 16): rules that see the resolved request and
// answer `allow | deny | ask`. 5A ships one, agent branch limits; 5E adds protected branches
// (filling the guard's `denyBranches`), sub-project 7 command rules and hooks. The outcome's
// guard goes with every write into `fin-node`, which checks it again inside the store's write
// transaction (Decision 17).
import type { FinError, FinRpcTag, Principal, RpcRule } from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Effect } from "effect";
import { branchNotAllowed } from "./fixes.ts";
import { type ChainProjects, resolveWrites } from "./resolve.ts";

export type Outcome = { readonly allow: true; readonly guard: WriteGuard } | { readonly deny: FinError } | { readonly ask: { readonly reason: string } };

export interface PolicyInput {
  readonly tag: FinRpcTag;
  readonly rule: RpcRule;
  readonly principal: Principal;
  readonly payload: unknown;
  readonly projects: ChainProjects;
}

/** A rule's answer; `undefined`: no opinion. A `deny` or `ask` ends the stage. */
export type PolicyRule = (input: PolicyInput) => Effect.Effect<Outcome | undefined, FinError>;

/** The author a principal's own branches carry (`"own"` in a credential's `branches`). */
export function authorOf(p: Principal): string {
  return `${p.kind}:${p.id}`;
}

const projectOf = (payload: unknown): string | undefined => {
  const v = (payload as { projectId?: unknown } | null)?.projectId;
  return typeof v === "string" ? v : undefined;
};

/**
 * Agent branch limits: a principal whose `branches` is not `"*"` writes only to those branches
 * (ids or names, resolved through the store; `"own"` = every branch whose author is
 * `authorOf(principal)`). A write elsewhere is `BRANCH_NOT_ALLOWED`; an allowed one carries the
 * allowed ids as the guard's `allowBranches`. A new branch (`branch.create`) is the writer's own.
 */
export const agentBranchLimits: PolicyRule = ({ tag, rule, principal, payload, projects }) =>
  Effect.gen(function* () {
    const projectId = projectOf(payload);
    if (principal.branches === "*" || (rule.writes ?? []).length === 0 || projectId === undefined) return undefined;
    const allowed = new Set<string>();
    for (const entry of principal.branches) {
      if (entry === "own") {
        for (const b of yield* projects.branches(projectId)) if (b.createdBy === authorOf(principal)) allowed.add(b.id);
      } else {
        // An entry that names no branch (yet) allows nothing.
        const id = yield* Effect.orElseSucceed(projects.resolveBranch(projectId, entry), () => undefined);
        if (id !== undefined) allowed.add(id);
      }
    }
    const targets = yield* resolveWrites(rule, payload, projects);
    const outside = targets.find((t) => !allowed.has(t));
    if (outside !== undefined) return { deny: branchNotAllowed(tag, outside) };
    return { allow: true, guard: { denyBranches: [], allowBranches: [...allowed] } } satisfies Outcome;
  });

export const DEFAULT_POLICIES: ReadonlyArray<PolicyRule> = [agentBranchLimits];

/** Runs `rules` in order: the first `deny` or `ask` wins; the allows' guards are combined. */
export function runPolicies(rules: ReadonlyArray<PolicyRule>, input: PolicyInput): Effect.Effect<Outcome, FinError> {
  return Effect.gen(function* () {
    const deny = new Set<string>();
    let allow: Set<string> | undefined;
    for (const r of rules) {
      const o = yield* r(input);
      if (o === undefined) continue;
      if (!("allow" in o)) return o;
      for (const b of o.guard.denyBranches ?? []) deny.add(b);
      const only = o.guard.allowBranches;
      if (only !== undefined && only !== null) allow = allow === undefined ? new Set(only) : new Set([...allow].filter((b) => only.includes(b)));
    }
    const guard: WriteGuard = { denyBranches: [...deny], ...(allow === undefined ? {} : { allowBranches: [...allow] }) };
    return { allow: true, guard } satisfies Outcome;
  });
}
