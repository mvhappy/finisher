// Branch reach and path reach of an agent session's credential (sub-project 7A Task 7,
// Decisions 14d, 16, 17; spec §5.1, §5.2, §5.6), checked by 5A's chain through its hooks
// (`chain/hooks.ts`) and its policy stage:
// - **Branch reach:** the credential's principal is the session (its id is the session id). It
//   writes its root branch and the branches beneath it, nothing else: the target of every
//   write RPC (`tx.commit`'s branch, `history.undo|redo|checkpoint|revert`'s, the `into` of
//   `history.merge` and `history.cherryPick`, `branch.set`'s, `project.acknowledgeRewind`'s)
//   and the parent of `branch.create` must be in that subtree, else `BRANCH_NOT_ALLOWED`. The
//   guard the store checks inside the write's transaction allows exactly the subtree. Reads
//   are not limited by branch. A `plan` session has no branch and writes none.
// - **Lineage** comes from the engine's branch index (`branch.list`), cached per project. A
//   branch's parent never changes and ids are never reused, so the cache can only miss new
//   branches: a target it does not know makes it read the index again once.
// - **Path reach:** the project folder (with `workspace/<sessionId>/` in it) and the roots of
//   the libraries the project references; `project.open` and `project.create` are refused
//   (`SCOPE_DENIED`): the session's project is opened by `finisherd` at start.
// - **Nothing outside its branches changes:** `project.acknowledgeSalvage` (project state, no
//   branch) and the diagnostics settings (`diagnostics.project`, `.consent`, `.purge`) are
//   refused to sessions too (`SCOPE_DENIED {reason: agentSession}`); their reads stay open.
// - **Children (7B Decisions 16, 18):** a session drives its own children only: a
//   `agent.session.prompt|steer|interrupt|end|wait` naming any other session (a sibling, its
//   parent, another tree's) is `SESSION_NOT_FOUND`, as a missing one; an `agent` credential
//   that is no session's drives none (`SCOPE_DENIED {reason: noSession}`). A live session's
//   root branch and the branches beneath it are that session's alone: every other session's
//   reach (its parent's, whose subtree holds it) excludes them until it ends; reads stay open,
//   so a parent merges a child's branch into its own.
// - **Reviews (7B Task 11):** a session's `review.request` and `review.refresh` name a source
//   in that reach (`BRANCH_NOT_ALLOWED` otherwise); comments and reads stay project-wide.
// - **Rollback (7B Decision 23):** a session rolls back its own direct children only:
//   `agent.session.rollback` naming itself, a sibling, a grandchild or another tree's session
//   (or any session, from an `agent` credential that is no session's) is `SCOPE_DENIED`.
// - **Author and `session_txs`:** every write carries the session's author
//   (`agent:<role>:<sessionId>`), whatever it sent, and every commit, undo, redo, revert,
//   cherry-pick and merge it makes is recorded in `session_txs` after the engine answers,
//   whatever its call attribution: `exact` with a `FIN_CALL`; without one (a CLI agent's own
//   shell) what `claude/attribution.ts` infers from the `fin-args-hash` header (7D Task 12).
import { join } from "node:path";
import type { FinError, FinRpcTag, Principal, Session } from "@finisher/contracts";
import { ruleOf } from "@finisher/contracts";
import { Effect } from "effect";
import type { ChainHooks } from "../chain/hooks.ts";
import { branchNotAllowed } from "../chain/fixes.ts";
import type { Outcome, PolicyRule } from "../chain/policy.ts";
import { canonicalPath, type ChainProjects, resolveWrites, rootsOf } from "../chain/resolve.ts";
import { actorFor, headerOf } from "../diagnostics.ts";
import { finError } from "../errors.ts";
import { committedOf } from "../history.ts";
import { ARGS_HASH_HEADER, type Matched } from "./claude/attribution.ts";
import { resumeDenied } from "./child-resume.ts";
import type { SessionTx } from "./store-tables.ts";

/** What reach reads of the agent store (`AgentStore` is one). */
export interface SessionSource {
  get(sessionId: string): Session | undefined;
  /** The root branches of a project's sessions that have not ended (`SessionTree.liveRoots`). */
  readonly tree?: { liveRoots(projectId: string): ReadonlyArray<{ readonly id: string; readonly rootBranch: string }> };
  recordTx(t: { txId: string; sessionId: string; callId: string | null; attribution: SessionTx["attribution"]; at: number }): void;
}

/** The writes recorded in `session_txs` (spec §5.2). */
export const SESSION_TX_RPCS: ReadonlySet<string> = new Set([
  "tx.commit",
  "history.undo",
  "history.redo",
  "history.revert",
  "history.cherryPick",
  "history.merge",
]);

/** The author of a session's records (Decision 17). */
export function sessionAuthor(s: { readonly role: string; readonly id: string }): string {
  return `agent:${s.role}:${s.id}`;
}

/** A call id as `FIN_CALL` takes it (`crates/fin/src/agent_env.rs`). */
const CALL_ID = /^[0-9A-Za-z._:-]{1,128}$/;

const projectOf = (payload: unknown): string | undefined => {
  const v = (payload as { projectId?: unknown } | null)?.projectId;
  return typeof v === "string" ? v : undefined;
};

/** Each project's branches and their parents (`branch.list {all: true}`), read on demand. */
export class BranchLineage {
  readonly #parents = new Map<string, ReadonlyMap<string, string | null>>();

  #load(projects: ChainProjects, projectId: string): Effect.Effect<ReadonlyMap<string, string | null>, FinError> {
    return Effect.map(projects.branches(projectId), (bs) => {
      const m = new Map(bs.map((b) => [b.id, b.parent ?? null] as const));
      this.#parents.set(projectId, m);
      return m;
    });
  }

  /**
   * `root` and every branch beneath it. The cached index is read again when it lacks `root` or
   * one of `want` (a branch made since), so a new branch is never refused for being new.
   */
  subtree(projects: ChainProjects, projectId: string, root: string, want: ReadonlyArray<string> = []): Effect.Effect<ReadonlySet<string>, FinError> {
    return Effect.gen({ self: this }, function* () {
      const cached = this.#parents.get(projectId);
      let parents = cached ?? (yield* this.#load(projects, projectId));
      let sub = beneath(parents, root);
      if (cached !== undefined && (!sub.has(root) || want.some((b) => !sub.has(b)))) {
        parents = yield* this.#load(projects, projectId);
        sub = beneath(parents, root);
      }
      return sub;
    });
  }

  /** Drops a project's index (its engine closed or was rebuilt). */
  forget(projectId: string): void {
    this.#parents.delete(projectId);
  }
}

function beneath(parents: ReadonlyMap<string, string | null>, root: string): Set<string> {
  const out = new Set<string>();
  if (!parents.has(root)) return out;
  const children = new Map<string, string[]>();
  for (const [id, parent] of parents) {
    if (parent === null) continue;
    const list = children.get(parent);
    if (list === undefined) children.set(parent, [id]);
    else list.push(id);
  }
  const todo = [root];
  while (todo.length > 0) {
    const id = todo.pop()!;
    if (out.has(id)) continue;
    out.add(id);
    todo.push(...(children.get(id) ?? []));
  }
  return out;
}

export class BranchReach {
  readonly #sessions: SessionSource;
  readonly #lineage: BranchLineage;
  readonly #projects: ChainProjects;

  constructor(sessions: SessionSource, lineage: BranchLineage, projects: ChainProjects) {
    this.#sessions = sessions;
    this.#lineage = lineage;
    this.#projects = projects;
  }

  /** The session an `agent` principal is (its id is the session id); `undefined` for any other. */
  sessionOf(p: Principal): Session | undefined {
    return p.kind === "agent" ? this.#sessions.get(p.id) : undefined;
  }

  covers(p: Principal): boolean {
    return this.sessionOf(p) !== undefined;
  }

  /**
   * The branches a session's write may change (its subtree), once every branch the call writes
   * or acts on is in it; `BRANCH_NOT_ALLOWED` naming the first that is not. `undefined`: the
   * principal is not a session, or the call writes nothing.
   */
  allowed(principal: Principal, projectId: string | undefined, tag: FinRpcTag, payload: unknown): Effect.Effect<ReadonlyArray<string> | undefined, FinError> {
    return Effect.gen({ self: this }, function* () {
      const session = this.sessionOf(principal);
      const rule = ruleOf(tag);
      if (session !== undefined && REVIEW_SOURCE_RPCS.has(tag)) return yield* this.#reviewSource(session, projectId, tag, payload);
      if (session === undefined || (rule.writes === undefined && rule.targets === undefined)) return undefined;
      // Every writing RPC names its project, and reach kept it to the session's.
      if (projectId === undefined || projectId !== session.projectId || session.rootBranch === undefined) {
        return yield* Effect.fail(branchNotAllowed(tag, ""));
      }
      const targets = [...(yield* resolveWrites(rule, payload, this.#projects))];
      // A session forks new branches from its own only (the parent is read, but a branch made
      // from `main` would be outside its subtree from birth).
      const from = (payload as { from?: unknown } | null)?.from;
      if (tag === "branch.create" && typeof from === "string") targets.push(yield* this.#projects.resolveBranch(projectId, from));
      const sub = yield* this.#reach(session, targets);
      const outside = targets.find((t) => !sub.has(t));
      if (outside !== undefined) return yield* Effect.fail(branchNotAllowed(tag, outside));
      return [...sub];
    });
  }

  /**
   * A review the session asks for or refreshes (7B Task 11 review, S1): its source (the
   * request's `source`, the item's as the `review` resolver added it) must be in the session's
   * reach, else `BRANCH_NOT_ALLOWED`. Nothing is written, so no guard: `undefined` when it is.
   */
  #reviewSource(session: Session, projectId: string | undefined, tag: FinRpcTag, payload: unknown): Effect.Effect<undefined, FinError> {
    return Effect.gen({ self: this }, function* () {
      const source = (payload as { source?: unknown } | null)?.source;
      if (typeof source !== "string") return undefined;
      if (projectId === undefined || projectId !== session.projectId || session.rootBranch === undefined) return yield* Effect.fail(branchNotAllowed(tag, source));
      const id = yield* this.#projects.resolveBranch(projectId, source);
      const sub = yield* this.#reach(session, [id]);
      return sub.has(id) ? undefined : yield* Effect.fail(branchNotAllowed(tag, id));
    });
  }

  /**
   * `session`'s subtree without the subtrees rooted at other live sessions' roots (its live
   * children's, Decision 18). `want`: branches the call names (a new one reads the index again).
   */
  #reach(session: Session, want: ReadonlyArray<string>): Effect.Effect<Set<string>, FinError> {
    return Effect.gen({ self: this }, function* () {
      const projectId = session.projectId;
      const sub = new Set(yield* this.#lineage.subtree(this.#projects, projectId, session.rootBranch!, want));
      for (const other of this.#sessions.tree?.liveRoots(projectId) ?? []) {
        if (other.id === session.id || other.rootBranch === session.rootBranch || !sub.has(other.rootBranch)) continue;
        for (const b of yield* this.#lineage.subtree(this.#projects, projectId, other.rootBranch)) sub.delete(b);
      }
      return sub;
    });
  }

  /** The branches `session`'s writes reach (its subtree less its live children's): a rollback's guard (7B Task 8). */
  subtreeOf(session: Session): Effect.Effect<ReadonlyArray<string>, FinError> {
    return session.rootBranch === undefined ? Effect.succeed([]) : Effect.map(this.#reach(session, []), (s) => [...s]);
  }

  /** `branch` (a reference) as its id, when it is in `session`'s subtree; else `BRANCH_NOT_ALLOWED`. */
  inSubtree(session: Session, branch: string, tag = "agent.self.try"): Effect.Effect<string, FinError> {
    return Effect.gen({ self: this }, function* () {
      if (session.rootBranch === undefined) return yield* Effect.fail(branchNotAllowed(tag, branch));
      const id = yield* this.#projects.resolveBranch(session.projectId, branch);
      const sub = yield* this.#reach(session, [id]);
      return sub.has(id) ? id : yield* Effect.fail(branchNotAllowed(tag, id));
    });
  }

  /** The policy rule (stage 5): sessions' writes stay in their subtree, in the guard too. */
  readonly policy: PolicyRule = ({ tag, principal, payload }) =>
    Effect.map(this.allowed(principal, projectOf(payload), tag, payload), (allow): Outcome | undefined =>
      allow === undefined ? undefined : { allow: true, guard: { denyBranches: [], allowBranches: [...allow] } },
    );
}

/**
 * The libraries a project references: their roots, canonical (sub-project 5D owns the
 * registry; until a project names its libraries there are none).
 */
export type LibraryRoots = (projectId: string) => Effect.Effect<ReadonlyArray<string>>;

export class PathReach {
  readonly #sessions: SessionSource;
  readonly #projects: ChainProjects;
  readonly #libraries: LibraryRoots;

  constructor(sessions: SessionSource, projects: ChainProjects, libraries: LibraryRoots = () => Effect.succeed([])) {
    this.#sessions = sessions;
    this.#projects = projects;
    this.#libraries = libraries;
  }

  /** The roots of an `agent` principal that is a session, beyond its project folder: its project's libraries. */
  extra(principal: Principal, only: string | undefined): Effect.Effect<ReadonlyArray<string>> {
    const s = principal.kind === "agent" ? this.#sessions.get(principal.id) : undefined;
    if (s === undefined || (only !== undefined && only !== s.projectId)) return Effect.succeed([]);
    return Effect.flatMap(this.#libraries(s.projectId), (roots) => Effect.forEach(roots, (r) => Effect.promise(() => canonicalPath(r))));
  }

  /**
   * Every folder the session `principal` may name for `projectId`: the project folder, its
   * `workspace/<sessionId>/` and the referenced libraries' roots, canonical. Empty for a
   * principal that is not that project's session.
   */
  async roots(principal: Principal, projectId: string): Promise<string[]> {
    const s = principal.kind === "agent" ? this.#sessions.get(principal.id) : undefined;
    if (s === undefined || s.projectId !== projectId) return [];
    const folders = (await Effect.runPromise(rootsOf(principal, this.#projects, projectId))) ?? [];
    const libraries = await Effect.runPromise(this.extra(principal, projectId));
    const workspaces = await Promise.all(folders.map((f) => canonicalPath(join(f, "workspace", s.id))));
    return [...folders, ...workspaces, ...libraries];
  }
}

/**
 * The RPCs no session may call whatever its scopes (spec §5.6, §6.1): opening or creating a
 * project (the session's own is opened by `finisherd` at start) or closing one, and the calls that change
 * state outside any branch: a salvage acknowledgement and the diagnostics settings and consent.
 */
export const SESSION_REFUSED_RPCS: ReadonlySet<string> = new Set([
  "project.open",
  "project.create",
  "project.close",
  "project.acknowledgeSalvage",
  "diagnostics.project",
  "diagnostics.consent",
  "diagnostics.purge",
]);

/**
 * The `agent.read` RPCs a session may not call although its mode holds `agent.read` (7B Task
 * 5: it holds it for its own approval cards, `agent.approval.wait|list`): other sessions'
 * transcripts and state, and the rules, stay people's.
 */
export const SESSION_PEOPLE_READS: ReadonlySet<string> = new Set(["agent.session.get", "agent.watch", "agent.rules.list"]);

/**
 * The review RPCs whose source branch must be in a session's reach (7B Task 11 review, S1):
 * asking for a review and refreshing (or handing back) an item. `review.comment` and
 * `review.get` stay project-wide: a critic comments on the item it judges, outside its reach.
 */
export const REVIEW_SOURCE_RPCS: ReadonlySet<string> = new Set(["review.request", "review.refresh"]);

/**
 * The RPCs a session calls on its own children only (7B Decision 16): `sessionId`, or `ids` of a
 * wait. A resume is checked apart (`resumeCheck`): a child the parent interrupted itself only.
 */
export const CHILD_RPCS: ReadonlySet<string> = new Set(["agent.session.prompt", "agent.session.steer", "agent.session.interrupt", "agent.session.end", "agent.session.wait"]);

/**
 * Stage 4 for the session RPCs an agent may call (Decision 16): a session starts children and
 * drives its own; an `agent` credential that is no session's does neither.
 */
function childCheck(tag: string, principal: Principal, payload: unknown, deps: AgentHookDeps): Effect.Effect<void, FinError> {
  if (principal.kind === "agent" && tag === "agent.session.rollback") return rollbackCheck(principal, payload, deps);
  if (principal.kind === "agent" && tag === "agent.session.resume") return resumeCheck(principal, payload, deps);
  if (principal.kind !== "agent" || (tag !== "agent.session.start" && !CHILD_RPCS.has(tag))) return Effect.void;
  if (!deps.reach.covers(principal)) {
    return Effect.fail(finError("SCOPE_DENIED", `${tag} is for a session's own children`, { tag, scopes: [], principal: "agent", reason: "noSession" }, "Run it from inside an agent session, or as a person."));
  }
  if (tag === "agent.session.start") return Effect.void;
  const p = payload as { sessionId?: unknown; ids?: unknown } | null;
  const ids = tag === "agent.session.wait" ? (Array.isArray(p?.ids) ? p.ids : []) : [p?.sessionId];
  for (const id of ids) {
    const s = typeof id === "string" ? deps.sessions.get(id) : undefined;
    if (s?.parent !== principal.id) {
      return Effect.fail(finError("SESSION_NOT_FOUND", "no such agent session", { sessionId: typeof id === "string" ? id : "" }, "list your children with `fin agent ls`"));
    }
  }
  return Effect.void;
}

/** Stage 4 for an agent's rollback (Decision 23): its own direct child only, else `SCOPE_DENIED`. */
function rollbackCheck(principal: Principal, payload: unknown, deps: AgentHookDeps): Effect.Effect<void, FinError> {
  const id = (payload as { sessionId?: unknown } | null)?.sessionId;
  const target = typeof id === "string" ? deps.sessions.get(id) : undefined;
  if (deps.reach.covers(principal) && target !== undefined && target.parent === principal.id) return Effect.void;
  const reason = deps.reach.covers(principal) ? "notOwnChild" : "noSession";
  return Effect.fail(
    finError("SCOPE_DENIED", "a session rolls back its own direct children only", { tag: "agent.session.rollback", scopes: [], principal: "agent", reason }, "Roll back your own child (`fin agent ls`), or ask a person to."),
  );
}

/**
 * Stage 4 for an agent's resume (7B Decision 16 as amended 2026-11-13): its own child, and one it
 * interrupted itself (`child-resume.ts`); else `SCOPE_DENIED`. The runtime checks again.
 */
function resumeCheck(principal: Principal, payload: unknown, deps: AgentHookDeps): Effect.Effect<void, FinError> {
  if (!deps.reach.covers(principal)) {
    return Effect.fail(finError("SCOPE_DENIED", "a session resumes its own children only", { tag: "agent.session.resume", scopes: [], principal: "agent", reason: "noSession" }, "Run it from inside an agent session, or as a person."));
  }
  const id = (payload as { sessionId?: unknown } | null)?.sessionId;
  const denied = resumeDenied(typeof id === "string" ? deps.sessions.get(id) : undefined, principal);
  return denied === null ? Effect.void : Effect.fail(denied);
}

/** `SCOPE_DENIED` for a session calling one of `SESSION_REFUSED_RPCS`. */
function refusedToSessions(tag: string): FinError {
  const fix =
    tag === "project.open" || tag === "project.create"
      ? "An agent session works in the project it was started for, which Finisher opens for it."
      : "Only a person changes this: run it yourself, outside the agent session.";
  return finError("SCOPE_DENIED", `${tag} is not available to an agent session`, { tag, scopes: [], principal: "agent", reason: "agentSession" }, fix);
}

export interface AgentHookDeps {
  readonly reach: BranchReach;
  readonly paths: PathReach;
  readonly sessions: SessionSource;
  readonly clock?: () => number;
  /** CLI shells' inferred attribution (7D Task 12); without it a commit with no call is `none`. */
  readonly attribution?: { match(sessionId: string, argsHash: string | undefined, txIds: ReadonlyArray<string>): Matched };
}

/** The chain's hooks for agent sessions (`chain/hooks.ts`). */
export function agentChainHooks(deps: AgentHookDeps): ChainHooks {
  const clock = deps.clock ?? Date.now;
  return {
    extraRoots: (principal, only) => deps.paths.extra(principal, only),
    authorize: (tag, principal, resolved) =>
      (SESSION_REFUSED_RPCS.has(tag) || SESSION_PEOPLE_READS.has(tag)) && deps.reach.covers(principal)
        ? Effect.fail(refusedToSessions(tag))
        : childCheck(tag, principal, resolved.payload, deps),
    authorOf: (principal) => {
      const s = deps.reach.sessionOf(principal);
      return s === undefined ? undefined : sessionAuthor(s);
    },
    after: (tag, principal, payload, result, headers) =>
      Effect.sync(() => {
        if (!SESSION_TX_RPCS.has(tag)) return;
        const s = deps.reach.sessionOf(principal);
        if (s === undefined) return;
        const p = payload as { dryRun?: unknown } | null;
        if (p?.dryRun === true || (result as { dryRun?: unknown } | null)?.dryRun === true) return;
        const call = actorFor(principal, headers).actor.call;
        const callId = call !== undefined && CALL_ID.test(call) ? call : null;
        const at = clock();
        try {
          const txs = committedOf(result).map((c) => c.tx.txId);
          const hash = headerOf(headers, ARGS_HASH_HEADER);
          const m: Matched = callId !== null ? { callId, attribution: "exact" } : (deps.attribution?.match(s.id, hash, txs) ?? { callId: null, attribution: "none" });
          for (const txId of txs) deps.sessions.recordTx({ txId, sessionId: s.id, callId: m.callId, attribution: m.attribution, at });
        } catch (e) {
          // The write landed; a lost row is reported, never turned into a failed call.
          process.stderr.write(`finisherd: warning: cannot record session ${s.id}'s transaction: ${String(e)}\n`);
        }
      }),
  };
}
