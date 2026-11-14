// The plugin command registry (sp8b plan Task 9; spec §3.8): every enabled, active code
// plugin's `commands` contribution, named `plugin:<id>/<command>` (the name 7B's command
// rules match and `fin @<id> <command>` spells). The list is read from the installed rows on
// every ask, so enabling, disabling, a crash loop, a revocation or an update shows at once;
// each version's manifest and compiled argument schemas are read once (the store is fixed per
// version). Developer plugins (`source: dev`, Task 11) are never listed to, nor run by, an
// `agent` or `mcp` principal (Decision 19). `digest` = BLAKE3 of the list's canonical JSON,
// so `fin` caches it per server.
import type { FinError, Principal } from "@finisher/contracts";
import type { CommandContribution, Permission, PluginCommandInfo, PluginCommandList, ValidationReport } from "@finisher/core-types";
import { blake3Hex, PluginCommandArgs, validateManifest } from "@finisher/fin-node";
import { Effect } from "effect";
import { canonicalJson } from "../../agent/approvals.ts";
import { finError, toFinError } from "../../errors.ts";
import type { InstalledFull, PluginTables } from "../tables.ts";
import { storeDirOf } from "./program.ts";

/** The registry name's prefix. */
export const COMMAND_PREFIX = "plugin:";
/** A command's timeout without one in its manifest, and the most a manifest may give (spec §3.3). */
export const DEFAULT_COMMAND_TIMEOUT_S = 10;
export const MAX_COMMAND_TIMEOUT_S = 60;

/** One runnable command: what `plugin.commands` lists, its plugin row, grants and compiled args. */
export interface CommandEntry {
  readonly info: PluginCommandInfo;
  readonly row: InstalledFull;
  readonly grants: ReadonlyArray<Permission>;
  readonly args: PluginCommandArgs;
}

/** `plugin:<id>/<command>`. */
export function commandName(id: string, command: string): string {
  return `${COMMAND_PREFIX}${id}/${command}`;
}

/** A registry name's plugin id and command, or `undefined` for anything else. */
export function parseCommandName(name: string): { readonly id: string; readonly command: string } | undefined {
  if (!name.startsWith(COMMAND_PREFIX)) return undefined;
  const rest = name.slice(COMMAND_PREFIX.length);
  const cut = rest.lastIndexOf("/");
  if (cut <= 0 || !rest.startsWith("@")) return undefined;
  return { id: rest.slice(0, cut), command: rest.slice(cut + 1) };
}

/** Whether `principal` may see and run `row`'s commands (no developer plugin for agents or MCP). */
function visible(principal: Pick<Principal, "kind">, row: InstalledFull): boolean {
  return row.source !== "dev" || (principal.kind !== "agent" && principal.kind !== "mcp");
}

export function commandNotFound(name: string): FinError {
  return finError("PLUGIN_NOT_FOUND", `there is no plugin command ${name}`, { name }, "List the commands with `fin help @<plugin>` or `plugin.commands`.");
}

export interface CommandRegistryDeps {
  /** Links resolved. */
  readonly serverData: string;
  readonly tables: PluginTables;
}

interface Version {
  readonly commands: ReadonlyArray<CommandContribution>;
  readonly args: Map<string, PluginCommandArgs>;
}

export function makeCommandRegistry(d: CommandRegistryDeps) {
  /** Each installed version's commands, read once (`<id>@<version>`). */
  const versions = new Map<string, Promise<Version>>();
  /** The same, once read. */
  const read = new Map<string, Version>();

  const versionOf = (row: InstalledFull): Effect.Effect<Version, FinError> =>
    Effect.suspend(() => {
      const key = `${row.id}@${row.version}`;
      let p = versions.get(key);
      if (p === undefined) {
        p = (async () => {
          const report = JSON.parse(await validateManifest(storeDirOf(d.serverData, row), row.source)) as ValidationReport;
          if (!report.ok || report.manifest === undefined) {
            throw finError("PLUGIN_MANIFEST_INVALID", `${row.id}@${row.version}'s manifest no longer validates`, { plugin: row.id, version: row.version });
          }
          const commands = report.manifest.contributes?.commands ?? [];
          const v: Version = { commands, args: new Map(commands.map((c) => [c.name, new PluginCommandArgs(c.args)])) };
          read.set(key, v);
          return v;
        })();
        versions.set(key, p);
        // A failed read is not kept: the next ask reads again.
        p.catch(() => versions.get(key) === p && versions.delete(key));
      }
      const got = p;
      return Effect.tryPromise({ try: () => got, catch: toFinError });
    });

  /** The enabled, active code plugins' rows `principal` may see. */
  const rows = (principal: Pick<Principal, "kind">): InstalledFull[] =>
    d.tables
      .installed()
      .filter((r) => r.kind === "code" && r.enabled && r.state === "active")
      .map((r) => d.tables.installedFull(r.id))
      .filter((r): r is InstalledFull => r !== undefined && visible(principal, r));

  const entriesOf = (row: InstalledFull): Effect.Effect<CommandEntry[], FinError> =>
    Effect.map(versionOf(row), (v) => {
      const grants = d.tables.grants(row.id).map((g) => JSON.parse(g) as Permission);
      return v.commands.map((c) => ({
        info: {
          name: commandName(row.id, c.name),
          plugin: `${row.id}@${row.version}`,
          command: c.name,
          title: c.title,
          args: c.args,
          writes: c.writes === true,
          timeoutS: Math.min(MAX_COMMAND_TIMEOUT_S, c.timeoutS ?? DEFAULT_COMMAND_TIMEOUT_S),
        },
        row,
        grants,
        args: v.args.get(c.name)!,
      }));
    });

  /** Every command `principal` can run, sorted by name; a plugin whose manifest fails is left out. */
  const entries = (principal: Pick<Principal, "kind">): Effect.Effect<CommandEntry[]> =>
    Effect.map(
      Effect.forEach(rows(principal), (r) => Effect.orElseSucceed(entriesOf(r), () => [] as CommandEntry[])),
      (all) => all.flat().sort((a, b) => (a.info.name < b.info.name ? -1 : a.info.name > b.info.name ? 1 : 0)),
    );

  /** `name`'s command for `principal`; `PLUGIN_NOT_FOUND` when no enabled plugin it may use has it. */
  const resolve = (principal: Pick<Principal, "kind">, name: string): Effect.Effect<CommandEntry, FinError> =>
    Effect.gen(function* () {
      const parsed = parseCommandName(name);
      if (parsed === undefined) return yield* Effect.fail(commandNotFound(name));
      const row = rows(principal).find((r) => r.id === parsed.id);
      if (row === undefined) return yield* Effect.fail(commandNotFound(name));
      const found = (yield* Effect.orElseSucceed(entriesOf(row), () => [] as CommandEntry[])).find((e) => e.info.command === parsed.command);
      if (found === undefined) return yield* Effect.fail(commandNotFound(name));
      return found;
    });

  return {
    entries,
    resolve,
    /** `plugin.commands`: the list `principal` may run and its digest. */
    list: (principal: Pick<Principal, "kind">): Effect.Effect<PluginCommandList> =>
      Effect.map(entries(principal), (es) => {
        const commands = es.map((e) => e.info);
        return { digest: blake3Hex(Buffer.from(canonicalJson(commands), "utf8")), commands };
      }),
    /** The chain's `pluginCommand` resolver: whether `name` writes, for a command `principal` may run. */
    writes: (principal: Principal, name: string): Effect.Effect<boolean | undefined> =>
      Effect.orElseSucceed(Effect.map(resolve(principal, name), (e) => e.info.writes), () => undefined),
    /** A running command job's timeout (its `PluginContext.limits`), 60 s when unknown. */
    timeoutOf: (plugin: string, command: string): number => {
      const c = read.get(plugin)?.commands.find((x) => x.name === command);
      return c === undefined ? MAX_COMMAND_TIMEOUT_S : Math.min(MAX_COMMAND_TIMEOUT_S, c.timeoutS ?? DEFAULT_COMMAND_TIMEOUT_S);
    },
  };
}

export type CommandRegistry = ReturnType<typeof makeCommandRegistry>;
