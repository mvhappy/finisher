// Types of `sandbox.js` (re-exported by `index.d.ts`).
import type { SandboxMechanism, SandboxStatus } from "@finisher/core-types";

export type { SandboxMechanism, SandboxStatus };

/**
 * Every sandbox profile's status on this host (`fin_sandbox::status::all`): `enforced`,
 * `disabled {reason}` (F2: runs unconfined, with warnings) or `unavailable {need}` (the start
 * is refused). Synchronous and cheap (one Landlock version query on Linux).
 */
export declare function sandboxStatus(): SandboxStatus;
