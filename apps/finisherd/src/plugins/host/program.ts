// A plugin job's program (sp8b plan Task 4): `job.via.plugin` (`<id>@<version>`) → its
// installed row, which must be enabled and `active` (not revoked, not in a crash loop) at that
// exact version → its store directory → the manifest's runtime: `native` runs the package's
// executable for this platform with no arguments, `node` runs finisherd's own Node with the
// permission flags of `node-runtime.ts`. The store is trusted as 8A installed it: nothing is
// re-hashed before a start (Decision 16; until 7C's profile makes the store read-only, a
// user-level process can change an installed package unnoticed: the recorded residual).
import { readFileSync } from "node:fs";
import { isAbsolute, join, relative, sep } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { Manifest } from "@finisher/core-types";
import { finError, toFinError } from "../../errors.ts";
import type { InstalledFull } from "../tables.ts";
import { viaPlugin } from "./keys.ts";
import { nodeArgs } from "./node-runtime.ts";

/** The manifest's file name in a package (`fin_plugin_api::MANIFEST_FILE`). */
export const MANIFEST_FILE = "finisher-plugin.json";

/** What the family reads the installed plugins through (`PluginTables.installedFull`). */
export type InstalledOf = (id: string) => InstalledFull | undefined;

/** `<id>@<version>` split at its last `@` (an id starts with `@`). */
export function splitVia(plugin: string): { readonly id: string; readonly version: string } {
  const at = plugin.lastIndexOf("@");
  if (at <= 0 || at === plugin.length - 1) throw finError("PLUGIN_NOT_FOUND", "a plugin job names no version", { reason: "noVersion" });
  return { id: plugin.slice(0, at), version: plugin.slice(at + 1) };
}

/**
 * Why `row` may not start a process, as the error its job fails with, or `undefined`. Task 5
 * adds the `plugins.problem` column and the crash-loop state machine; the states that exist
 * now are refused here.
 */
export function unusable(row: InstalledFull): ReturnType<typeof finError> | undefined {
  const plugin = row.id;
  if (row.kind !== "code") return finError("PLUGIN_RUNTIME_UNAVAILABLE", `${plugin} is not a code plugin`, { plugin, reason: "notCode" });
  if (row.state === "revoked") return finError("PLUGIN_REVOKED", `${plugin} is revoked`, { plugin });
  if (row.state === "crashLoop") return finError("PLUGIN_CRASH_LOOP", `${plugin} kept crashing and is stopped until its owner enables it`, { plugin });
  if (!row.enabled || row.state !== "active") return finError("PLUGIN_DISABLED", `${plugin} is disabled`, { plugin });
  return undefined;
}

/** `<serverData>/plugins/<row.dir>`: `dir` is `store/@pub/name/<version-dir>` with plain parts. */
export function storeDirOf(serverData: string, row: InstalledFull): string {
  const parts = row.dir.split("/");
  if (parts[0] !== "store" || parts.length < 4 || parts.some((p) => p === "" || p === "." || p === ".." || p.includes("\\"))) {
    throw finError("PLUGIN_NOT_FOUND", `${row.id} has no package in the store`, { plugin: row.id, reason: "storeDir" });
  }
  return join(serverData, "plugins", ...parts);
}

/** A package path (`/`-separated, relative, no `.`/`..` part) inside `dir`. */
function inPackage(dir: string, path: string, plugin: string): string {
  const parts = path.split("/");
  const full = join(dir, ...parts);
  const rel = relative(dir, full);
  if (path === "" || isAbsolute(path) || parts.some((p) => p === "" || p === "." || p === ".." || p.includes("\\")) || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw finError("PLUGIN_MANIFEST_INVALID", `${plugin}'s runtime names a path outside its package`, { plugin, reason: "runtimePath" });
  }
  return full;
}

/** This host's platform in the manifest's spelling (`linux-x64`). */
export const hostPlatform = (): string => `${process.platform}-${process.arch}`;

/** What `program(job)` resolves before the family adds its environment and grants. */
export interface ResolvedRuntime {
  readonly row: InstalledFull;
  readonly storeDir: string;
  readonly runtime: "native" | "node";
  readonly path: string;
  readonly args: ReadonlyArray<string>;
}

export interface ResolveDeps {
  readonly serverData: string;
  readonly installed: InstalledOf;
  /** The Node that runs `node` plugins (default `process.execPath`). */
  readonly execPath?: string;
  readonly platform?: string;
}

/** The job's installed row, or the error it fails with (not installed, another version, unusable). */
export function jobRow(installed: InstalledOf, job: JobView): { readonly row: InstalledFull } | { readonly error: FinError } {
  try {
    const { id, version } = splitVia(viaPlugin(job));
    const row = installed(id);
    if (row === undefined) return { error: finError("PLUGIN_NOT_FOUND", `${id} is not installed`, { plugin: id }) };
    // Jobs of another version (an update switched the row) never start the new one's code.
    if (row.version !== version) return { error: finError("PLUGIN_NOT_FOUND", `${id}@${version} is not the installed version`, { plugin: id, version, installed: row.version }) };
    const refused = unusable(row);
    return refused === undefined ? { row } : { error: refused };
  } catch (e) {
    return { error: toFinError(e) };
  }
}

/**
 * The job's plugin, its package and its runtime's program; `tmp` is the key's temp, `data` the
 * caller's `pluginData` directory when `fs pluginData` is granted (a `node` plugin's flags name it).
 */
export function resolveRuntime(deps: ResolveDeps, job: JobView, tmp: string, data?: { readonly path: string; readonly write: boolean }): ResolvedRuntime {
  const found = jobRow(deps.installed, job);
  if ("error" in found) throw found.error;
  const { row } = found;
  const id = row.id;
  const storeDir = storeDirOf(deps.serverData, row);
  let manifest: Manifest;
  try {
    manifest = JSON.parse(readFileSync(join(storeDir, MANIFEST_FILE), "utf8")) as Manifest;
  } catch {
    throw finError("PLUGIN_NOT_FOUND", `${id}'s package cannot be read`, { plugin: id, reason: "manifest" });
  }
  const rt = manifest.runtime;
  if (rt?.type === "native") {
    const platform = deps.platform ?? hostPlatform();
    const exec = (rt.exec as Readonly<Record<string, string | undefined>>)[platform];
    if (exec === undefined) throw finError("PLUGIN_PLATFORM", `${id} has no executable for ${platform}`, { plugin: id, platform });
    return { row, storeDir, runtime: "native", path: inPackage(storeDir, exec, id), args: [] };
  }
  if (rt?.type === "node") {
    const main = inPackage(storeDir, rt.main, id);
    return { row, storeDir, runtime: "node", path: deps.execPath ?? process.execPath, args: nodeArgs({ pluginDir: storeDir, main, tmp, ...(data === undefined ? {} : { data }) }) };
  }
  throw finError("PLUGIN_RUNTIME_UNAVAILABLE", `${id} has no runtime this host runs`, { plugin: id, runtime: (rt as { type?: unknown } | undefined)?.type ?? null });
}
