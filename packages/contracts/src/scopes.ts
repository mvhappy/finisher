// The closed scope vocabulary (spec §4.5, plan 5A Decision 18), every RPC's rule (`RPC_SCOPES`:
// its scopes, all required; its resolvers; its escalations; the payload fields naming the
// branch it writes) and the human-only scopes (spec §4.6). Defaults per principal kind are
// `defaultScopes` in `auth.ts` (Decision 20).
import { Schema } from "effect";
import type { RpcGroup } from "effect/rpc";
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
}

export interface RpcRule {
  /** Required, all of them (Decision 18). */
  readonly scopes: ReadonlyArray<Scope>;
  readonly resolve?: ReadonlyArray<ResolverName>;
  /** More scopes this call needs, from its payload and the resolved facts (Decision 21). */
  readonly escalate?: (payload: never, facts: RuleFacts) => ReadonlyArray<Scope>;
  /** Payload fields (dotted paths) naming a branch the call writes to: policy and the guard. */
  readonly writes?: ReadonlyArray<string>;
}

const rule = <P = unknown>(
  scopes: ReadonlyArray<Scope>,
  more: { resolve?: ReadonlyArray<ResolverName>; escalate?: (payload: P, facts: RuleFacts) => ReadonlyArray<Scope>; writes?: ReadonlyArray<string> } = {},
): RpcRule => ({ scopes, ...more }) as RpcRule;

const PROJECT: ReadonlyArray<ResolverName> = ["project"];
const BRANCH: ReadonlyArray<ResolverName> = ["project", "branch"];

/**
 * Every RPC's rule (spec §4.5 and §4.8); the mapped type makes a missing RPC a type error and
 * `scopes.test.ts` compares the keys with `FinRpcs`.
 *
 * Until 5A Task 7 the diagnostics RPCs keep the scopes every `local` caller holds
 * (`diagnostics.read`): Task 7 makes `diagnostics.consent`, `diagnostics.purge`,
 * `diagnostics.project {collect: "default"}` and content exports need `diagnostics.admin`, moving
 * their tests to the console in the same commit. No `agent`/`mcp` credential exists before Task 9.
 */
export const RPC_SCOPES: { readonly [T in FinRpcTag]: RpcRule } = {
  "server.hello": rule(["public"]),
  "auth.prove": rule(["public"]),
  "project.create": rule(["project.create"], { resolve: ["createDir"] }),
  // Attaching to a project already open here only reads it; loading one may take its lease.
  "project.open": rule<{ relocate?: boolean }>(["project.read"], {
    resolve: ["openDir"],
    escalate: (p, facts) => [...(facts.attached === true ? [] : (["project.write"] as const)), ...(p.relocate === true ? (["project.confirm"] as const) : [])],
  }),
  // Protocol 2 (Task 6) adds `projectId`; until then the path's reach is the principal's projects.
  "asset.probe": rule(["project.write"], { resolve: ["probePath"] }),
  "tx.commit": rule(["project.write"], { resolve: BRANCH, writes: ["tx.branch"] }),
  "snapshot.get": rule(["project.read"], { resolve: BRANCH }),
  "ops.subscribe": rule(["project.read"], { resolve: BRANCH }),
  "project.status": rule(["project.read"], { resolve: PROJECT }),
  "project.takeover": rule<{ confirm: boolean }>(["project.admin"], {
    resolve: PROJECT,
    escalate: (p) => (p.confirm ? ["project.confirm"] : []),
  }),
  "project.verify": rule(["project.read"], { resolve: PROJECT }),
  "project.rebuild": rule(["project.admin"], { resolve: PROJECT }),
  "project.compact": rule(["project.admin"], { resolve: PROJECT }),
  // Recovery writes new branches only (never an existing one).
  "project.recover": rule(["project.admin"], { resolve: ["project", "importDir"] }),
  // A new branch is the writer's own; its parent is only read.
  "branch.create": rule(["project.write"], { resolve: BRANCH }),
  "branch.list": rule(["project.read"], { resolve: PROJECT }),
  "branch.set": rule(["project.write"], { resolve: BRANCH, writes: ["branch"] }),
  "history.undo": rule(["project.write"], { resolve: BRANCH, writes: ["branch"] }),
  "history.redo": rule(["project.write"], { resolve: BRANCH, writes: ["branch"] }),
  "history.checkpoint": rule(["project.write"], { resolve: BRANCH, writes: ["branch"] }),
  "history.merge": rule(["project.write"], { resolve: BRANCH, writes: ["into"] }),
  "history.cherryPick": rule(["project.write"], { resolve: BRANCH, writes: ["into"] }),
  "history.revert": rule(["project.write"], { resolve: BRANCH, writes: ["branch"] }),
  "project.acknowledgeRewind": rule(["project.write"], { resolve: BRANCH }),
  "project.acknowledgeSalvage": rule(["project.write"], { resolve: PROJECT }),
  "project.events": rule(["project.read"], { resolve: PROJECT }),
  "tx.get": rule(["project.read"], { resolve: PROJECT }),
  "eval.frame": rule(["project.read"], { resolve: BRANCH }),
  "eval.audio": rule(["project.read"], { resolve: BRANCH }),
  "fonts.catalog": rule(["server.read"]),
  "fonts.status": rule(["server.read"]),
  "fonts.fetch": rule(["fonts.fetch"], { resolve: PROJECT }),
  "fonts.plan": rule(["project.read"], { resolve: BRANCH }),
  "diagnostics.status": rule(["diagnostics.read"]),
  "diagnostics.export": rule(["diagnostics.read"]),
  "diagnostics.purge": rule(["diagnostics.read"]),
  "diagnostics.project": rule(["diagnostics.read"], { resolve: PROJECT }),
  "diagnostics.consent": rule(["diagnostics.read"]),
  "diagnostics.record": rule(["diagnostics.record"]),
};

/** Every scope `tag` needs for `payload`, given the resolved `facts`. */
export function requiredScopes(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): ReadonlyArray<Scope> {
  const r = RPC_SCOPES[tag];
  const more = r.escalate?.(payload as never, facts) ?? [];
  return [...new Set([...r.scopes, ...more])];
}
