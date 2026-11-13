// The `finisherd` console (spec §4.7, plan 5A Decisions 29–31): `finisherd` alone, `serve` or
// only flags serves; any other first word is a command for the running server, proved as
// `console` with `<server-data>/auth/console.key`. Read-only and service commands (`status`,
// `audit`, `stop`) run anywhere; every command that changes something needs an interactive
// terminal and the person's confirmation there: a `y`, or for what cannot be undone the target's
// name typed back (`confirm.ts`).
import { resolve } from "node:path";
import { defaultServerData } from "../config.ts";
import { connectConsole, ConsoleError } from "./client.ts";
import { agent } from "./commands/agent.ts";
import { audit } from "./commands/audit.ts";
import { auth } from "./commands/auth.ts";
import { cache } from "./commands/cache.ts";
import type { CommandContext } from "./commands/context.ts";
import { devices, revoke } from "./commands/devices.ts";
import { diagnostics } from "./commands/diagnostics.ts";
import { env } from "./commands/env.ts";
import { library } from "./commands/library.ts";
import { mcp } from "./commands/mcp.ts";
import { pair } from "./commands/pair.ts";
import { project } from "./commands/project.ts";
import { remote } from "./commands/remote.ts";
import { review } from "./commands/review.ts";
import { status } from "./commands/status.ts";
import { tls } from "./commands/tls.ts";
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
  library,
  agent,
  provider,
  training,
  cache,
  review,
  mcp,
  auth,
  plugin,
  registry,
  // 5B: pairing, devices, remote access, the identity and the environment.
  pair,
  devices,
  revoke,
  remote,
  tls,
  env,
};

export const HELP = `Usage: finisherd [serve] [--server-data <dir>]
       finisherd <command> [--server-data <dir>]

Commands for the running server, as its owner:
  status [--json]                          the server's state
  stop                                     stop the server gracefully
  audit [--since <date>] [--kind <kind>] [--limit <n>] [--json]
                                           the security audit log, newest first
  agent credentials [--all] [--json]       agent and MCP credentials (live ones, or all)
  devices [--all] [--json]                 paired devices (live ones, or all)
  remote status [--json]                   whether remote access is on, where, and its pin
  tls show [--json]                        the identity's pin and the routes devices use
  env show [--json]                        the environment devices pair with

These change something: each needs an interactive terminal, says what it will do, and goes
ahead once you answer y (or, for what cannot be undone, type the name it shows back).
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
  library add <dir> [--name <n>] [--storage local|network|sync] [--include <glob>]... [--exclude <glob>]...
              [--kinds all|<kind>,...] [--poll demand|<s>] [--full daily|weekly|manual]
              [--hydrate] [--bandwidth <MiB/s>] [--alias <path>]...
                                           index a folder as a library and start its first scan
  library rm <library>                     forget a library and delete its catalog (the files stay)
  library set <library> [--hydrate on|off] [--bandwidth <MiB/s>|off] [--alias <path>]... [--no-aliases]
              [--accept-volume] [--accept-unmounted <dir>] [--accept-empty-root]
                                           the library settings only a person changes
  agent revoke <id>                        end a credential: its connections close now
  mcp setup --out <file> [--label <name>] [--scope <scope>]...
                                           a credential for the MCP adapter
  auth rotate-master                       a new key for all credentials: every one of them ends
  pair [--label <text>] [--ttl <duration>] a one-time pairing link and QR code; approve the device here
                                           after comparing its safety code
  revoke <device-id>                       end a device: its connections close now (type its name)
  remote enable [--listen <host:port>] [--tls self-signed | --cert <file> --key <file>] [--public-url <url>]
                                           open the server to the network, TLS only
  remote disable                           close the remote listener; devices stay paired
  tls rotate                               a new identity: every device pairs again (type the environment's name)
  env name <name>                          the name devices show for this environment
  env adopt                                the server data was moved here: devices are let in again
  env reset                                it is a copy: a new identity, devices deleted, credentials revoked

Agent sessions (sub-project 7):
  agent start --project <dir|id> [--provider openrouter|local] [--model <id>] [--role <role>]
              [--mode plan|draft|auto] [--base <branch>] [--title <text>]
              [--prompt <text> | --prompt-file <file>] [--budget-usd <n>] [--budget-tokens <n>]
  agent run <prompt> --project <dir|id> [--param <name>=<value>]... [--harness <h>] [--provider openrouter|local] [--model <id>]
                                           start a saved prompt (agent prompts [--project] lists them)
  agent ls [--project <dir|id>] [--status <status>] [--json]
  agent show <id> [--nodes] [--json]
  agent prompt|steer <id> <text>
  agent interrupt|resume|end <id>          a session's branch stays when it is interrupted or ended
  agent rollback <id> --to-run <seq>       undo its runs after <seq> on every branch, all or nothing
  agent watch <id> [--json]                its conversation as it happens, until the session ends
  agent providers [--json]                 the harnesses, whether keys are set, the local endpoint
  agent rules ls [--project <dir|id>] [--session <id>] [--json]
                                           command rules of every layer, and which apply
  provider key ls [--json]                 the stored model keys (never their values)
  provider local show|test [--json]        the local model endpoint, and whether it answers
  provider signin|signout claudeCode       prints the CLI's own sign-in commands to run here; runs nothing
  training closed-targets [--json]         the counsel gate on closed-model training targets
  cache status [--json]                    the media cache: its size against the cap, by recipe
  review policy show --project <dir|id> [--json]
                                           protected branches, required review, auto merge
These change something (each needs an interactive terminal and your confirmation):
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
  cache gc [--to <GiB>]                    evict unpinned media cache entries down to the cap or lower
  review policy set --project <dir|id> [--protect <branch>]... [--unprotect <branch>]...
              [--require-review on|off] [--auto-merge on|off]
                                           auto merge answers AUTO_MERGE_UNAVAILABLE for now

Reading recorded content needs no code:
  diagnostics export --content [--store diagnostics|agentTraces] [--kind <kind>]... [--since <date>] [--project <id>] [--limit <n>]

Plugins and registries (spec §5):
  plugin search [text] [--kind content|code] [--registry <id>] [--json]
  plugin info <id> [--json]
  plugin ls [--json]
  registry ls|status <id>|refresh [<id>] [--json]
These change something (each needs an interactive terminal and your confirmation):
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
  /** The terminal's width (tests; default stdout's). */
  readonly columns?: () => number | undefined;
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
      columns: deps.columns ?? (() => process.stdout.columns),
    });
  } catch (e) {
    if (e instanceof ConsoleError) return printFailure(output, e.error);
    return printFailure(output, { code: "INTERNAL", message: e instanceof Error ? e.message : String(e) });
  }
}
