// Types of `sandbox.js` (re-exported by `index.d.ts`).
import type { AgentInput, SandboxMechanism, SandboxStatus } from "@finisher/core-types";

export type { SandboxMechanism, SandboxStatus };

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
 * (Linux, macOS); as written on Windows. Plan 7C Task 11.
 */
export declare function sandboxToolchainBase(): string[];

/** `null` when `fin-sandbox` builds an `agent` policy from `input`, else why not. Plan 7C Task 11. */
export declare function sandboxAgentCheck(input: AgentInput): string | null;
