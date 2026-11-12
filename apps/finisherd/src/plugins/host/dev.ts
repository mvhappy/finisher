// Developer mode (sp8b plan Task 11, Decision 19; spec §5.5): `finisherd --plugin-dir <dir>`,
// started by a person on the console, serves one unpacked code plugin `@dev/<name>` from its
// own folder, for that server run only:
// - the folder is validated with source class `dev` (8A's `validate`); its id must be
//   `@dev/<name>`, it must be a code plugin, and it may not declare models (8C's, Decision 18);
// - its permissions are shown and approved on the console before the server starts
//   (`console/dev-plugin.ts`); nothing is installed without that answer;
// - the row (`source: dev`, `dir` = the folder's real path, enabled) and its grants are written
//   at start and deleted when the server stops; a start deletes any a crash left, so a
//   developer plugin never outlives its run;
// - it is never re-hashed (it is meant to change; Decision 16), never listed to nor run by an
//   `agent` or `mcp` principal (`commands.ts`), and its Node process reads only its folder
//   (`program.ts`, `node-runtime.ts`);
// - it runs unconfined until 7C's `plugin` profile applies, with `PLUGINS_UNCONFINED` shown.
import { realpathSync, statSync } from "node:fs";
import { resolve } from "node:path";
import type { Manifest, ValidationReport } from "@finisher/core-types";
import { validateManifest } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError } from "../../errors.ts";
import { currentSandboxStatus, profileWarning, type SandboxStatus, type SandboxWarning } from "../../sandbox/status.ts";
import { canonical } from "../plans.ts";
import type { PluginTables } from "../tables.ts";

/** The publisher every developer plugin has. */
export const DEV_PUBLISHER = "@dev/";
/** A developer plugin's `treeHash`: it is never hashed. */
export const DEV_TREE_HASH = "dev";
/** Who approved a developer plugin's grants: the person at the console that started the server. */
export const DEV_GRANTED_BY = "console:console";

/** A validated developer plugin folder: its real path and its manifest. */
export interface DevPlugin {
  readonly dir: string;
  readonly manifest: Manifest;
}

const NOT_A_FOLDER_FIX = "Name the folder that holds your plugin's finisher-plugin.json.";

/** Validates `dir` as a developer plugin; throws the `FinError` that refuses it. */
export async function loadDevPlugin(dir: string): Promise<DevPlugin> {
  let real: string;
  try {
    real = realpathSync(resolve(dir));
  } catch {
    throw finError("PLUGIN_NOT_FOUND", `${dir} does not exist`, { path: dir, reason: "devDir" }, NOT_A_FOLDER_FIX);
  }
  if (!statSync(real).isDirectory()) throw finError("PLUGIN_NOT_FOUND", `${dir} is not a folder`, { path: dir, reason: "devDir" }, NOT_A_FOLDER_FIX);
  // Node's permission flags name the folder: `*` would widen the grant, `,` is refused.
  if (real.includes("*") || real.includes(",")) {
    throw finError("PLUGIN_RUNTIME_UNAVAILABLE", "the plugin folder's path holds a character Node's permission flags cannot take", { path: real, reason: "pathCharacter" }, "Move the folder to a path without * or ,.");
  }
  const report = JSON.parse(await validateManifest(real, "dev")) as ValidationReport;
  const errors = report.errors ?? [];
  if (!report.ok || report.manifest == null) {
    // The host-dependent refusals first (platform, host API, Node), as `plugin.enable` does.
    const first = errors.find((x) => x.code !== "PLUGIN_MANIFEST_INVALID") ?? errors[0];
    throw finError(first?.code ?? "PLUGIN_MANIFEST_INVALID", first?.message ?? "the plugin's manifest does not validate", { ...(first?.details as object | undefined), dir: real, problems: errors.length });
  }
  const manifest = report.manifest;
  const id = String(manifest.id);
  if (!id.startsWith(DEV_PUBLISHER)) {
    throw finError("PLUGIN_MANIFEST_INVALID", `developer mode loads @dev/<name> plugins, not ${id}`, { path: "/id", reason: "devPublisherRequired", id }, "Give the manifest the id @dev/<name> while you develop it.");
  }
  if (manifest.kind !== "code") {
    throw finError("PLUGIN_MANIFEST_INVALID", `${id} is a content pack; developer mode serves code plugins`, { path: "/kind", reason: "devCodeOnly", id });
  }
  if ((manifest.models ?? []).length > 0) {
    throw finError("PLUGIN_CONTRIBUTION_UNSUPPORTED", `${id} needs models, which this version of Finisher cannot provide yet`, { plugin: id, kind: "models" });
  }
  return { dir: real, manifest };
}

/** `PLUGINS_UNCONFINED` while the `plugin` profile is not enforced here (every host before 7C). */
export function devUnconfined(status: SandboxStatus = currentSandboxStatus()): SandboxWarning | undefined {
  const w = profileWarning("plugin", status.os, status.plugin);
  return w?.code === "PLUGINS_UNCONFINED" ? w : undefined;
}

/** Deletes every developer plugin's row and grants; answers the ids. */
export function clearDevPlugins(tables: PluginTables): string[] {
  const ids = tables
    .installed()
    .filter((r) => r.source === "dev")
    .map((r) => r.id);
  if (ids.length > 0) tables.store.tx(() => ids.forEach((id) => tables.deletePlugin(id)));
  return ids;
}

/** Installs `dev` enabled, with every permission its manifest asks for granted, at `at`. */
export function installDevPlugin(tables: PluginTables, dev: DevPlugin, at: number): void {
  const { manifest } = dev;
  const id = String(manifest.id);
  tables.store.tx(() => {
    tables.deletePlugin(id);
    tables.addPlugin({ id, version: manifest.version, source: "dev", treeHash: DEV_TREE_HASH, dir: dev.dir, kind: "code", enabled: true, state: "active", installedBy: DEV_GRANTED_BY, installedAt: at });
    tables.setGrants(id, manifest.version, (manifest.permissions ?? []).map(canonical), DEV_GRANTED_BY, at);
  });
}

/**
 * The start's developer-mode step (before anything serves): rows an earlier run left go; `dev`,
 * when the person approved one, is installed until the scope closes, and warned about.
 */
export const devPluginForRun = (tables: PluginTables, dev: DevPlugin | undefined, log: (message: string) => void, clock: () => number = Date.now) =>
  Effect.acquireRelease(
    Effect.sync(() => {
      clearDevPlugins(tables);
      if (dev === undefined) return;
      installDevPlugin(tables, dev, clock());
      const id = String(dev.manifest.id);
      log(`finisherd: developer plugin ${id} ${dev.manifest.version} is served from ${dev.dir} for this run only; agents and MCP clients cannot reach it`);
      const w = devUnconfined();
      if (w !== undefined) log(`finisherd: ${w.code} (${id}): ${w.message}`);
    }),
    () =>
      Effect.sync(() => {
        try {
          clearDevPlugins(tables);
        } catch {
          // The control store is busy or closing: the next start removes the row.
        }
      }),
  );
