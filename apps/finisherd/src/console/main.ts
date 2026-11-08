// The `finisherd` console (spec §4.7, plan 5A Decisions 29–31): `finisherd` alone, `serve` or
// only flags serves; any other first word is a command for the running server, proved as
// `console` with `<server-data>/auth/console.key`. Read-only and service commands (`status`,
// `audit`, `stop`) run anywhere; every command that changes something needs an interactive
// terminal and the code the server shows to be typed back.
import { resolve } from "node:path";
import { defaultServerData } from "../config.ts";
import { connectConsole, ConsoleError } from "./client.ts";
import { agent } from "./commands/agent.ts";
import { audit } from "./commands/audit.ts";
import { auth } from "./commands/auth.ts";
import type { CommandContext } from "./commands/context.ts";
import { diagnostics } from "./commands/diagnostics.ts";
import { mcp } from "./commands/mcp.ts";
import { project } from "./commands/project.ts";
import { status } from "./commands/status.ts";
import { training } from "./commands/training.ts";
import { provider } from "../secrets/console.ts";
import { stop } from "./commands/stop.ts";
import { one, type Output, parseArgs, printFailure, processOutput } from "./format.ts";
import { plugin } from "./plugin.ts";
import { type Prompt, ttyPrompt } from "./prompt.ts";
import { registry } from "./registry.ts";

const COMMANDS: Record<string, (ctx: CommandContext) => Promise<number>> = {
  status,
  stop,
  audit,
  diagnostics,
  project,
  agent,
  provider,
  training,
  mcp,
  auth,
  plugin,
  registry,
};

export const HELP = `Usage: finisherd [serve] [--server-data <dir>]
       finisherd <command> [--server-data <dir>]

Commands for the running server, as its owner:
  status [--json]                          the server's state
  stop                                     stop the server gracefully
  audit [--since <date>] [--kind <kind>] [--limit <n>] [--json]
                                           the security audit log, newest first
  agent credentials [--all] [--json]       agent and MCP credentials (live ones, or all)

These change something: each needs an interactive terminal, shows what it will do and a
short code, and goes ahead once you type the code back.
  audit purge (--all | --before <date>)
  diagnostics consent diagnostics <on|off|unset>
  diagnostics <on|off|unset>
  diagnostics consent training <off|unset>  (training is turned on in the Finisher app only)
  diagnostics purge (--all | --project <id> | --before <date>)
  diagnostics project default <projectId>
  project takeover <dir|projectId> --confirm
  project open <dir> --relocate
  agent mint --label <name> [--scope <scope>]... [--project <id>]... [--branch own|<id>]...
             [--idle <duration>] [--absolute <duration>|none] [--out <file>]
                                           a credential for an agent harness you run by hand
  agent revoke <id>                        end a credential: its connections close now
  mcp setup --out <file> [--label <name>] [--scope <scope>]...
                                           a credential for the MCP adapter
  auth rotate-master                       a new key for all credentials: every one of them ends

Agent sessions (sub-project 7):
  agent start --project <dir|id> [--provider openrouter|local] [--model <id>] [--role <role>]
              [--mode plan|draft|auto] [--base <branch>] [--title <text>]
              [--prompt <text> | --prompt-file <file>] [--budget-usd <n>] [--budget-tokens <n>]
  agent ls [--project <dir|id>] [--status <status>] [--json]
  agent show <id> [--nodes] [--json]
  agent prompt|steer <id> <text>
  agent interrupt|resume|end <id>          a session's branch stays when it is interrupted or ended
  agent watch <id> [--json]                its conversation as it happens, until the session ends
  agent providers [--json]                 the harnesses, whether keys are set, the local endpoint
  agent rules ls [--project <dir|id>] [--session <id>] [--json]
                                           command rules of every layer, and which apply
  provider key ls [--json]                 the stored model keys (never their values)
  provider local show|test [--json]        the local model endpoint, and whether it answers
  training closed-targets [--json]         the counsel gate on closed-model training targets
These change something (each needs an interactive terminal and the typed code):
  agent approve <approval> allow|deny [--add <amount>] [--remember session|project|projectLocal]
                                           --add raises a budget: USD, tokens (200k) or time (30m)
  agent delete <id>                        removes a session's conversation; its branch stays
  agent rules add allow|ask|deny <command> [--project <dir|id>] [--session <id>] [--branch <ref>]
              [--mode plan|draft|auto] [--preset <p>] [--bytes-over <n>] [--minutes-over <n>]
  agent rules rm <id>                      a local rule (the other layers live in their files)
  project trust <dir|id>                   its finisher.rules.json applies, until the file changes
  provider key set openrouter [--project <dir|id>] [--label <name>]
                                           asks for the key without showing it (never an argument)
  provider key rm <id>
  provider local set <ollama|lmstudio|url>
  provider local rm
  training closed-targets pendingCounsel|cleared|excluded

Reading recorded content needs no code:
  diagnostics export --content [--store diagnostics|agentTraces] [--kind <kind>]... [--since <date>] [--project <id>] [--limit <n>]

Plugins and registries (spec §5):
  plugin search [text] [--kind content|code] [--registry <id>] [--json]
  plugin info <id> [--json]
  plugin ls [--json]
  registry ls|status <id>|refresh [<id>] [--json]
These change something (each needs an interactive terminal and the typed code):
  registry add <id> --dir <path> [--root <file> | --root-sha256 <hex>]
                                           no root given: shows the root's SHA-256 and waits for you to type it back
  registry rm <id>
  registry forget --root <sha256>
  plugin install <id>[@<version>] [--registry <id>] [--auto-update] [--accept-copyleft]
                                           prints the plan with its risks and asks for yes; for third-party code
                                           it warns first and asks for the plugin's id typed back
  plugin update <id>[@<version>] [--registry <id>] [--allow-downgrade] [--auto-update | --no-auto-update]
                                           --allow-downgrade needs the exact version
  plugin rm <id> [--purge-data]
  plugin enable|disable <id>

A credential's file is its only copy: keep it where only you can read it. Durations are like
90m, 24h or 7d; agent credentials expire after 24h unused or 7d, MCP ones after 30d unused.`;

/** Decision 29: serve for no argument, `serve`, or flags only. */
export function isServeCommand(argv: ReadonlyArray<string>): boolean {
  const first = argv[0];
  if (first === "--help" || first === "-h") return false;
  return first === undefined || first === "serve" || first.startsWith("-");
}

export interface ConsoleDeps {
  readonly prompt?: Prompt;
  readonly output?: Output;
  readonly env?: NodeJS.ProcessEnv;
  readonly createFile?: CommandContext["createFile"];
}

/** Runs one console command; resolves with its exit code (`fin`'s codes). */
export async function runConsole(argv: ReadonlyArray<string>, deps: ConsoleDeps = {}): Promise<number> {
  const output = deps.output ?? processOutput;
  try {
    const args = parseArgs(argv);
    const [name, ...words] = args.words;
    if (name === "help" || args.flags.has("help")) {
      output.out(HELP);
      return 0;
    }
    const command = name === undefined ? undefined : COMMANDS[name];
    if (command === undefined) {
      output.err(HELP);
      return printFailure(output, { code: "USAGE", message: `unknown command: ${name ?? ""}` });
    }
    const dir = one(args, "server-data");
    const serverData = dir === undefined ? defaultServerData(deps.env ?? process.env) : resolve(dir);
    return await command({
      args: { words, flags: args.flags },
      serverData,
      prompt: deps.prompt ?? ttyPrompt(),
      output,
      connect: () => connectConsole(serverData),
      createFile: deps.createFile,
    });
  } catch (e) {
    if (e instanceof ConsoleError) return printFailure(output, e.error);
    return printFailure(output, { code: "INTERNAL", message: e instanceof Error ? e.message : String(e) });
  }
}
