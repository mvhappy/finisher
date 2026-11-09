// The one `plugin` family (sp8b plan Decisions 2, 3, 9; Task 4): every code plugin's process
// is a 5C worker of this family (`Program::Path`, profile `plugin`), whatever plugin it runs.
// `program(job)` resolves the job's plugin (`program.ts`); `warmKey` keeps one process per
// (plugin@version, project, principal class) (`keys.ts`); the process gets `childEnv()`, the
// protocol version and its key's private temp and nothing else (no `FamilySpec.env`, no
// credential, no socket to finisherd: 5C passes only stdio), 2 GiB of memory, 8 processes and
// 1,024 open files. 5C's worker codes become the plugin codes of spec §13 when a job fails.
import type { FinErrorData } from "@finisher/contracts";
import { childEnv } from "../../env.ts";
import type { FamilySpec, KindRegistry, MemoryRule, ProgramSpec } from "../../jobs/kinds.ts";
import { ensurePrivateDir, keyTmpDir, pluginWarmKey, type RootSessionOf, sweepPluginTmp } from "./keys.ts";
import { nodeRuntimeEnv } from "./node-runtime.ts";
import { type InstalledOf, jobRow, resolveRuntime } from "./program.ts";

export const PLUGIN_FAMILY = "plugin";
/** The envelope's plugin protocol a process speaks (`FIN_PLUGIN_PROTOCOL`). */
export const PLUGIN_PROTOCOL = "1";
/** Warm processes per key: 5 minutes idle (spec §3.3); up to 16 keys at once (Decision 3a). */
export const PLUGIN_WARM = { idleS: 300, slots: 16 } as const;
/** Decision 9 (before 7C's profile supplies them). */
export const PLUGIN_MEMORY: MemoryRule = { fixedMiB: 2048 };
export const PLUGIN_LIMITS = { processes: 8, openFiles: 1024 } as const;

export interface PluginFamilyDeps {
  /** Server data with links resolved (the grants are canonical, plan 7C Task 7). */
  readonly serverData: string;
  readonly installed: InstalledOf;
  readonly rootSessionOf?: RootSessionOf;
  /** Told every key a process is started for (its temp directory is removed once it stops). */
  readonly onKey?: (key: string) => void;
  /** Tests: the runtime's Node, platform and versions (`process.*` otherwise). */
  readonly execPath?: string;
  readonly platform?: string;
  readonly versions?: Readonly<Record<string, string | undefined>>;
}

/** 5C's codes as a plugin job's (spec §13; `WORKER_OOM` and the rest are kept). */
export function mapPluginError(e: FinErrorData): FinErrorData {
  const details = e.details !== null && typeof e.details === "object" ? (e.details as Record<string, unknown>) : {};
  const as = (code: string): FinErrorData => ({ ...e, code, details: { ...details, cause: e.code } });
  switch (e.code) {
    case "WORKER_CRASHED":
      // A broken envelope is the plugin's protocol, not a crash.
      return as(details["protocol"] === undefined ? "PLUGIN_CRASHED" : "PLUGIN_PROTOCOL");
    case "WORKER_HUNG":
    // The kind's own `timeoutS` (a command's 60 s ceiling); Task 9 cancels a command at its
    // manifest timeout and answers `PLUGIN_TIMEOUT` itself (Decision 7).
    case "JOB_TIMEOUT":
      return as("PLUGIN_TIMEOUT");
    case "WORKER_PROTOCOL_MISMATCH":
      return as("PLUGIN_PROTOCOL");
    default:
      return e;
  }
}

/** The program, environment, grants and limits of a plugin job's process. */
export function pluginProgram(deps: PluginFamilyDeps, job: Parameters<typeof pluginWarmKey>[0], warmKey?: string): ProgramSpec {
  // A warm spawn's key is the one the engine tagged the worker with, so its `TMPDIR` and its
  // key can never disagree; a cold spawn computes it.
  const key = warmKey ?? pluginWarmKey(job, deps.rootSessionOf);
  const tmp = keyTmpDir(deps.serverData, key);
  const rt = resolveRuntime({ serverData: deps.serverData, installed: deps.installed, ...(deps.execPath === undefined ? {} : { execPath: deps.execPath }), ...(deps.platform === undefined ? {} : { platform: deps.platform }) }, job, tmp);
  ensurePrivateDir(tmp);
  deps.onKey?.(key);
  const env: Record<string, string> = {
    ...childEnv(),
    FIN_PLUGIN_PROTOCOL: PLUGIN_PROTOCOL,
    TMPDIR: tmp,
    TEMP: tmp,
    TMP: tmp,
    ...(rt.runtime === "node" ? nodeRuntimeEnv(deps.versions) : {}),
  };
  return {
    path: rt.path,
    args: rt.args,
    process: {
      env,
      // One directory of server data, not the jobs root (7C's `plugin_input`).
      cwd: tmp,
      grants: [
        { path: tmp, mode: "readWrite" },
        // The one package in the store; Node itself outside server data.
        { path: rt.storeDir, mode: "read" },
        ...(rt.runtime === "node" ? [{ path: rt.path, mode: "read" as const }] : []),
      ],
      limits: PLUGIN_LIMITS,
    },
  };
}

export function pluginFamily(deps: PluginFamilyDeps): FamilySpec {
  return {
    family: PLUGIN_FAMILY,
    profile: "plugin",
    program: (job, key) => pluginProgram(deps, job, key),
    warmKey: (job) => pluginWarmKey(job, deps.rootSessionOf),
    usable: (job) => {
      const found = jobRow(deps.installed, job);
      return "error" in found ? found.error : undefined;
    },
    warm: PLUGIN_WARM,
    memory: { interactive: PLUGIN_MEMORY, batch: PLUGIN_MEMORY },
    mapError: mapPluginError,
  };
}

/**
 * Registers the family (the plugin layer's part of the engine's start: a restart reconciles
 * jobs against the registry, so it is registered before the engine starts) and empties the
 * keys' temp directories.
 */
export function registerPluginFamily(registry: KindRegistry, deps: PluginFamilyDeps): void {
  sweepPluginTmp(deps.serverData);
  registry.registerFamily(pluginFamily(deps));
}
