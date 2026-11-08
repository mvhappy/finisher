// Stage 5 of the chain (spec §4.4, plan 5A Decision 16): rules that see the resolved request and
// answer `allow | deny | ask`. 5A ships one, agent branch limits; 5E adds protected branches
// (filling the guard's `denyBranches`), sub-project 7 command rules and hooks. The outcome's
// guard goes with every write into `fin-node`, which checks it again inside the store's write
// transaction (Decision 17).
import type { FinError, FinRpcTag, Principal, RpcRule, RuleLayer } from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Effect } from "effect";
import { branchNotAllowed } from "./fixes.ts";
import { type ChainProjects, resolveWrites } from "./resolve.ts";

/**
 * An `ask`: why, and for a command rule (sub-project 7B) the command it decided on and the rule
 * and layer that asked, which an agent session's durable card shows (`ChainDeps.agentAsk`).
 */
export interface Ask {
  readonly reason: string;
  readonly command?: string;
  readonly rule?: string;
  readonly layer?: RuleLayer;
}

/**
 * `undo`: what an `allow` took that must be given back when the call goes no further (a
 * one-time grant consumed for it, sub-project 7B Decision 10): run when a later rule denies
 * or asks, and by the chain when the handler fails with a typed error that committed nothing.
 */
export type Outcome = { readonly allow: true; readonly guard: WriteGuard; readonly undo?: Effect.Effect<void> } | { readonly deny: FinError } | { readonly ask: Ask };

export interface PolicyInput {
  readonly tag: FinRpcTag;
  readonly rule: RpcRule;
  readonly principal: Principal;
  readonly payload: unknown;
  readonly projects: ChainProjects;
  /** Every project the call names, as stage 3 resolved them (`Resolved.projects`). */
  readonly projectIds?: ReadonlyArray<string>;
  /** The call's headers (`fin-actor-call`: the `FIN_CALL` a grant is consumed for). */
  readonly headers?: unknown;
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

/**
 * Runs `rules` in order: the first `deny` or `ask` wins (giving back what earlier allows took,
 * their `undo`s); the allows' guards are combined, and so are their `undo`s.
 */
export function runPolicies(rules: ReadonlyArray<PolicyRule>, input: PolicyInput): Effect.Effect<Outcome, FinError> {
  const undos: Effect.Effect<void>[] = [];
  const undoAll = Effect.suspend(() => Effect.forEach(undos.splice(0), (u) => u, { discard: true }));
  const run = Effect.gen(function* () {
    const deny = new Set<string>();
    let allow: Set<string> | undefined;
    for (const r of rules) {
      const o = yield* r(input);
      if (o === undefined) continue;
      if (!("allow" in o)) {
        yield* undoAll;
        return o;
      }
      if (o.undo !== undefined) undos.push(o.undo);
      for (const b of o.guard.denyBranches ?? []) deny.add(b);
      const only = o.guard.allowBranches;
      if (only !== undefined) allow = allow === undefined ? new Set(only) : new Set([...allow].filter((b) => only.includes(b)));
    }
    const guard: WriteGuard = { denyBranches: [...deny], ...(allow === undefined ? {} : { allowBranches: [...allow] }) };
    const given = undos.splice(0);
    const undo = given.length === 0 ? {} : { undo: Effect.forEach(given, (u) => u, { discard: true }) };
    return { allow: true, guard, ...undo } satisfies Outcome;
  });
  // A rule that fails gives back what the rules before it took.
  return Effect.tapError(run, () => undoAll);
}
