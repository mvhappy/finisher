// What the capability probe (plan 7D Decision 8, Task 5's `probe.ts`) learns about one
// `(version, flags hash, OS)`, as the launch plan (Task 4) reads it. Kept apart from `probe.ts`
// so the launch plan does not depend on the probe's process code; `probe.ts` re-exports it.
import type { CliModel } from "../catalogue/cli-models.ts";

/** The probe's settings canary (Decision 4): `outputStyle` in its settings, `output_style` in `initialize` and init. */
export const CANARY_STYLE = "Explanatory";

export interface ProbeResult {
  readonly version: string;
  /**
   * How permission prompts reach Finisher (Decision 10): `stdio` (`can_use_tool` control
   * requests, `--permission-prompt-tool stdio`) or `mcp` (the documented fallback: the
   * `finisher` SDK MCP server's `approve` tool, `--permission-prompt-tool mcp__finisher__approve`).
   */
  readonly permissionPath: "stdio" | "mcp";
  /**
   * The CLI's tool set as the unsigned probe's init listed it (every tool Finisher maps, Task 4's
   * `CLI_TOOLS`). Empty when the probe was `initPending` and when a session's init completed it:
   * a session's subset is never recorded here.
   */
  readonly tools: readonly string[];
  readonly models: readonly CliModel[];
  readonly capabilities: readonly string[];
  /**
   * `--restricted` replaces `--setting-sources user` (Decision 1): set only for a version whose
   * probe saw it keep the SDK MCP server, the `--tools` names, `--settings` and the sign-in.
   * *Task 5 review:* the probe never sets it, so it stays off (and the flags hash covers only the
   * `--setting-sources user` launch) until Task 0's signed-in half confirms the sign-in survives;
   * the task that turns it on probes and hashes the `--restricted` launch too.
   */
  readonly restricted?: boolean;
  /**
   * Windows only (Decision 21): the shell is the `finisher` SDK MCP server's `shell` tool, not
   * the CLI's `Bash`. Off until Task 13 builds the brokered shell and sets it (owner Q2(a),
   * 2026-11-12): Windows sessions run `direct` and use the CLI's own `Bash` until then.
   */
  readonly brokeredShell?: boolean;
  /**
   * Step 2 is pending the first instance (Decision 8): `initialize` showed an account other than
   * exactly the unsigned one, so the probe sent no prompt; the first real instance's init must
   * pass `checkInit` before `completeProbe` clears this (`tools` stays empty).
   */
  readonly initPending?: true;
}
