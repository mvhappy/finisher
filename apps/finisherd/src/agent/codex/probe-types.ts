// What the Codex capability probe (plan 7E Decision 8, Task 8's `probe.ts`) learns about one
// `(pinned version, flags hash, OS)`, as the launch plan (Task 7) reads it. Kept apart from
// `probe.ts` so the launch plan does not depend on the probe's process code (7D's pattern).
import type { CliModel } from "../catalogue/cli-models.ts";

export interface CodexProbe {
  /** The version `initialize`'s `userAgent` names: the pin's. */
  readonly version: string;
  /**
   * The thread sandbox (Decision 7): `read-only`, or `danger-full-access` where Codex's own
   * sandbox cannot start inside Finisher's (then only with `approval: "granular"`).
   */
  readonly sandbox: "read-only" | "danger-full-access";
  /** `on-request`, or Decision 7's granular policy (every patch still asks). */
  readonly approval: "on-request" | "granular";
  /** `model/list` as the probe saw it, signed out (Decision 10). */
  readonly models: readonly CliModel[];
}
