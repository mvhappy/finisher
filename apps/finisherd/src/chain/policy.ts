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
  /** Every project the call names, as stage 3 resolved them (`Resolved.projects`). */
  readonly projectIds?: ReadonlyArray<string>;
}

/** A rule's answer; `undefined`: no opinion. A `deny` or `ask` ends the stage. */
export type PolicyRule = (input: PolicyInput) => Effect.Effect<Outcome | undefined, FinError>;

/** The author a principal's own branches carry (`"own"` in a credential's `branches`). */
export function authorOf(p: Principal): string {
  return `${p.kind}:${p.id}`;
}

/** Principals that write for a credential (not a person, not `fin`): `agent`, `mcp`, and 5B's `device`, 8's `plugin`. */
const CREDENTIAL_KINDS: ReadonlySet<string> = new Set(["agent", "mcp", "device", "plugin"]);

/**
 * Whether the author of `p`'s new branches is always `authorOf(p)`, whatever the payload says
 * (review of Tasks 8–9, M1): every credential principal, branch-limited or not, so none can
 * plant a branch another agent's `own` would then reach.
 */
export function stampsAuthor(p: Principal): boolean {
  return CREDENTIAL_KINDS.has(p.kind);
}

const projectOf = (payload: unknown): string | undefined => {
  const v = (payload as { projectId?: unknown } | null)?.projectId;
  return typeof v === "string" ? v : undefined;
};

/** Whether `p` may write only to some branches (`branches` is not `"*"`). */
export function branchLimited(p: Principal): boolean {
  return p.branches !== "*";
}

/**
 * Agent branch limits: a principal whose `branches` is not `"*"` writes only to those branches
 * (ids or names, resolved through the store; `"own"` = every branch whose author is
 * `authorOf(principal)`). A write elsewhere is `BRANCH_NOT_ALLOWED`. **Every** write of such a
 * principal (every rule with `writes`, an empty list included) carries the allowed ids as the
 * guard's `allowBranches`, so what the payload does not name (a recovery re-applying a
 * `branchSet`) is still limited inside the store (review of Task 5, H1). A new branch
 * (`branch.create`, recovery's) is in no list and not a target: it becomes the writer's own.
 * A rule's `targets` (a branch acted on without a record, `project.acknowledgeRewind`) are
 * checked the same way.
 */
export const agentBranchLimits: PolicyRule = (input) => agentBranchLimitsExcept(() => false)(input);

/**
 * `agentBranchLimits` for every principal but those `skip` names: an agent session's
 * credential reaches its root branch's whole subtree, which `agent/reach.ts` checks instead
 * (sub-project 7A Decision 16).
 */
export const agentBranchLimitsExcept = (skip: (p: Principal) => boolean): PolicyRule => ({ tag, rule, principal, payload, projects }) =>
  Effect.gen(function* () {
    if (!branchLimited(principal) || skip(principal) || (rule.writes === undefined && rule.targets === undefined)) return undefined;
    const projectId = projectOf(payload);
    // Every writing RPC names its project; one that does not is refused, never left open.
    if (projectId === undefined) return { deny: branchNotAllowed(tag, "") };
    const allowed = new Set<string>();
    for (const entry of principal.branches === "*" ? [] : principal.branches) {
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
      if (only !== undefined) allow = allow === undefined ? new Set(only) : new Set([...allow].filter((b) => only.includes(b)));
    }
    const guard: WriteGuard = { denyBranches: [...deny], ...(allow === undefined ? {} : { allowBranches: [...allow] }) };
    return { allow: true, guard } satisfies Outcome;
  });
}
