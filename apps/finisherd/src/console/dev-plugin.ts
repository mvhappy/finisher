// The start-up approval of developer mode (sp8b plan Task 11, Decision 19; spec §5.5):
// `finisherd [serve] --plugin-dir <dir>` needs a person at an interactive terminal. The folder is
// validated, what it is and every permission it asks for are shown (each manifest string through
// `safe()`), with the high-risk and `PLUGINS_UNCONFINED` warnings first, and the person approves
// them for this run before the server starts: `y`, or the plugin's id typed back when any
// permission is high-risk (`net`, `secrets`, `media:file`, `hooks:modify`, …). Anything else
// starts nothing. The prompt writes to stderr: a serving finisherd keeps stdout empty.
//
// Residual (as every console confirmation, `confirm.ts`): a program driving a pseudo-terminal
// passes the terminal check; sub-project 7's sandbox closes it for agents.
import { existsSync } from "node:fs";
import type { CommandContribution } from "@finisher/core-types";
import { permissionRisks } from "@finisher/fin-node";
import { type DevPlugin, devDataDir, devUnconfined, loadDevPlugin } from "../plugins/host/dev.ts";
import type { SandboxWarning } from "../sandbox/status.ts";
import { ConsoleError } from "./client.ts";
import { ask, proceed, requireTty, typeName } from "./confirm.ts";
import { exitCodeOf } from "./exit-codes.ts";
import { permissionText } from "./plugin-render.ts";
import { type Prompt, ttyPrompt } from "./prompt.ts";
import { safe } from "./safe.ts";

/** The flag, as the console names it. */
export const PLUGIN_DIR_FLAG = "--plugin-dir";

/** What the screen needs besides the plugin: the warning, and whether its data folder exists. */
export interface DevScreen {
  readonly unconfined: SandboxWarning | undefined;
  /** `plugins/data/@dev/<name>/` from an earlier run, when it exists. */
  readonly existingData?: string | undefined;
}

/** What the person reads before approving `dev` (loud warnings first). */
export function devPluginLines(dev: DevPlugin, { unconfined, existingData }: DevScreen): string[] {
  const m = dev.manifest;
  const lines = [
    "!! DEVELOPER PLUGIN — THIRD-PARTY CODE, HIGH RISK",
    `   ${safe(m.id)} ${safe(m.version)} is code from ${safe(dev.dir)}. It runs with your rights on this`,
    "   computer, for this run only, unsigned and never re-checked. Agents and MCP clients cannot reach it.",
    "",
  ];
  if (unconfined !== undefined) lines.push(`!! ${unconfined.code}`, `   ${unconfined.message}`, `   ${unconfined.fix}`, "");
  lines.push(`${safe(m.id)} ${safe(m.version)} by ${safe(m.publisher.name)}, licence ${safe(m.license)}`);
  const commands: ReadonlyArray<CommandContribution> = m.contributes?.commands ?? [];
  if (commands.length > 0) lines.push(`Commands: ${commands.map((c) => safe(c.name)).join(", ")}`);
  const permissions = m.permissions ?? [];
  if (permissions.length === 0) lines.push("Permissions: none");
  else {
    lines.push(`Permissions (${permissions.length}), granted until this server stops:`);
    const risks = permissionRisks(permissions);
    permissions.forEach((p, i) => lines.push(`  [${safe(risks[i])}] ${permissionText(p)}`));
  }
  if (existingData !== undefined) lines.push(`Existing plugin data will be visible to it: ${safe(existingData)}`);
  return lines;
}

/** Whether approving `dev` needs its id typed back: any of its permissions is high-risk. */
export const highRisk = (dev: DevPlugin): boolean => permissionRisks(dev.manifest.permissions ?? []).includes("high");

/**
 * The developer plugin of `pluginDir`, served from `serverData`, once the person approved it on
 * `prompt` (the terminal by default), or `undefined` without the flag. Throws
 * `CONSOLE_NEEDS_TTY` without a terminal, the validation's `FinError` for a folder that is not a
 * developer plugin (or is, holds or sits in server data), and `CONFIRMATION_REQUIRED` when declined.
 */
export async function approveDevPlugin(pluginDir: string | undefined, serverData: string, prompt?: Prompt, unconfined = devUnconfined): Promise<DevPlugin | undefined> {
  if (pluginDir === undefined) return undefined;
  const asked = prompt ?? ttyPrompt(process.stdin, process.stderr);
  requireTty(asked, PLUGIN_DIR_FLAG);
  const dev = await loadDevPlugin(pluginDir, serverData);
  const id = String(dev.manifest.id);
  const data = devDataDir(serverData, id);
  const lines = devPluginLines(dev, { unconfined: unconfined(), existingData: existsSync(data) ? data : undefined });
  await ask(asked, lines.join("\n"), highRisk(dev) ? typeName("plugin's id", id) : proceed, PLUGIN_DIR_FLAG);
  return dev;
}

/** Writes why the start was refused (`finisherd: CODE: message`, its fix) and answers the exit code. */
export function devRefused(e: unknown, write: (text: string) => void = (t) => void process.stderr.write(t)): number {
  const f = e instanceof ConsoleError ? e.error : (e as { code?: unknown; message?: unknown; fix?: unknown });
  const code = typeof f.code === "string" ? f.code : "INTERNAL";
  write(`finisherd: ${safe(code)}: ${safe(typeof f.message === "string" ? f.message : String(e))}\n`);
  if (typeof f.fix === "string") write(`finisherd: fix: ${safe(f.fix)}\n`);
  return exitCodeOf(code);
}
