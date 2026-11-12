// `plugin.command` (sp8b plan Task 15; spec §11): one entry per `plugin.command.run`, whatever
// its outcome. Task 9's `commands.ts` calls `journalPluginCommand` (through
// `PluginHost.journalCommand`) once per run. The plugin and the command go to fin-node raw
// (`recordPlugin`): it names the plugin by its `PluginRef`, keys the command as the hash of
// `<plugin id>/<command>`, turns `txId` into an `IdRef`, and keeps `args` (training content,
// `toolArgs`) only while training is on and within 16 KiB of JSON. Nothing is built while the
// journal is off; a refused entry is logged, never thrown (a run never fails for its entry).
import type { PluginCommandOutcome } from "@finisher/core-types";
import type { Journal } from "@finisher/fin-node";
import { pluginIdOf } from "./stats.ts";

export type CommandJournal = Pick<Journal, "enabled" | "recordPlugin">;

/** One finished command run, as Task 9 knows it. */
export interface PluginCommandRun {
  /** `<id>@<version>` (`Tx.via.plugin`) or the bare id; the version is dropped. */
  readonly plugin: string;
  /** The command's name in the manifest (`Tx.via.command`). */
  readonly command: string;
  /**
   * `accepted` (committed as is, or validated with `dryRun`), `rebased`, `conflict`,
   * `refused` (permission, reach, disabled), `invalid` (`PLUGIN_COMMAND_ARGS`,
   * `PLUGIN_TX_INVALID`), `timeout`, `data` (returned data, no transaction), `failed` (a
   * crash, a protocol error, the host): see `commandOutcome` for a run that ended in an error.
   */
  readonly outcome: PluginCommandOutcome;
  /** The error code of a run that did not commit (`CONFLICT`, `PLUGIN_TIMEOUT`, ...). */
  readonly code?: string;
  /** The committed transaction's id. */
  readonly txId?: string;
  /** Op kind → count of the ops the plugin returned (fin-core op names). */
  readonly ops?: Readonly<Record<string, number>>;
  readonly dryRun?: boolean;
  /** From the RPC's start to its answer. */
  readonly durationMs: number;
  /** The arguments as validated (training only; dropped by fin-node otherwise). */
  readonly args?: unknown;
}

/** The outcome of a run that ended with error `code`. */
export function commandOutcome(code: string): PluginCommandOutcome {
  switch (code) {
    case "CONFLICT":
      return "conflict";
    case "PLUGIN_COMMAND_ARGS":
    case "PLUGIN_TX_INVALID":
    case "PLUGIN_OUTPUT_INVALID":
      return "invalid";
    case "PLUGIN_TIMEOUT":
      return "timeout";
    case "PLUGIN_CRASHED":
    case "PLUGIN_PROTOCOL":
    case "PLUGIN_COMMAND_FAILED":
    case "INTERNAL":
    case "IO_ERROR":
      return "failed";
    default:
      return "refused";
  }
}

/**
 * Task 9's journal hook (`command-run.ts` `CommandRunEvent`, passed to `onRun`): structural
 * here, so this module does not depend on it. `outcome` is the result's, or `error` with `code`.
 */
export interface CommandRunEventLike {
  readonly plugin: string;
  readonly command: string;
  readonly outcome: "accepted" | "rebased" | "conflict" | "data" | "error";
  readonly code?: string;
  readonly dryRun: boolean;
  readonly ms: number;
}

/** What `CommandRunEvent` does not carry (Task 9 may add them to its event): training args, the commit. */
export interface CommandRunExtra {
  readonly txId?: string;
  readonly ops?: Readonly<Record<string, number>>;
  readonly args?: unknown;
}

/** `e` (Task 9's `onRun` event) as a `PluginCommandRun`; an `error` maps through `commandOutcome`. */
export function commandRunOf(e: CommandRunEventLike, extra: CommandRunExtra = {}): PluginCommandRun {
  const outcome: PluginCommandOutcome = e.outcome === "error" ? commandOutcome(e.code ?? "INTERNAL") : e.outcome;
  return {
    plugin: e.plugin,
    command: e.command,
    outcome,
    ...(e.code === undefined ? {} : { code: e.code }),
    ...(e.dryRun ? { dryRun: true } : {}),
    durationMs: e.ms,
    ...extra,
  };
}

/** Records `run` as `plugin.command` v1; whether an entry was handed to the journal. */
export function journalPluginCommand(journal: CommandJournal | undefined, run: PluginCommandRun, log?: (message: string) => void): boolean {
  if (journal?.enabled !== true) return false;
  const payload = {
    command: run.command,
    outcome: run.outcome,
    ...(run.code === undefined ? {} : { code: run.code }),
    ...(run.txId === undefined ? {} : { txId: run.txId }),
    ...(run.ops === undefined ? {} : { ops: run.ops }),
    ...(run.dryRun === true ? { dryRun: true } : {}),
    micros: Math.max(0, Math.round(run.durationMs * 1000)),
    ...(run.args === undefined ? {} : { args: run.args }),
  };
  try {
    journal.recordPlugin("plugin.command", 1, pluginIdOf(run.plugin), JSON.stringify(payload));
    return true;
  } catch (e) {
    log?.(`plugin commands: the journal refused a plugin.command: ${(e as Error).message}`);
    return false;
  }
}
