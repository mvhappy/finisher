// The one `plugin` family (sp8b plan Decisions 2, 3, 9; Task 4): every code plugin's process
// is a 5C worker of this family (`Program::Path`, profile `plugin`), whatever plugin it runs.
// `program(job)` resolves the job's plugin (`program.ts`); `warmKey` keeps one process per
// (plugin@version, project, authority key) (`keys.ts`, `authority.ts`); the process gets
// `childEnv()`, the protocol version and its key's private temp and nothing else (no
// `FamilySpec.env`, no credential, no socket to finisherd: 5C passes only stdio), its caller's
// `pluginData` directory when `fs pluginData` is granted, 2 GiB of memory, 8 processes and
// 1,024 open files. 5C's worker codes become the plugin codes of spec §13 when a job fails.
import { type FinErrorData, JOB_ERROR_CODES } from "@finisher/contracts";
import { FIN_ERROR_CODES } from "../../console/exit-codes.ts";
import { type AuthorityGenerations, jobAuthority, jobDataDir, memoryGenerations } from "./authority.ts";
import { childEnv } from "../../env.ts";
import { finError } from "../../errors.ts";
import type { FamilySpec, KindRegistry, MemoryRule, ProgramSpec } from "../../jobs/kinds.ts";
import { bareKey, ensurePrivateDir, keyTmpDir, pluginWarmKey, principalClass, type RootSessionOf, sweepPluginTmp } from "./keys.ts";
import { rightsHash } from "./rights.ts";
import { scrubLogText } from "./log.ts";
import { pluginAnswerError } from "./plugin-errors.ts";
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
  /** The classes' policy generations (`storedGenerations` in the server; all 0 when omitted). */
  readonly generations?: AuthorityGenerations;
  /**
   * Told every key a process is started for (its temp directory is removed once it stops);
   * `coldJob`: the job a cold process was started for (its calls check that key, Task 7).
   */
  readonly onKey?: (key: string, cold?: { readonly job: string; readonly plugin: string }) => void;
  /** Host calls (Task 7). */
  readonly onCall?: FamilySpec["onCall"];
  /** The processes' stderr (Task 7): scrubbed and capped into the server log. */
  readonly onStderr?: FamilySpec["onStderr"];
  /** Tests: the runtime's Node, platform and versions (`process.*` otherwise). */
  readonly execPath?: string;
  readonly platform?: string;
  readonly versions?: Readonly<Record<string, string | undefined>>;
}

/** Codes the host knows: the FinError table and 5C's job codes. */
const HOST_CODES: ReadonlySet<string> = new Set([...FIN_ERROR_CODES, ...Object.keys(JOB_ERROR_CODES)]);

/**
 * 5C's codes as a plugin job's (spec §13; `WORKER_OOM` and the rest are kept). A code the host
 * does not know is the plugin's own: free text that must not reach `job.done`, `plugin.command`
 * or `plugin.stats` (nor fail them for its pattern), so it becomes `PLUGIN_COMMAND_FAILED` and is
 * not kept in `details` (sp8b Task 15 review).
 */
export function mapPluginError(raw: FinErrorData): FinErrorData {
  const given = raw.details !== null && typeof raw.details === "object" ? (raw.details as Record<string, unknown>) : {};
  // A crash's `stderrTail` is the plugin's own text: scrubbed like its stderr lines before it is
  // stored or answered (review of Task 7: paths otherwise reach control.db and non-human callers).
  const details = typeof given["stderrTail"] === "string" ? { ...given, stderrTail: scrubLogText(given["stderrTail"]) } : given;
  const e: FinErrorData = details === given ? raw : { ...raw, details };
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
      return HOST_CODES.has(e.code) ? e : { code: "PLUGIN_COMMAND_FAILED", message: "the plugin's job failed", details: { cause: "plugin" } };
  }
}

/** The program, environment, grants and limits of a plugin job's process. */
export function pluginProgram(deps: PluginFamilyDeps, job: Parameters<typeof pluginWarmKey>[0], warmKey?: string): ProgramSpec {
  // A warm spawn's key is the one the engine tagged the worker with, so its `TMPDIR` and its
  // key can never disagree; a cold spawn computes it.
  const gens = deps.generations ?? memoryGenerations();
  // Under a sandbox the pool's tag is `<project>/<key>` (`warm.ts`): the temp directory is the
  // authority key's, as `contextOf` and the calls name it (review of Task 7).
  const key = warmKey !== undefined ? bareKey(warmKey) : pluginWarmKey(job, deps.rootSessionOf, gens);
  const tmp = keyTmpDir(deps.serverData, key);
  const data = pluginDataFor(deps, job);
  const rt = resolveRuntime({ serverData: deps.serverData, installed: deps.installed, ...(deps.execPath === undefined ? {} : { execPath: deps.execPath }), ...(deps.platform === undefined ? {} : { platform: deps.platform }) }, job, tmp, data);
  ensurePrivateDir(tmp);
  deps.onKey?.(key, warmKey === undefined ? { job: job.id, plugin: job.via?.plugin ?? "" } : undefined);
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

/**
 * `PluginContext.dirs.data` of `job` (spec §5.1; Task 7 names it in the context): the caller's
 * `pluginData` directory for the job's authority, created on this first use; `undefined`
 * without an `fs pluginData` grant.
 */
export function pluginDataFor(deps: Pick<PluginFamilyDeps, "serverData" | "rootSessionOf" | "generations">, job: Parameters<typeof pluginWarmKey>[0]): { readonly path: string; readonly write: boolean } | undefined {
  const authority = jobAuthority(job, principalClass(job.owner, deps.rootSessionOf), deps.generations ?? memoryGenerations());
  return jobDataDir(deps.serverData, job, authority);
}

export function pluginFamily(deps: PluginFamilyDeps): FamilySpec {
  const gens = deps.generations ?? memoryGenerations();
  return {
    family: PLUGIN_FAMILY,
    profile: "plugin",
    program: (job, key) => pluginProgram({ ...deps, generations: gens }, job, key),
    warmKey: (job) => pluginWarmKey(job, deps.rootSessionOf, gens),
    checkRights: (owner, rights) => {
      const cls = principalClass(owner, deps.rootSessionOf);
      if (rights.class !== cls) return finError("INVALID_REQUEST", "a plugin job's rights are for its owner's principal class", { field: "rights", class: rights.class });
      // The authority the host computed: its hash names these rights, its generation is not
      // from the future (the key uses the class's generation now in any case).
      if (rights.rightsHash !== rightsHash(rights)) return finError("INVALID_REQUEST", "a plugin job's rights hash does not match its rights", { field: "rights", reason: "rightsHash" });
      if (rights.generation > gens.of(cls)) return finError("INVALID_REQUEST", "a plugin job's rights name a policy generation that has not happened", { field: "rights", reason: "generation" });
      return undefined;
    },
    usable: (job) => {
      const found = jobRow(deps.installed, job);
      return "error" in found ? found.error : undefined;
    },
    warm: PLUGIN_WARM,
    ...(deps.onCall === undefined ? {} : { onCall: deps.onCall }),
    ...(deps.onStderr === undefined ? {} : { onStderr: deps.onStderr }),
    memory: { interactive: PLUGIN_MEMORY, batch: PLUGIN_MEMORY },
    mapError: mapPluginError,
    workerError: pluginAnswerError,
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
