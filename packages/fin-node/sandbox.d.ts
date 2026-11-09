// Types of `sandbox.js` (re-exported by `index.d.ts`).
import type { AclGrant, AgentInput, HookInput, SandboxMechanism, SandboxStatus } from "@finisher/core-types";

export type { AclGrant, SandboxMechanism, SandboxStatus };

/**
 * Every sandbox profile's status on this host (`fin_sandbox::status::all`): `enforced`,
 * `disabled {reason}` (F2: runs unconfined, with warnings) or `unavailable {need}` (the start
 * is refused). Synchronous and cheap (one Landlock version query on Linux).
 */
export declare function sandboxStatus(): SandboxStatus;

/**
 * The launcher `fin-sandbox-exec[.exe]` in `dir` (default: beside the loaded fin-node binary,
 * where `scripts/copy-workers.mjs` puts it with `fin-workers`). Plan 7C Task 5.
 */
export declare function sandboxExecPath(dir?: string): string;

/** The session's git configuration (`<session>/git/config`, named by `GIT_CONFIG_GLOBAL`). Plan 7C Task 11. */
export declare function sandboxSessionGitConfig(): string;

/**
 * This OS's base toolchain read list: links resolved, missing and nested entries dropped
 * (Linux, macOS); on Windows `%SystemRoot%` and `%ProgramFiles%` expanded and missing entries
 * dropped (plan 7C Task 11; Task 10 review B1).
 */
export declare function sandboxToolchainBase(): string[];

/** `null` when `fin-sandbox` builds an `agent` policy from `input`, else why not. Plan 7C Task 11. */
export declare function sandboxAgentCheck(input: AgentInput): string | null;

/**
 * One ACE the broker holds (plan 7C Task 10): `sid` gets `mask` on `path` (`inherit`: the
 * subtree inherits it) for `owner` (`session:<id>`, `job:<id>`, `call:<id>`, or `static`:
 * granted once and kept). The broker's argument, not a stored model.
 */
export interface AclBrokerGrant {
  readonly path: string;
  readonly sid: string;
  readonly mask: number;
  readonly inherit: boolean;
  readonly owner: string;
}

/**
 * Grants through the server's ACL broker: logged to `<serverData>/run/acl-broker.wal` and
 * fsynced, applied (Windows: the DACL; elsewhere nothing), marked done. A protected root is
 * refused first (`PATH_NOT_ALLOWED {reason: aclProtected}`), a missing path is `NOT_FOUND`,
 * a pair another owner holds `INVALID_OP {reason: aclOwned}`.
 */
export declare function aclGrant(serverData: string, grant: AclBrokerGrant): Promise<void>;

/** Revokes `owner`'s grants (only those on `path` when given); how many. */
export declare function aclRevoke(serverData: string, owner: string, path?: string): Promise<number>;

/**
 * Revokes every grant whose owner is not in `liveOwners` (`static` always is); how many. The
 * first call of a server opens the broker, completing what a crash left. A revoke that fails
 * stays pending for the next sweep; once every other one is done the call rejects with one
 * `IO_ERROR` naming them.
 */
export declare function aclSweep(serverData: string, liveOwners: Iterable<string>): Promise<number>;

/**
 * The ACEs an `agent` launch needs on Windows (`fin_sandbox::windows::grants::agent`; pure).
 * `extraRead` (each also in `input.toolchains`): read by the session's capability under the
 * session's owner, not static toolchain grants.
 */
export declare function sandboxAgentGrants(input: AgentInput, extraRead?: readonly string[]): AclGrant[];

/** The ACEs a `hook` launch needs on Windows (`fin_sandbox::windows::grants::hook`; pure). */
export declare function sandboxHookGrants(input: HookInput): AclGrant[];

/**
 * The ACE of a job visiting a warm worker for its attempt (plan 7C Decision 19; pure): its
 * directory in `<workerDir>/jobs/` to the worker's capability. `INVALID_ARGS` elsewhere.
 */
export declare function sandboxVisitGrant(workerDir: string, jobDir: string): AclGrant;

/** The AppContainer name of a project's agents and hooks (`finisher-p-<hash>`; pure). */
export declare function sandboxProjectContainer(projectDir: string): string;

/** The SID of the AppContainer `name` on Windows; `null` elsewhere. */
export declare function sandboxContainerSid(name: string): string | null;

/** @internal Test builds only: deletes a suite's AppContainer profile (a no-op off Windows). */
export declare function sandboxContainerDeleteForTest(name: string): void;

/** @internal Test builds only: the broker's live grants. */
export declare function aclGrantsForTest(serverData: string): AclBrokerGrant[];
