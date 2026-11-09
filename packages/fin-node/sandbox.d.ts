// Types of `sandbox.js` (re-exported by `index.d.ts`).
import type { SandboxMechanism, SandboxStatus } from "@finisher/core-types";

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
