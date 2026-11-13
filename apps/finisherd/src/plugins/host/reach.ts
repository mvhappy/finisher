// Reach of a plugin call (sp8 spec §3.6; sp8b plan Task 6), checked after its permission:
// - **Projects:** a project (and any asset or artifact in it) outside the rights' `projects` is
//   `PROJECT_NOT_FOUND`, the same answer as an unknown one, so ids never leak.
// - **Branches:** `project.snapshot` and `project.view` read only the call's branch; for an
//   `agent` invoker (rights with `branch`) it must lie in its session's subtree, less its live
//   children's (7A-2's `BranchReach`), else `BRANCH_NOT_ALLOWED`. Other invokers read any
//   branch of a project in reach (as their own reads do).
// - **Libraries:** a library-scoped read resolves only through a project in reach that
//   references the library, or a library the rights reach (`LIBRARY_NOT_FOUND` otherwise).
import type { FinError, Session } from "@finisher/contracts";
import type { HostMethod, JobRights, Permission } from "@finisher/core-types";
import { Effect } from "effect";
import type { BranchReach, SessionSource } from "../../agent/reach.ts";
import { branchNotAllowed, outOfReach } from "../../chain/fixes.ts";
import { finError } from "../../errors.ts";
import { METHOD_PERMISSION, reaches, requirePermission } from "./rights.ts";

/** The tag reach errors name. */
const TAG = "plugin.call";

/**
 * `branch` (a reference) as its id when it lies in the agent session `principal`'s subtree in
 * `projectId`, else `BRANCH_NOT_ALLOWED`.
 */
export type SubtreeOf = (principal: { readonly kind: string; readonly id: string }, root: string, projectId: string, branch: string) => Effect.Effect<string, FinError>;

/** 7A-2's branch reach as `SubtreeOf`: the session must still exist, in this project, under the same root. */
export function agentSubtree(sessions: Pick<SessionSource, "get">, reach: Pick<BranchReach, "inSubtree">): SubtreeOf {
  return (principal, root, projectId, branch) => {
    const s: Session | undefined = principal.kind === "agent" ? sessions.get(principal.id) : undefined;
    if (s === undefined || s.projectId !== projectId || s.rootBranch !== root) return Effect.fail(branchNotAllowed(TAG, branch));
    return reach.inSubtree(s, branch, TAG);
  };
}

/** Whether a project references a library (5D's registry; v1 projects reference none). */
export type ProjectLibraries = (projectId: string, libraryId: string) => boolean;

export interface ReachDeps {
  readonly subtree: SubtreeOf;
  readonly projectLibraries?: ProjectLibraries;
}

export function libraryNotFound(id: string): FinError {
  return finError("LIBRARY_NOT_FOUND", "there is no such library", { id }, "Check the library's id or name.");
}

/** `PROJECT_NOT_FOUND` unless `projectId` is in reach. */
export function checkProject(rights: JobRights, projectId: string): FinError | undefined {
  return reaches(rights.projects, projectId) ? undefined : outOfReach(projectId);
}

/**
 * The branch a read of `projectId` may see: the project in reach, and for an agent `branch`
 * in its subtree. Answers the branch's id for an agent, else `branch` as given.
 */
export function checkBranch(rights: JobRights, projectId: string, branch: string, deps: Pick<ReachDeps, "subtree">): Effect.Effect<string, FinError> {
  const p = checkProject(rights, projectId);
  if (p !== undefined) return Effect.fail(p);
  const limit = rights.branch;
  if (limit == null) return rights.principal.kind === "agent" ? Effect.fail(branchNotAllowed(TAG, branch)) : Effect.succeed(branch);
  const inside = deps.subtree(rights.principal, limit.root, projectId, branch);
  // `subtree: false` (never built today) would be the root alone.
  return limit.subtree ? inside : Effect.flatMap(inside, (id) => (id === limit.root ? Effect.succeed(id) : Effect.fail(branchNotAllowed(TAG, branch))));
}

/**
 * A library-scoped read: through `viaProject` (in reach, referencing the library) or a library
 * in the rights' reach; `LIBRARY_NOT_FOUND` otherwise.
 */
export function checkLibrary(rights: JobRights, libraryId: string, deps: Pick<ReachDeps, "projectLibraries">, viaProject?: string): FinError | undefined {
  if (reaches(rights.libraries, libraryId)) return undefined;
  if (viaProject !== undefined && reaches(rights.projects, viaProject) && deps.projectLibraries?.(viaProject, libraryId) === true) return undefined;
  return libraryNotFound(libraryId);
}

/** What a call acts on: its job's project and branch, or (library-scoped reads) a library. */
export interface CallTarget {
  readonly project?: string | undefined;
  readonly branch?: string | undefined;
  readonly library?: string | undefined;
}

/**
 * A call's permission, then its reach. `perm` is the method's (`METHOD_PERMISSION`); a call
 * with no permission (`log`) is still bounded by its job's project. Answers the branch id the
 * call may read, when it names one.
 */
export function authorizeCall(rights: JobRights, perm: Permission["perm"] | undefined, target: CallTarget, deps: ReachDeps, readsBranch = false): Effect.Effect<string | undefined, FinError> {
  return Effect.gen(function* () {
    if (perm !== undefined) {
      const denied = requirePermission(rights, perm);
      if (denied !== undefined) return yield* Effect.fail(denied);
    }
    // A branch read by a branch-limited invoker names its branch: no default resolved later.
    if (readsBranch && target.branch === undefined && (rights.branch != null || rights.principal.kind === "agent")) return yield* Effect.fail(branchNotAllowed(TAG, ""));
    let branch: string | undefined;
    if (target.project !== undefined) {
      if (target.branch !== undefined) branch = yield* checkBranch(rights, target.project, target.branch, deps);
      else {
        const p = checkProject(rights, target.project);
        if (p !== undefined) return yield* Effect.fail(p);
      }
    }
    if (target.library !== undefined) {
      const l = checkLibrary(rights, target.library, deps, target.project);
      if (l !== undefined) return yield* Effect.fail(l);
    }
    return branch;
  });
}

/** The host methods that read a branch of the call's project (`project.snapshot`, `project.view`). */
export const BRANCH_READS: ReadonlySet<HostMethod> = new Set<HostMethod>(["project.snapshot", "project.view"]);

/** `authorizeCall` for a host method of host API 1.1. */
export function authorizeMethod(rights: JobRights, method: HostMethod, target: CallTarget, deps: ReachDeps): Effect.Effect<string | undefined, FinError> {
  return authorizeCall(rights, METHOD_PERMISSION[method], target, deps, BRANCH_READS.has(method));
}
