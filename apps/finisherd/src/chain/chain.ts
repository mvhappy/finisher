// The middleware chain (spec §4.4, plan 5A Decision 14): every handler is wrapped once, unary
// and stream alike, so that each request passes, in this order:
//   1. decode (the protocol, `protocol.ts`): `INVALID_REQUEST`;
//   2. authenticate: anything but the public RPCs from `anonymous` is `UNAUTHENTICATED`;
//   3. resolve (`resolve.ts`): projects, directories and paths become current facts;
//   4. authorise (`authorize.ts`): scopes (all), human-only, reach;
//   5. policy (`policy.ts`): branch write targets resolved through the store, then
//      `allow {guard} | deny | ask` (`ask` answers `APPROVAL_REQUIRED {approvalId}`; an agent
//      session's rule ask is durable instead, `APPROVAL_PENDING`, through `agentAsk`); then a
//      `console` principal's mutating human-only call must carry its connection's typed
//      challenge (`auth/challenges.ts`, Decision 30: `CONFIRMATION_REQUIRED`);
//   6. the handler, with the resolved payload and the guard (`options.guard`), which every
//      write passes into `fin-node` for the store to check inside its write transaction; when
//      it fails with a typed error that committed nothing (`UNCOMMITTED`), what the policy
//      stage took for the call is given back (`Outcome.undo`: a one-time grant, 7B);
//   7. after (sub-project 7's hooks; nothing in 5A but `project.open`'s reach re-check).
// A refusal at stages 2–5 is audited (`auth.denied`: tag, code, scope or rule, principal kind;
// never the payload; a missing confirmation is `console.refused`) and counted in `rpc.stats`
// (`timed`), never journalled.
import {
  type FinError,
  type FinRpcTag,
  needsConfirmation,
  PUBLIC_RPCS,
  type Principal,
  type RpcRule,
  type RuleFacts,
  ruleOf,
  STREAM_RPCS,
} from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Effect, Stream } from "effect";
import { answerChallenge, confirmationRequired, undoesOwnMint } from "../auth/challenges.ts";
import { unauthenticated } from "../auth/handshake.ts";
import { credentialRevoked } from "../auth/revoke.ts";
import type { Connections, Transport } from "../connections.ts";
import type { Audit } from "../control/audit.ts";
import { ulid } from "../control/ulid.ts";
import { authorize } from "./authorize.ts";
import { withoutClientVia } from "./client-via.ts";
import type { ChainHooks } from "./hooks.ts";
import { approvalRequired, outOfReach } from "./fixes.ts";
import { type Ask, authorOf, branchLimited, DEFAULT_POLICIES, type PolicyInput, type PolicyRule, runPolicies, stampsAuthor } from "./policy.ts";
import { type ChainJobs, type ChainProjects, resolve } from "./resolve.ts";

export type { Ask, Outcome, PolicyRule } from "./policy.ts";

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
    if (credentialOf(c.principal) !== c.guard.credential) {
      return Effect.die(new Error("a credential's write does not name its credential"));
    }
    return Effect.succeed(c.guard);
  });
}

/**
 * The credential a write is made for (`<kind>:<id>`), for `agent`/`mcp` principals: fin-store
 * refuses it once the credential is revoked, inside the write's transaction (Task 9; review of
 * Task 7, M3). `undefined` for every other principal.
 */
export function credentialOf(p: Principal): string | undefined {
  return p.kind === "agent" || p.kind === "mcp" ? `${p.kind}:${p.id}` : undefined;
}

/**
 * `payload` with `author` as the author of what it writes: `tx.commit`'s `tx.author`, the
 * `author` of every other write (history calls, a new branch's `createdBy`, recovery's).
 */
export function stampAuthor(tag: FinRpcTag, rule: RpcRule, payload: unknown, author: string): unknown {
  if (payload === null || typeof payload !== "object") return payload;
  const p = payload as Record<string, unknown>;
  if (tag === "tx.commit") {
    return p.tx !== null && typeof p.tx === "object" ? { ...p, tx: { ...(p.tx as Record<string, unknown>), author } } : p;
  }
  if (rule.writes === undefined && rule.author === undefined) return p;
  return { ...p, [rule.author ?? "author"]: author };
}

/** Approvals kept in memory at most, and for how long (review of Task 5, L6). */
export const MAX_APPROVALS = 256;
export const APPROVAL_TTL_MS = 10 * 60_000;
/**
 * Approvals one principal may have pending (review of Tasks 8–9, L5): past it a new one drops
 * that principal's oldest, so one agent asking in a loop cannot push everyone else's out.
 */
export const MAX_APPROVALS_PER_PRINCIPAL = 16;

type Approvals = Map<string, { readonly tag: string; readonly principal: string; readonly at: number }>;

/**
 * Records an approval asked for, dropping expired ones, then the principal's oldest past its
 * quota, then the oldest of all past the cap.
 */
export function keepApproval(approvals: Approvals, id: string, entry: { readonly tag: string; readonly principal: string; readonly at: number }): void {
  for (const [k, v] of approvals) {
    if (entry.at - v.at < APPROVAL_TTL_MS) break;
    approvals.delete(k);
  }
  const mine = [...approvals].filter(([, v]) => v.principal === entry.principal);
  for (const [k] of mine.slice(0, Math.max(0, mine.length - MAX_APPROVALS_PER_PRINCIPAL + 1))) approvals.delete(k);
  while (approvals.size >= MAX_APPROVALS) approvals.delete(approvals.keys().next().value as string);
  approvals.set(id, entry);
}

/**
 * Sub-project 7B's durable ask (Decision 8; `agent/approvals.ts`): the refusal an `ask` answers
 * instead of 5A's (`APPROVAL_PENDING`), or `undefined` to keep 5A's in-memory ask; a failure
 * (`APPROVAL_LIMIT`) is the call's answer.
 */
export type AgentAsk = (input: PolicyInput & { readonly ask: Ask }) => Effect.Effect<FinError | undefined, FinError>;

export interface ChainDeps {
  readonly connections: Connections;
  readonly projects: ChainProjects;
  /** The job engine's ownership and kind scopes, for the `jobs.*` rules (5C-2). */
  readonly jobs?: ChainJobs;
  readonly audit?: Audit;
  /** Counts a unary refusal in `rpc.stats`. */
  readonly timed?: <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
  /** The policy rules, in order (default: agent branch limits). */
  readonly policies?: ReadonlyArray<PolicyRule>;
  /**
   * Approvals asked for (`ask`), by id, in memory, oldest first: at most `MAX_APPROVALS`, each
   * for `APPROVAL_TTL_MS` (sub-project 7 owns approving them and may keep them longer).
   */
  readonly approvals?: Approvals;
  /** Notes a request of an `agent`/`mcp` credential (its idle expiry, Task 9). */
  readonly touch?: (p: Principal) => void;
  readonly clock?: () => number;
  /** An agent session's durable ask (7B Task 4); none: every ask is 5A's. */
  readonly agentAsk?: AgentAsk;
  /** What the chain asks of agent sessions (sub-project 7A Task 7); none: 5A's chain. */
  readonly hooks?: ChainHooks;
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

/**
 * Handler failures after which a write may still have committed (a panic, an engine closed by
 * one): what the policy stage took stays taken (a lost grant, never a doubled write).
 */
const MAYBE_COMMITTED: ReadonlySet<string> = new Set(["INTERNAL", "ENGINE_CLOSED"]);

/** The codes stages 2–5 refuse with, audited as `auth.denied` with the rule that refused. */
const DENIALS = new Map<string, string>([
  ["UNAUTHENTICATED", "authenticate"],
  ["CREDENTIAL_REVOKED", "revoked"],
  ["HUMAN_ONLY", "humanOnly"],
  ["SCOPE_DENIED", "scopes"],
  ["PROJECT_NOT_FOUND", "reach"],
  ["PATH_NOT_ALLOWED", "reach"],
  ["BRANCH_NOT_ALLOWED", "agentBranchLimits"],
  ["BRANCH_PROTECTED", "protectedBranches"],
  ["APPROVAL_REQUIRED", "ask"],
  ["APPROVAL_PENDING", "ask"],
  ["APPROVAL_LIMIT", "ask"],
  ["RULE_DENIED", "commandRules"],
  ["CONSENT_UI_ONLY", "deviceOnly"],
  ["CONFIRMATION_REQUIRED", "confirmation"],
]);

/** `handlers` with the chain in front of each one. */
export function withChain<H extends object>(handlers: H, deps: ChainDeps): H {
  const streams = new Set<string>(STREAM_RPCS);
  const policies = deps.policies ?? DEFAULT_POLICIES;
  const approvals: Approvals = deps.approvals ?? new Map();
  const timed = deps.timed ?? ((_, self) => self);
  const hooks = deps.hooks ?? {};
  const clock = deps.clock ?? Date.now;

  const deny = (tag: string, principal: Principal, transport: Transport) => (e: FinError) =>
    Effect.sync(() => {
      const rule = DENIALS.get(e.code);
      if (rule === undefined) return;
      const d = (e.details ?? {}) as Record<string, unknown>;
      deps.audit?.append({
        kind: e.code === "CONFIRMATION_REQUIRED" ? "console.refused" : "auth.denied",
        principal: { kind: principal.kind, id: principal.id },
        // `APPROVAL_PENDING` is not a refusal: the request waits on a person's card.
        outcome: e.code === "APPROVAL_PENDING" ? "pending" : "denied",
        code: e.code,
        details: {
          rpc: tag,
          transport,
          rule,
          ...(e.code === "APPROVAL_PENDING" && typeof d.approval === "string" ? { approval: d.approval } : {}),
          ...(typeof d.scope === "string" ? { scope: d.scope } : {}),
          ...(Array.isArray(d.scopes) ? { scopes: d.scopes } : {}),
          ...(typeof d.reason === "string" && /^[a-z][a-zA-Z0-9]{0,47}$/.test(d.reason) ? { reason: d.reason } : {}),
        },
      });
    });

  /** Stages 2–5; the resolved payload and the guard, or the refusal. */
  const check = (tag: FinRpcTag, sent: unknown, clientId: number, headers: unknown) => {
    // Before anything reads it: a client's `via` would name a plugin it is not (spec §3.8).
    const payload = withoutClientVia(tag, sent);
    const principal = deps.connections.principalOf(clientId);
    const rule = ruleOf(tag);
    const stages = Effect.gen(function* () {
      // A credential revoked after this connection proved it (or took its ticket) runs nothing (M3).
      if (deps.connections.revoked(clientId)) return yield* Effect.fail(credentialRevoked());
      if (!PUBLIC_RPCS.has(tag) && principal.kind === "anonymous") return yield* Effect.fail(unauthenticated("unproved"));
      if (credentialOf(principal) !== undefined) deps.touch?.(principal);
      const resolved = yield* resolve(rule, payload, deps.projects, { principal, jobs: deps.jobs });
      yield* authorize(tag, principal, payload, resolved, deps.projects, hooks);
      const input: PolicyInput = { tag, rule, principal, payload: resolved.payload, projects: deps.projects, projectIds: resolved.projects, headers };
      const outcome = yield* runPolicies(policies, input);
      if ("deny" in outcome) return yield* Effect.fail(outcome.deny);
      if ("ask" in outcome) {
        const durable = deps.agentAsk === undefined ? undefined : yield* deps.agentAsk({ ...input, ask: outcome.ask });
        if (durable !== undefined) return yield* Effect.fail(durable);
        const approvalId = ulid();
        keepApproval(approvals, approvalId, { tag, principal: `${principal.kind}:${principal.id}`, at: clock() });
        return yield* Effect.fail(approvalRequired(tag, approvalId, outcome.ask.reason));
      }
      // Last, so that a challenge code is used up only by a call that goes ahead.
      // Every call that needs a human-only scope is confirmed, unless its rule opts out (L2).
      // A connection taking back a credential it minted needs no second code (M2 of the review of Tasks 8–9).
      const conn = deps.connections.get(clientId);
      if (principal.kind === "console" && needsConfirmation(tag, payload, resolved.facts) && !undoesOwnMint(conn, tag, payload)) {
        const refusal = answerChallenge(conn, (payload as { challenge?: unknown } | null)?.challenge);
        if (refusal !== undefined) return yield* Effect.fail(confirmationRequired(tag, refusal));
      }
      // A `service` opt-out (`server.stop`) is for the console that service managers run; any
      // other person's principal (5B's devices with `auth.admin`) must confirm it, and until
      // 5B has a way to, it cannot (review of Tasks 8–9, Task 8).
      if (rule.unconfirmed === "service" && principal.human && principal.kind !== "console") {
        return yield* Effect.fail(confirmationRequired(tag, "missing"));
      }
      // A credential's writes carry its own author, whatever it sent (M4; review of Tasks 8–9,
      // M1: whatever its branch limits, or it could plant an `own` branch for another; 7A
      // Decision 14d: its records too, so `FIN_AUTHOR` cannot speak for someone else).
      const stamped = stampsAuthor(principal)
        ? stampAuthor(tag, rule, resolved.payload, hooks.authorOf?.(principal) ?? authorOf(principal))
        : resolved.payload;
      const credential = credentialOf(principal);
      const guard: WriteGuard = credential === undefined ? outcome.guard : { ...outcome.guard, credential };
      return { payload: stamped, guard, principal, facts: resolved.facts, openedProject: resolved.openedProject, undo: outcome.undo };
    });
    return Effect.tapError(stages, deny(tag, principal, deps.connections.transport));
  };

  /**
   * Stage 7: `project.open` opened the project its folder named when it was checked; then the
   * hooks' own (an agent session's writes recorded under it).
   */
  const after = (tag: FinRpcTag, c: Checked, headers: unknown) => (result: unknown) => {
    const recorded = hooks.after === undefined ? Effect.succeed(result) : Effect.as(hooks.after(tag, c.principal, c.payload, result, headers), result);
    if (tag !== "project.open" || c.principal.projects === "*") return recorded;
    const got = (result as { projectId?: unknown } | null)?.projectId;
    return got === c.openedProject ? recorded : Effect.fail(outOfReach(typeof got === "string" ? got : undefined));
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
  const counted = (tag: FinRpcTag, payload: unknown, clientId: number, headers: unknown) =>
    Effect.catchCause(check(tag, payload, clientId, headers), (cause) => timed(tag, Effect.failCause(cause)));
  /** The handler's typed failure gives back what the policy stage took (unless it may have committed). */
  const undoing = (c: Checked, self: Effect.Effect<unknown, FinError>) =>
    c.undo === undefined ? self : Effect.tapError(self, (e) => (MAYBE_COMMITTED.has(e.code) ? Effect.void : c.undo!));

  const out: Record<string, Handler> = {};
  for (const [tag, handler] of Object.entries(handlers) as Array<[FinRpcTag, Handler]>) {
    if (streams.has(tag)) {
      out[tag] = (payload, options) =>
        Stream.unwrap(
          Effect.map(counted(tag, payload, options.client.id, options.headers), (c) => handler(c.payload, optionsOf(options, c)) as Stream.Stream<unknown, FinError>),
        );
      continue;
    }
    out[tag] = (payload, options) =>
      Effect.flatMap(counted(tag, payload, options.client.id, options.headers), (c) =>
        Effect.flatMap(undoing(c, handler(c.payload, optionsOf(options, c)) as Effect.Effect<unknown, FinError>), after(tag, c, options.headers)),
      );
  }
  return out as H;
}
