// The start-up approval of developer mode (sp8b plan Task 11, Decision 19; spec §5.5):
// `finisherd [serve] --plugin-dir <dir>` needs a person at an interactive terminal. The folder is
// validated, what it is and every permission it asks for are shown (each manifest string through
// `safe()`), with the high-risk and `PLUGINS_UNCONFINED` warnings first, and the person approves
// them for this run before the server starts. No answer, or anything but `y`, starts nothing.
// The prompt writes to stderr: a serving finisherd keeps stdout empty.
import type { CommandContribution } from "@finisher/core-types";
import { type DevPlugin, devUnconfined, loadDevPlugin } from "../plugins/host/dev.ts";
import type { SandboxWarning } from "../sandbox/status.ts";
import { ConsoleError } from "./client.ts";
import { ask, proceed, requireTty } from "./confirm.ts";
import { exitCodeOf } from "./exit-codes.ts";
import { permissionText } from "./plugin-render.ts";
import { type Prompt, ttyPrompt } from "./prompt.ts";
import { safe } from "./safe.ts";

/** The flag, as the console names it. */
export const PLUGIN_DIR_FLAG = "--plugin-dir";

/** What the person reads before approving `dev` (loud warnings first). */
export function devPluginLines(dev: DevPlugin, unconfined: SandboxWarning | undefined): string[] {
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
    for (const p of permissions) lines.push(`  ${permissionText(p)}`);
  }
  return lines;
}

/**
 * The developer plugin of `pluginDir` once the person approved it on `prompt`, or `undefined`
 * without the flag. Throws `CONSOLE_NEEDS_TTY` without a terminal, the validation's `FinError`
 * for a folder that is not a developer plugin, and `CONFIRMATION_REQUIRED` when declined.
 */
export async function approveDevPlugin(pluginDir: string | undefined, prompt: Prompt = ttyPrompt(process.stdin, process.stderr), unconfined = devUnconfined): Promise<DevPlugin | undefined> {
  if (pluginDir === undefined) return undefined;
  requireTty(prompt, PLUGIN_DIR_FLAG);
  const dev = await loadDevPlugin(pluginDir);
  await ask(prompt, devPluginLines(dev, unconfined()).join("\n"), proceed, PLUGIN_DIR_FLAG);
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
