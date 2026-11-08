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
// - **Author and `session_txs`:** every write carries the session's author
//   (`agent:<role>:<sessionId>`), whatever it sent, and every commit, undo, redo, revert,
//   cherry-pick and merge it makes is recorded in `session_txs` after the engine answers,
//   whatever its call attribution.
import { join } from "node:path";
import type { FinError, FinRpcTag, Principal, Session } from "@finisher/contracts";
import { ruleOf } from "@finisher/contracts";
import { Effect } from "effect";
import type { ChainHooks } from "../chain/hooks.ts";
import { branchNotAllowed } from "../chain/fixes.ts";
import type { Outcome, PolicyRule } from "../chain/policy.ts";
import { canonicalPath, type ChainProjects, resolveWrites, rootsOf } from "../chain/resolve.ts";
import { actorFor } from "../diagnostics.ts";
import { finError } from "../errors.ts";
import { committedOf } from "../history.ts";
import type { SessionTx } from "./store-tables.ts";

/** What reach reads of the agent store (`AgentStore` is one). */
export interface SessionSource {
  get(sessionId: string): Session | undefined;
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
      const sub = yield* this.#lineage.subtree(this.#projects, projectId, session.rootBranch, targets);
      const outside = targets.find((t) => !sub.has(t));
      if (outside !== undefined) return yield* Effect.fail(branchNotAllowed(tag, outside));
      return [...sub];
    });
  }

  /** `branch` (a reference) as its id, when it is in `session`'s subtree; else `BRANCH_NOT_ALLOWED`. */
  inSubtree(session: Session, branch: string, tag = "agent.self.try"): Effect.Effect<string, FinError> {
    return Effect.gen({ self: this }, function* () {
      if (session.rootBranch === undefined) return yield* Effect.fail(branchNotAllowed(tag, branch));
      const id = yield* this.#projects.resolveBranch(session.projectId, branch);
      const sub = yield* this.#lineage.subtree(this.#projects, session.projectId, session.rootBranch, [id]);
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
 * `SCOPE_DENIED` for a session opening or creating a project (spec §5.6, §6.1): no mode holds
 * either; the session's own project is opened by `finisherd` at start.
 */
function noProjectOpen(tag: string): FinError {
  return finError(
    "SCOPE_DENIED",
    `${tag} is not available to an agent session`,
    { tag, scopes: [], principal: "agent", reason: "agentSession" },
    "An agent session works in the project it was started for, which Finisher opens for it.",
  );
}

export interface AgentHookDeps {
  readonly reach: BranchReach;
  readonly paths: PathReach;
  readonly sessions: SessionSource;
  readonly clock?: () => number;
}

/** The chain's hooks for agent sessions (`chain/hooks.ts`). */
export function agentChainHooks(deps: AgentHookDeps): ChainHooks {
  const clock = deps.clock ?? Date.now;
  return {
    extraRoots: (principal, only) => deps.paths.extra(principal, only),
    authorize: (tag, principal) =>
      (tag === "project.open" || tag === "project.create") && deps.reach.covers(principal) ? Effect.fail(noProjectOpen(tag)) : Effect.void,
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
          for (const c of committedOf(result)) {
            deps.sessions.recordTx({ txId: c.tx.txId, sessionId: s.id, callId, attribution: callId === null ? "none" : "exact", at });
          }
        } catch (e) {
          // The write landed; a lost row is reported, never turned into a failed call.
          process.stderr.write(`finisherd: warning: cannot record session ${s.id}'s transaction: ${String(e)}\n`);
        }
      }),
  };
}
