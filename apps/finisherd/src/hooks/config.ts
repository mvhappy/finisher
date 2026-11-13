// Where hooks are configured (7F plan Decisions 5–8; spec §9.3, §9.4), in the order they run:
//
// - **managed** `<server-data>/managed-settings.json` and **user** `<server-data>/settings.json`,
//   key `hooks`; **project** `finisher.hooks.json` beside `finisher.project.json` and **local**
//   `<server-data>/projects/<id>/hooks.json`, both `{"hooks": [...]}`; **plugin**: the enabled
//   code plugins' `contributes.hooks` (a port; Task 10 fills it), by plugin id then their order.
//   A person edits the files: hooks are not a `settings.write` path.
// - **Fresh at every read** (Decision 5): each file is opened and `fstat`ed at every call, its
//   parsed hooks kept by `(path, ino, size, mtime, ctime)` (`file-cache.ts`), so a changed,
//   deleted or replaced file never applies after the change. A dispatch reads the project file
//   only while its trust record holds a `hooksHash` other than `none` (an untrusted file
//   applies to nothing); listing and the start warning always read it.
// - **A managed, user or local layer that cannot be read or decoded** stands as a synthetic
//   hook `<layer>.unreadable` on every `blockingBefore` event (Decision 8): it blocks them
//   (`HOOK_BLOCKED {reason: unreadable}`, the engine's) and is listed. No file, or no `hooks`
//   key, is no hooks. A file is unreadable as a whole when any hook in it does not decode, two
//   share an id, or it holds more than 64.
// - **Project trust** (as `trust.ts`'s rules): the file travels with the project (an agent could
//   edit it), so its hooks apply only while its hash is `project_trust.hooksHash`; untrusted,
//   changed, deleted or unreadable it simply applies nothing. Trusting binds what the person
//   saw (`checkTrustedHooks`, from `agent.project.trust`'s `expectedHooksHash`).
// - **Paths** (Decision 6): `run[0]` with a separator, and `file`, are relative to the folder
//   of the file that declares them unless absolute; absent `file` is `run[0]` when that is such
//   a declaring-folder path. A bare program name is kept as written (Task 3 resolves it).
import { isAbsolute, join, resolve } from "node:path";
import { type AgentHooksListSuccess, type AgentWarning, HOOK_CLASSES, HOOK_LIMITS, type HookEvent, type HookLayer, HookSpec, type ListedHook } from "@finisher/contracts";
import { Schema } from "effect";
import { settingsPath } from "../config.ts";
import { finError } from "../errors.ts";
import { firstLine, type Read, readCached } from "../agent/file-cache.ts";
import type { ProjectTrust } from "../agent/rule-store.ts";
import { MANAGED_SETTINGS, NO_FILE } from "../agent/trust.ts";

export const HOOKS_FILE = "finisher.hooks.json";
/** The local layer's file in `<server-data>/projects/<id>/`. */
export const LOCAL_HOOKS = "hooks.json";

/** A hook a plugin contributes (Task 10 fills the port). */
export interface PluginHookRow {
  readonly plugin: string;
  readonly event: HookEvent;
  readonly handler: string;
}

/** One configured hook, paths resolved. */
export interface ConfiguredHook {
  readonly layer: HookLayer;
  /** The file it came from, or the plugin's id. */
  readonly source: string;
  readonly id: string;
  readonly event: HookEvent;
  /** As configured (absent: a plugin's or a synthetic hook). */
  readonly spec?: HookSpec;
  /** The program (resolved when it is a declaring-folder path) and its arguments. */
  readonly run?: readonly string[];
  /** The hook's own file (read-only to it), resolved. */
  readonly file?: string;
  /** The folder of the file that declares it. */
  readonly dir?: string;
  /** The synthetic hook of an unreadable layer: it blocks. */
  readonly unreadable?: true;
  readonly plugin?: string;
}

export type ProjectHooksStatus = "none" | "trusted" | "untrusted" | "changed";
export type ProjectHooksReason = "deleted" | "unreadable" | "invalidHooks";

export interface ProjectHooks {
  readonly status: ProjectHooksStatus;
  /** The hooks that apply (none unless `trusted`). */
  readonly hooks: readonly ConfiguredHook[];
  /** The file's hooks that do not apply (untrusted, changed), for listing. */
  readonly ignored: readonly ConfiguredHook[];
  /** The file's hash now (`none`: no file), what a person trusts; absent when it cannot be read. */
  readonly hooksHash?: string;
  readonly reason?: ProjectHooksReason;
}

export interface HookConfigDeps {
  readonly serverData: string;
  readonly trustOf: (projectId: string) => ProjectTrust | undefined;
  /** The folder of a project (open, else indexed). */
  readonly dirOf: (projectId: string) => string | undefined;
  readonly plugins?: () => readonly PluginHookRow[];
}

const BLOCKING: readonly HookEvent[] = (Object.keys(HOOK_CLASSES) as HookEvent[]).filter((e) => HOOK_CLASSES[e] === "blockingBefore");
/** A project id as a folder name: anything else has no local layer. */
const SAFE_ID = /^[A-Za-z0-9_-]{1,128}$/;
const decodeSpec = Schema.decodeUnknownSync(HookSpec);
const isObject = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const hasSeparator = (p: string) => p.includes("/") || (process.platform === "win32" && p.includes("\\"));

/** The synthetic hooks of a layer that cannot be read: one per event whose block refuses an action. */
function unreadableHooks(layer: HookLayer, source: string): ConfiguredHook[] {
  return BLOCKING.map((event) => ({ layer, source, id: `${layer}.unreadable`, event, unreadable: true }));
}

/** One file's hooks, decoded one by one so an error names the hook (`hook "<id>": …`). */
function decodeHooks(list: unknown, layer: HookLayer, source: string, dir: string): ConfiguredHook[] {
  if (!Array.isArray(list)) throw new Error("hooks is not a list");
  if (list.length > HOOK_LIMITS.perLayer) throw new Error(`more than ${HOOK_LIMITS.perLayer} hooks`);
  const seen = new Set<string>();
  return list.map((raw, i) => {
    const name = isObject(raw) && typeof raw.id === "string" ? JSON.stringify(raw.id.slice(0, 64)) : `#${i}`;
    let spec: HookSpec;
    try {
      spec = decodeSpec(raw, { onExcessProperty: "error" });
    } catch (e) {
      throw new Error(`hook ${name}: ${firstLine(e)}`);
    }
    if (seen.has(spec.id)) throw new Error(`hook ${name}: a second hook with this id`);
    seen.add(spec.id);
    const [program, ...args] = spec.run as [string, ...string[]];
    const local = !isAbsolute(program) && hasSeparator(program);
    const run = [local ? resolve(dir, program) : program, ...args];
    const file = spec.file !== undefined ? resolve(dir, spec.file) : local ? run[0] : undefined;
    return { layer, source, id: spec.id, event: spec.event, spec, run, ...(file === undefined ? {} : { file }), dir };
  });
}

/** `{"hooks": [...]}` (the project's and local files); `key` optional (settings files). */
function hooksFile(path: string, slot: string, layer: HookLayer, source: string, dir: string, optional: boolean): Read<ConfiguredHook[]> {
  return readCached(path, `hooks\0${slot}\0${path}`, (j) => {
    if (!isObject(j)) throw new Error("not an object");
    if (j.hooks === undefined && optional) return [];
    if (j.hooks === undefined) throw new Error('a hooks file is {"hooks": [...]}');
    return decodeHooks(j.hooks, layer, source, dir);
  });
}

/** A layer that always applies: its hooks, none without a file, or its synthetic hooks when unreadable. */
function layerHooks(read: Read<ConfiguredHook[]>, layer: HookLayer, source: string): readonly ConfiguredHook[] {
  if ("absent" in read) return [];
  return "hash" in read && read.value !== undefined ? read.value : unreadableHooks(layer, source);
}

function readProject(dir: string): Read<ConfiguredHook[]> {
  return hooksFile(join(dir, HOOKS_FILE), "project", "project", HOOKS_FILE, dir, false);
}

/** A project's hooks file and whether it applies, against its trust record. */
export function projectHooks(dir: string | undefined, trust: ProjectTrust | undefined): ProjectHooks {
  const none: ProjectHooks = { status: "none", hooks: [], ignored: [] };
  if (dir === undefined) return none;
  const trusted = trust?.hooksHash;
  const read = readProject(dir);
  if ("absent" in read) {
    // A trusted file since deleted is a change (its hooks no longer apply), not "no file".
    return trusted !== undefined && trusted !== NO_FILE ? { status: "changed", hooks: [], ignored: [], hooksHash: NO_FILE, reason: "deleted" } : { ...none, hooksHash: NO_FILE };
  }
  if ("error" in read) return { status: trusted === undefined ? "untrusted" : "changed", hooks: [], ignored: [], reason: "unreadable" };
  const status: ProjectHooksStatus = trusted === undefined ? "untrusted" : trusted === read.hash ? "trusted" : "changed";
  const reason = read.value === undefined ? { reason: "invalidHooks" as const } : {};
  if (status === "trusted") return { status, hooks: read.value ?? [], ignored: [], hooksHash: read.hash, ...reason };
  return { status, hooks: [], ignored: read.value ?? [], hooksHash: read.hash, ...reason };
}

/**
 * Checks a project's hooks file against what the person saw (`agent.project.trust`'s
 * `expectedHooksHash`): its hash, or `none` for no file, must be `expected`, else
 * `INVALID_REQUEST {field: expectedHooksHash, reason: changed}`; a file that cannot be read or
 * does not read as hooks is `INVALID_REQUEST`. Records nothing: the caller does once every
 * check passed. Answers the hash to record.
 */
export function checkTrustedHooks(dir: string, projectId: string, expected: string): string {
  const read = readProject(dir);
  if ("error" in read) throw finError("INVALID_REQUEST", `${HOOKS_FILE} cannot be read`, { projectId, reason: "unreadable" }, `fix or remove ${HOOKS_FILE}, then trust the project again`);
  const hash = "absent" in read ? NO_FILE : read.hash;
  if (hash !== expected) {
    throw finError("INVALID_REQUEST", `${HOOKS_FILE} changed since it was shown`, { projectId, field: "expectedHooksHash", reason: "changed", hooksHash: hash }, "list the project's hooks again and trust what they show now");
  }
  if ("hash" in read && read.value === undefined) {
    throw finError("INVALID_REQUEST", `${HOOKS_FILE} is not a hooks file: ${read.invalid ?? ""}`, { projectId, reason: "invalidHooks" }, `a hooks file is {"hooks": [{"id", "event", "run", ...}]}`);
  }
  return hash;
}

const inert = (event: HookEvent) => (event === "PreRender" || event === "PostRender" ? { inert: "noRenderKind" as const } : {});

/** A configured hook as `agent.hooks.list` shows it. */
export function listedHook(h: ConfiguredHook, applies: boolean): ListedHook {
  return {
    layer: h.layer,
    source: h.source,
    id: h.id,
    event: h.event,
    applies,
    ...inert(h.event),
    ...(h.plugin === undefined ? {} : { plugin: h.plugin }),
    ...(h.spec === undefined ? {} : { run: [...h.spec.run] }),
    ...(h.spec?.match === undefined ? {} : { match: h.spec.match }),
  };
}

/** Every layer's hooks, read fresh at each call (see the module docs). */
export class HookConfig {
  readonly #d: HookConfigDeps;

  constructor(d: HookConfigDeps) {
    this.#d = d;
  }

  #server(): readonly ConfiguredHook[] {
    const data = this.#d.serverData;
    return [
      ...layerHooks(hooksFile(join(data, MANAGED_SETTINGS), "managed", "managed", MANAGED_SETTINGS, data, true), "managed", MANAGED_SETTINGS),
      ...layerHooks(hooksFile(settingsPath(data), "user", "user", "settings.json", data, true), "user", "settings.json"),
    ];
  }

  #local(projectId: string): readonly ConfiguredHook[] {
    if (!SAFE_ID.test(projectId)) return [];
    const dir = join(this.#d.serverData, "projects", projectId);
    return layerHooks(hooksFile(join(dir, LOCAL_HOOKS), "local", "local", LOCAL_HOOKS, dir, false), "local", LOCAL_HOOKS);
  }

  #plugins(): readonly ConfiguredHook[] {
    const rows = [...(this.#d.plugins?.() ?? [])];
    // By plugin id, then each plugin's own order (a stable sort keeps it).
    rows.sort((a, b) => (a.plugin < b.plugin ? -1 : a.plugin > b.plugin ? 1 : 0));
    return rows.map((r) => ({ layer: "plugin", source: r.plugin, id: r.handler, event: r.event, plugin: r.plugin }));
  }

  /** A project's hooks file and whether it applies. */
  project(projectId: string): ProjectHooks {
    return projectHooks(this.#d.dirOf(projectId), this.#d.trustOf(projectId));
  }

  /**
   * The hooks that apply to an event in `projectId` (or to a server event with none), in order:
   * managed, user, project, local, plugin. The project file is read only while trusted.
   */
  hooks(projectId: string | undefined): readonly ConfiguredHook[] {
    if (projectId === undefined) return [...this.#server(), ...this.#plugins()];
    const trusted = this.#d.trustOf(projectId)?.hooksHash;
    const project = trusted === undefined || trusted === NO_FILE ? [] : this.project(projectId).hooks;
    return [...this.#server(), ...project, ...this.#local(projectId), ...this.#plugins()];
  }

  /** `agent.hooks.list`'s answer: every layer's hooks, a project file's with whether it applies. */
  list(projectId: string | undefined): AgentHooksListSuccess {
    if (projectId === undefined) return { hooks: [...this.#server(), ...this.#plugins()].map((h) => listedHook(h, true)) };
    const p = this.project(projectId);
    const file = [...p.hooks.map((h) => listedHook(h, true)), ...p.ignored.map((h) => listedHook(h, false))];
    const server = this.#server().map((h) => listedHook(h, true));
    const rest = [...this.#local(projectId), ...this.#plugins()].map((h) => listedHook(h, true));
    return {
      hooks: [...server, ...file, ...rest],
      projectHooks: p.status,
      ...(p.hooksHash === undefined ? {} : { hooksHash: p.hooksHash }),
      ...(p.reason === undefined ? {} : { projectHooksReason: p.reason }),
    };
  }

  /** `PROJECT_HOOKS_UNTRUSTED` while the project has a hooks file that does not apply (Owner question 8). */
  warnings(projectId: string): AgentWarning[] {
    const p = this.project(projectId);
    if (p.status !== "untrusted" && p.status !== "changed" && p.reason === undefined) return [];
    return [
      {
        code: "PROJECT_HOOKS_UNTRUSTED",
        message: `the project's ${HOOKS_FILE} does not apply: its hooks are ${p.status === "changed" ? "changed since they were trusted" : p.status === "untrusted" ? "not trusted" : "not readable"}`,
        details: { projectId, status: p.status, ...(p.reason === undefined ? {} : { reason: p.reason }) },
        fix: `review ${HOOKS_FILE}, then run \`finisherd project trust ${projectId}\``,
      },
    ];
  }
}
