// What the capability probe (plan 7D Decision 8, Task 5's `probe.ts`) learns about one
// `(version, flags hash, OS)`, as the launch plan (Task 4) reads it. Kept apart from `probe.ts`
// so the launch plan does not depend on the probe's process code; `probe.ts` re-exports it.
import type { CliModel } from "../catalogue/cli-models.ts";

export interface ProbeResult {
  readonly version: string;
  /**
   * How permission prompts reach Finisher (Decision 10): `stdio` (`can_use_tool` control
   * requests, `--permission-prompt-tool stdio`) or `mcp` (the documented fallback: the
   * `finisher` SDK MCP server's `approve` tool, `--permission-prompt-tool mcp__finisher__approve`).
   */
  readonly permissionPath: "stdio" | "mcp";
  readonly tools: readonly string[];
  readonly models: readonly CliModel[];
  readonly capabilities: readonly string[];
  /**
   * `--restricted` replaces `--setting-sources user` (Decision 1): set only for a version whose
   * probe saw it keep the SDK MCP server, the `--tools` names, `--settings` and the sign-in.
   */
  readonly restricted?: boolean;
  /**
   * Windows only (Decision 21): the shell is the `finisher` SDK MCP server's `shell` tool, not
   * the CLI's `Bash`. Off until Task 13 builds the brokered shell and sets it (owner Q2(a),
   * 2026-11-12): Windows sessions run `direct` and use the CLI's own `Bash` until then.
   */
  readonly brokeredShell?: boolean;
}
