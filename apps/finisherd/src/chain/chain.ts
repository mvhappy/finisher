// The middleware chain (spec §4.4, plan 5A Decision 14): every handler is wrapped once, unary
// and stream alike, so that each request passes, in this order:
//   1. decode (the protocol, `protocol.ts`): `INVALID_REQUEST`;
//   2. authenticate: anything but the public RPCs from `anonymous` is `UNAUTHENTICATED`;
//   3. resolve (`resolve.ts`): projects, directories and paths become current facts;
//   4. authorise (`authorize.ts`): scopes (all), human-only, reach;
//   5. policy (`policy.ts`): branch write targets resolved through the store, then
//      `allow {guard} | deny | ask` (`ask` answers `APPROVAL_REQUIRED {approvalId}`);
//   6. the handler, with the resolved payload and the guard (`options.guard`), which every
//      write passes into `fin-node` for the store to check inside its write transaction;
//   7. after (sub-project 7's hooks; nothing in 5A but `project.open`'s reach re-check).
// A refusal at stages 2–5 is audited (`auth.denied`: tag, code, scope or rule, principal kind;
// never the payload) and counted in `rpc.stats` (`timed`), never journalled.
import { type FinError, type FinRpcTag, PUBLIC_RPCS, type Principal, type RuleFacts, ruleOf, STREAM_RPCS } from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Effect, Stream } from "effect";
import { unauthenticated } from "../auth/handshake.ts";
import type { Connections, Transport } from "../connections.ts";
import type { Audit } from "../control/audit.ts";
import { ulid } from "../control/ulid.ts";
import { authorize } from "./authorize.ts";
import { approvalRequired, outOfReach } from "./fixes.ts";
import { authorOf, branchLimited, DEFAULT_POLICIES, type PolicyRule, runPolicies } from "./policy.ts";
import { type ChainProjects, resolve } from "./resolve.ts";

export type { Outcome, PolicyRule } from "./policy.ts";

/** The guard of a write no rule limits. */
export const OPEN_GUARD: WriteGuard = { denyBranches: [] };

/**
 * The guard a write handler passes to the store. A write that did not pass the chain, or a
 * branch-limited principal's write without an allow-list, is a defect (`INTERNAL`), never an
 * open guard (review of Task 5, H1 and L3).
 */
export function writeGuardOf(options: object): Effect.Effect<WriteGuard> {
  return Effect.suspend(() => {
    const c = chainOf(options);
    if (c === undefined) return Effect.die(new Error("a write reached its handler without the chain's guard"));
    if (branchLimited(c.principal) && c.guard.allowBranches === undefined) {
      return Effect.die(new Error("a branch-limited principal's write has no allow-list"));
    }
    return Effect.succeed(c.guard);
  });
}

export interface ChainDeps {
  readonly connections: Connections;
  readonly projects: ChainProjects;
  readonly audit?: Audit;
  /** Counts a unary refusal in `rpc.stats`. */
  readonly timed?: <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
  /** The policy rules, in order (default: agent branch limits). */
  readonly policies?: ReadonlyArray<PolicyRule>;
  /** Approvals asked for (`ask`), by id, in memory (sub-project 7 owns approving them). */
  readonly approvals?: Map<string, { readonly tag: string; readonly principal: string; readonly at: number }>;
}

/** What a handler sees in its options beside Effect's: the policy stage's guard and what was resolved. */
export interface ChainOptions {
  readonly guard: WriteGuard;
  readonly principal: Principal;
  readonly facts: RuleFacts;
  /** `project.open`: the project its folder named when it was checked. */
  readonly openedProject?: string;
}

/** The chain's options of a handler call; `undefined` outside the chain (a write then refuses). */
export function chainOf(options: object): ChainOptions | undefined {
  const o = options as Partial<ChainOptions>;
  if (o.guard === undefined || o.principal === undefined) return undefined;
  return { guard: o.guard, principal: o.principal, facts: o.facts ?? {}, ...(o.openedProject === undefined ? {} : { openedProject: o.openedProject }) };
}

type Handler = (payload: unknown, options: { readonly client: { readonly id: number }; readonly [k: string]: unknown }) => unknown;

/** The codes stages 2–5 refuse with, audited as `auth.denied` with the rule that refused. */
const DENIALS = new Map<string, string>([
  ["UNAUTHENTICATED", "authenticate"],
  ["HUMAN_ONLY", "humanOnly"],
  ["SCOPE_DENIED", "scopes"],
  ["PROJECT_NOT_FOUND", "reach"],
  ["PATH_NOT_ALLOWED", "reach"],
  ["BRANCH_NOT_ALLOWED", "agentBranchLimits"],
  ["BRANCH_PROTECTED", "protectedBranches"],
  ["APPROVAL_REQUIRED", "ask"],
]);

/** `handlers` with the chain in front of each one. */
export function withChain<H extends object>(handlers: H, deps: ChainDeps): H {
  const streams = new Set<string>(STREAM_RPCS);
  const policies = deps.policies ?? DEFAULT_POLICIES;
  const approvals = deps.approvals ?? new Map();
  const timed = deps.timed ?? ((_, self) => self);

  const deny = (tag: string, principal: Principal, transport: Transport) => (e: FinError) =>
    Effect.sync(() => {
      const rule = DENIALS.get(e.code);
      if (rule === undefined) return;
      const d = (e.details ?? {}) as Record<string, unknown>;
      deps.audit?.append({
        kind: "auth.denied",
        principal: { kind: principal.kind, id: principal.id },
        outcome: "denied",
        code: e.code,
        details: {
          rpc: tag,
          transport,
          rule,
          ...(typeof d.scope === "string" ? { scope: d.scope } : {}),
          ...(Array.isArray(d.scopes) ? { scopes: d.scopes } : {}),
          ...(typeof d.reason === "string" && /^[a-z][a-zA-Z0-9]{0,47}$/.test(d.reason) ? { reason: d.reason } : {}),
        },
      });
    });

  /** Stages 2–5; the resolved payload and the guard, or the refusal. */
  const check = (tag: FinRpcTag, payload: unknown, clientId: number) => {
    const principal = deps.connections.principalOf(clientId);
    const rule = ruleOf(tag);
    const stages = Effect.gen(function* () {
      if (!PUBLIC_RPCS.has(tag) && principal.kind === "anonymous") return yield* Effect.fail(unauthenticated("unproved"));
      const resolved = yield* resolve(rule, payload, deps.projects);
      yield* authorize(tag, principal, payload, resolved, deps.projects);
      const outcome = yield* runPolicies(policies, { tag, rule, principal, payload: resolved.payload, projects: deps.projects });
      if ("deny" in outcome) return yield* Effect.fail(outcome.deny);
      if ("ask" in outcome) {
        const approvalId = ulid();
        approvals.set(approvalId, { tag, principal: `${principal.kind}:${principal.id}`, at: Date.now() });
        return yield* Effect.fail(approvalRequired(tag, approvalId, outcome.ask.reason));
      }
      // A branch-limited principal's new branches carry its own author, whatever it sent (M4).
      const stamped =
        rule.author !== undefined && branchLimited(principal)
          ? { ...(resolved.payload as Record<string, unknown>), [rule.author]: authorOf(principal) }
          : resolved.payload;
      return { payload: stamped, guard: outcome.guard, principal, facts: resolved.facts, openedProject: resolved.openedProject };
    });
    return Effect.tapError(stages, deny(tag, principal, deps.connections.transport));
  };

  /** Stage 7: `project.open` opened the project its folder named when it was checked. */
  const after = (tag: string, principal: Principal, opened: string | undefined) => (result: unknown) => {
    if (tag !== "project.open" || principal.projects === "*") return Effect.succeed(result);
    const got = (result as { projectId?: unknown } | null)?.projectId;
    return got === opened ? Effect.succeed(result) : Effect.fail(outOfReach(typeof got === "string" ? got : undefined));
  };

  type Checked = Effect.Success<ReturnType<typeof check>>;
  const optionsOf = <O extends object>(options: O, c: Checked) => ({
    ...options,
    guard: c.guard,
    principal: c.principal,
    facts: c.facts,
    ...(c.openedProject === undefined ? {} : { openedProject: c.openedProject }),
  });
  /** A refusal counted in `rpc.stats` (streams too: only their refusals, review M5). */
  const counted = (tag: FinRpcTag, payload: unknown, clientId: number) =>
    Effect.catchCause(check(tag, payload, clientId), (cause) => timed(tag, Effect.failCause(cause)));

  const out: Record<string, Handler> = {};
  for (const [tag, handler] of Object.entries(handlers) as Array<[FinRpcTag, Handler]>) {
    if (streams.has(tag)) {
      out[tag] = (payload, options) =>
        Stream.unwrap(
          Effect.map(counted(tag, payload, options.client.id), (c) => handler(c.payload, optionsOf(options, c)) as Stream.Stream<unknown, FinError>),
        );
      continue;
    }
    out[tag] = (payload, options) =>
      Effect.flatMap(counted(tag, payload, options.client.id), (c) =>
        Effect.flatMap(handler(c.payload, optionsOf(options, c)) as Effect.Effect<unknown, FinError>, after(tag, c.principal, c.openedProject)),
      );
  }
  return out as H;
}
