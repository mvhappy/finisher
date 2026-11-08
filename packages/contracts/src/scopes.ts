// The closed scope vocabulary (spec §4.5, plan 5A Decision 18), every RPC's rule (`RPC_SCOPES`:
// its scopes, all required; its resolvers; its escalations; the payload fields naming the
// branch it writes) and the human-only scopes (spec §4.6). Defaults per principal kind are
// `defaultScopes` in `auth.ts` (Decision 20).
import { Schema } from "effect";
import type { Rpc, RpcGroup } from "effect/rpc";
import type { FinRpcs } from "./rpc.ts";

export const SCOPES = [
  "public",
  "server.read",
  "project.read",
  "project.create",
  "project.write",
  "project.admin",
  "project.confirm",
  "fonts.fetch",
  "branch.protected",
  "review.write",
  "review.decide",
  "jobs.read",
  "jobs.submit",
  "jobs.control",
  "library.read",
  "library.scan",
  "library.admin",
  "diagnostics.read",
  "diagnostics.record",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.install",
] as const;

export const ScopeSchema = Schema.Literals(SCOPES);
export type Scope = typeof ScopeSchema.Type;

/** Scopes only a person holds (spec §4.6): never `local`, `agent` or `mcp`. */
export const HUMAN_ONLY: ReadonlySet<Scope> = new Set<Scope>([
  "project.confirm",
  "library.admin",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.install",
]);

export type FinRpcTag = RpcGroup.Rpcs<typeof FinRpcs>["_tag"];

/**
 * What the chain resolves before it authorises (spec §4.4 stage 3):
 * - `project`: the payload's project (`projectId`, `fonts.fetch.pinned`), checked for reach;
 * - `openDir` / `createDir`: `dir`, canonicalised; `openDir` reads its project's id
 *   (`finisher.project.json`) and whether that project is open here;
 * - `probePath` / `importDir`: the path, canonical and absolute (`INVALID_PATH` otherwise);
 * - `branch`: the payload's branch references (a name, an id or `name#id`), as ids through
 *   the store, when a policy reads them (5A: the write targets of `agent`/`mcp` principals
 *   with branch limits; 5E: protected branches).
 */
export type ResolverName = "project" | "openDir" | "createDir" | "probePath" | "importDir" | "branch";

/** Resolved facts an escalation may read. */
export interface RuleFacts {
  /** `project.open`: the folder's project is already open on this server (attaching). */
  readonly attached?: boolean;
  /**
   * A project the call names by id is not open here, so serving it reopens it from disk
   * (taking its lease): that needs `project.write` whatever the RPC's own scopes (Decision 15).
   */
  readonly reopens?: boolean;
}

/** The decoded payload of RPC `T`. */
export type PayloadOf<T extends FinRpcTag> = Rpc.Payload<Rpc.ExtractTag<RpcGroup.Rpcs<typeof FinRpcs>, T>>;

/** The rule of RPC `T`: its escalation reads `T`'s own payload type. */
export interface RuleOf<T extends FinRpcTag> {
  /** Required, all of them (Decision 18). */
  readonly scopes: ReadonlyArray<Scope>;
  readonly resolve?: ReadonlyArray<ResolverName>;
  /** More scopes this call needs, from its payload and the resolved facts (Decision 21). */
  readonly escalate?: (payload: PayloadOf<T>, facts: RuleFacts) => ReadonlyArray<Scope>;
  /**
   * Set exactly on the RPCs that write (every call whose handler passes a write guard to the
   * store; `WRITE_RPCS`): the payload fields (dotted paths) naming a branch the call writes to,
   * for the policy stage. `[]`: it writes, but names no existing branch (a new branch, a
   * recovery): the guard still limits what it may change.
   */
  readonly writes?: ReadonlyArray<string>;
  /**
   * Branches the call changes state on without writing a record through the store's guard
   * (`project.acknowledgeRewind`): the payload fields naming them, checked by the policy stage
   * like `writes` (agent branch limits; review of Task 6, finding 7).
   */
  readonly targets?: ReadonlyArray<string>;
  /**
   * The payload field holding the author a new branch records (`createdBy`): for a principal
   * with branch limits the server sets it to the principal's own author, so `"own"` cannot be
   * claimed by naming someone else (review of Task 5, M4).
   */
  readonly author?: string;
  /**
   * Whenever a call needs a human-only scope, a `console` principal must send the typed code
   * of a fresh `console.challenge` of the same connection (`challenge`; Decision 30), or it is
   * `CONFIRMATION_REQUIRED`: on by default (owner decision, review of Task 7, L2). A rule opts
   * out only as `readOnly` (it changes nothing: reading content, asking for a code, listing the
   * audit) or `service` (`server.stop`, which service managers call without a terminal, spec
   * §4.7).
   */
  readonly unconfirmed?: "readOnly" | "service";
  /**
   * True for a payload only a `device` may send (the app's onboarding question: training on).
   * Every other principal is `CONSENT_UI_ONLY`, before the human-only check (Decision 21).
   */
  readonly deviceOnly?: (payload: PayloadOf<T>) => boolean;
}

/** A rule with its RPC's types erased, as the chain reads it. */
export interface RpcRule {
  readonly scopes: ReadonlyArray<Scope>;
  readonly resolve?: ReadonlyArray<ResolverName>;
  readonly escalate?: (payload: never, facts: RuleFacts) => ReadonlyArray<Scope>;
  readonly writes?: ReadonlyArray<string>;
  readonly targets?: ReadonlyArray<string>;
  readonly author?: string;
  readonly unconfirmed?: "readOnly" | "service";
  readonly deviceOnly?: (payload: never) => boolean;
}

const PROJECT: ReadonlyArray<ResolverName> = ["project"];
const BRANCH: ReadonlyArray<ResolverName> = ["project", "branch"];

/**
 * Every RPC's rule (spec §4.5 and §4.8); the mapped type makes a missing RPC a type error and
 * types each escalation by its RPC's payload; `scopes.test.ts` compares the keys with `FinRpcs`
 * and the writers with `WRITE_RPCS`.
 */
export const RPC_SCOPES: { readonly [T in FinRpcTag]: RuleOf<T> } = {
  "server.hello": { scopes: ["public"] },
  "auth.prove": { scopes: ["public"] },
  "auth.ticket": { scopes: ["server.read"] },
  "auth.whoami": { scopes: ["server.read"] },
  // No id: a credential can revoke only itself.
  "auth.revokeSelf": { scopes: ["server.read"] },
  // The console's typed confirmations (Decision 30); the handler serves `console` principals only.
  "console.challenge": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  // 5A Task 8. Stopping is the owner's, but service managers call it without a terminal.
  "server.status": { scopes: ["server.read"] },
  "server.stop": { scopes: ["auth.admin"], unconfirmed: "service" },
  "auth.audit.list": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.audit.purge": { scopes: ["auth.admin"] },
  // 5A Task 9: minting and revoking are the owner's, with a typed code; listing reads only.
  "auth.credential.mint": { scopes: ["auth.admin"] },
  "auth.credential.list": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.credential.revoke": { scopes: ["auth.admin"] },
  "project.create": { scopes: ["project.create"], resolve: ["createDir"] },
  // Attaching to a project already open here only reads it; loading one may take its lease.
  "project.open": {
    scopes: ["project.read"],
    resolve: ["openDir"],
    escalate: (p, facts) => [...(facts.attached === true ? [] : (["project.write"] as const)), ...(p.relocate === true ? (["project.confirm"] as const) : [])],
  },
  // Protocol 2: the probe is for `projectId`, and a principal with roots probes only inside its folder.
  "asset.probe": { scopes: ["project.write"], resolve: ["project", "probePath"] },
  "tx.commit": { scopes: ["project.write"], resolve: BRANCH, writes: ["tx.branch"] },
  "snapshot.get": { scopes: ["project.read"], resolve: BRANCH },
  "ops.subscribe": { scopes: ["project.read"], resolve: BRANCH },
  "project.status": { scopes: ["project.read"], resolve: PROJECT },
  "project.takeover": { scopes: ["project.admin"], resolve: PROJECT, escalate: (p) => (p.confirm ? ["project.confirm"] : []) },
  "project.verify": { scopes: ["project.read"], resolve: PROJECT },
  "project.rebuild": { scopes: ["project.admin"], resolve: PROJECT },
  "project.compact": { scopes: ["project.admin"], resolve: PROJECT },
  // Recovery makes new branches and may re-apply a `branchSet` to a live one: the guard limits it.
  "project.recover": { scopes: ["project.admin"], resolve: ["project", "importDir"], writes: [], author: "author" },
  // A new branch is the writer's own; its parent is only read.
  "branch.create": { scopes: ["project.write"], resolve: BRANCH, writes: [], author: "author" },
  "branch.list": { scopes: ["project.read"], resolve: PROJECT },
  "branch.set": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.undo": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.redo": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.checkpoint": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.merge": { scopes: ["project.write"], resolve: BRANCH, writes: ["into"] },
  "history.cherryPick": { scopes: ["project.write"], resolve: BRANCH, writes: ["into"] },
  "history.revert": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  // Clears a branch's rewind warning: a branch-limited principal only on its own branches.
  "project.acknowledgeRewind": { scopes: ["project.write"], resolve: BRANCH, targets: ["branch"] },
  "project.acknowledgeSalvage": { scopes: ["project.write"], resolve: PROJECT },
  "project.events": { scopes: ["project.read"], resolve: PROJECT },
  "tx.get": { scopes: ["project.read"], resolve: PROJECT },
  "eval.frame": { scopes: ["project.read"], resolve: BRANCH },
  "eval.audio": { scopes: ["project.read"], resolve: BRANCH },
  "fonts.catalog": { scopes: ["server.read"] },
  "fonts.status": { scopes: ["server.read"] },
  "fonts.fetch": { scopes: ["fonts.fetch"], resolve: PROJECT },
  "fonts.plan": { scopes: ["project.read"], resolve: BRANCH },
  "diagnostics.status": { scopes: ["diagnostics.read"] },
  // Redacted for a caller that is not a person; the content itself is human-only (Decision 22).
  "diagnostics.export": {
    scopes: ["diagnostics.read"],
    escalate: (p) => (p.content === true ? ["diagnostics.admin"] : []),
    unconfirmed: "readOnly",
  },
  "diagnostics.purge": { scopes: ["diagnostics.admin"] },
  // Setting `never` stays open; clearing it binds every collaborator.
  "diagnostics.project": {
    scopes: ["diagnostics.read"],
    resolve: PROJECT,
    escalate: (p) => (p.collect === "default" ? ["diagnostics.admin"] : []),
  },
  // Every consent change needs a person; training on, the app's onboarding question alone.
  "diagnostics.consent": {
    scopes: ["diagnostics.admin"],
    deviceOnly: (p) => p.purpose === "training" && p.value === "on",
  },
  "diagnostics.record": { scopes: ["diagnostics.record"] },
};

/** The RPCs that write through the store's guard: exactly the rules with `writes`. */
export const WRITE_RPCS: ReadonlySet<FinRpcTag> = new Set(
  (Object.keys(RPC_SCOPES) as FinRpcTag[]).filter((t) => (RPC_SCOPES[t] as RpcRule).writes !== undefined),
);

/** `tag`'s rule with its types erased. */
export function ruleOf(tag: FinRpcTag): RpcRule {
  return RPC_SCOPES[tag] as RpcRule;
}

/** The scopes `tag` needs before any escalation (its rule's own). */
export function baseScopes(tag: FinRpcTag): ReadonlyArray<Scope> {
  return ruleOf(tag).scopes;
}

/** The scopes `tag` needs beyond its own for `payload` and the resolved `facts`. */
export function escalatedScopes(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): ReadonlyArray<Scope> {
  const r = ruleOf(tag);
  const more = [...(r.escalate?.(payload as never, facts) ?? []), ...(facts.reopens === true ? (["project.write"] as const) : [])];
  return [...new Set(more)].filter((s) => !r.scopes.includes(s));
}

/**
 * Whether a `console` principal's call must carry its connection's typed code: it needs a
 * human-only scope and its rule does not opt out (`unconfirmed`).
 */
export function needsConfirmation(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): boolean {
  return ruleOf(tag).unconfirmed === undefined && requiredScopes(tag, payload, facts).some((s) => HUMAN_ONLY.has(s));
}

/** Every scope `tag` needs for `payload`, given the resolved `facts`. */
export function requiredScopes(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): ReadonlyArray<Scope> {
  return [...new Set([...baseScopes(tag), ...escalatedScopes(tag, payload, facts)])];
}
