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
// - **`hooksHash` binds the project files the hooks name** (review of Task 2): each `run[0]`
//   with a separator and each `file` that is project content (relative, or absolute inside the
//   project folder) is read like the hooks file at every read, and `hooksHash` (`hooksHashOf`)
//   is `sha256:` over the hooks file's hash and the sorted `[relative path, hash]` pairs, so an
//   edited, deleted or replaced script is `changed` at once. A relative path out of the project
//   (by `..`, or through a symlink: checked on `realpath` whenever the file's key changes) makes
//   the file untrustable (`invalidHooks`); a missing one too (`unreadable`), with no hash. An
//   absolute path outside the project and a bare program name are not project content.
// - **Paths** (Decision 6): `run[0]` with a separator, and `file`, are relative to the folder
//   of the file that declares them unless absolute; absent `file` is `run[0]` when that is such
//   a declaring-folder path. The program is resolved at load into `program` (`programOf`):
//   `node` is finisherd's own Node (`process.execPath`), a path is resolved as above (for a
//   project hook exactly the path its `hooksHash` binds), another bare name is looked up in the
//   toolchain base when the hook runs (`hooks/process.ts`; the parsed file is kept by its key,
//   so a lookup at load would outlive an install), never in the user's `PATH`. On Windows
//   anything but `node` must be an `.exe`: a `.cmd`, `.bat` or other file a shell would run
//   makes the file unreadable (nothing runs through a shell, Decision 9).
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { isAbsolute, join, posix, relative, resolve, sep, win32 } from "node:path";
import { type AgentHooksListSuccess, type AgentWarning, HOOK_CLASSES, HOOK_LIMITS, type HookEvent, type HookLayer, HookSpec, type ListedHook } from "@finisher/contracts";
import { Effect, Schema } from "effect";
import { settingsPath } from "../config.ts";
import { finError } from "../errors.ts";
import { firstLine, type Read, readCached, readCachedBytes } from "../agent/file-cache.ts";
import type { ProjectTrust } from "../agent/rule-store.ts";
import { MANAGED_SETTINGS, NO_FILE } from "../agent/trust.ts";

export const HOOKS_FILE = "finisher.hooks.json";
/** The local layer's file in `<server-data>/projects/<id>/`. */
export const LOCAL_HOOKS = "hooks.json";

/** A hook a plugin contributes (Task 10: `plugins/host/hooks.ts`). */
export interface PluginHookRow {
  readonly plugin: string;
  readonly event: HookEvent;
  readonly handler: string;
  /** The manifest's `timeoutS` (default 10, at most 60). */
  readonly timeoutS?: number;
}

/**
 * A hook's program (Decision 6): a path (`node`: finisherd's own Node, its install directory
 * then readable too), or a bare name looked up in the toolchain base at each run.
 */
export type HookProgram = { readonly path: string; readonly node?: true } | { readonly bare: string };

/**
 * `run[0]` resolved against `dir`, the folder of the file that declares it. On Windows anything
 * but `node` must be an `.exe` (throws otherwise): nothing runs through a shell.
 */
export function programOf(program: string, dir: string, platform: NodeJS.Platform = process.platform): HookProgram {
  if (program === "node") return { path: process.execPath, node: true };
  if (platform === "win32" && !/\.exe$/i.test(program)) throw new Error(`on Windows a hook's program is node or an .exe (nothing runs through a shell): ${program}`);
  const path = platform === "win32" ? win32 : posix;
  if (path.isAbsolute(program)) return { path: program };
  if (program.includes("/") || (platform === "win32" && program.includes("\\"))) return { path: path.resolve(dir, program) };
  return { bare: program };
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
  /** `run[0]` resolved (`programOf`). */
  readonly program?: HookProgram;
  /** The hook's own file (read-only to it), resolved. */
  readonly file?: string;
  /**
   * A trusted project hook's project files (its program, its own file) by resolved path, each
   * with the hash its `hooksHash` binds: the bytes that may run (`hooks/process.ts` runs a
   * verified copy).
   */
  readonly bound?: Readonly<Record<string, string>>;
  /** The folder of the file that declares it. */
  readonly dir?: string;
  /** The synthetic hook of an unreadable layer: it blocks. */
  readonly unreadable?: true;
  /** A plugin handler's plugin id (`id` is then its handler) and its timeout. */
  readonly plugin?: string;
  readonly timeoutS?: number;
}

export type ProjectHooksStatus = "none" | "trusted" | "untrusted" | "changed";
export type ProjectHooksReason = "deleted" | "unreadable" | "invalidHooks";

export interface ProjectHooks {
  readonly status: ProjectHooksStatus;
  /** The hooks that apply (none unless `trusted`). */
  readonly hooks: readonly ConfiguredHook[];
  /** The file's hooks that do not apply (untrusted, changed), for listing. */
  readonly ignored: readonly ConfiguredHook[];
  /**
   * The hash a person trusts now (`hooksHashOf`; `none`: no file); absent when the file or a
   * project file it names cannot be read, or a path leaves the project.
   */
  readonly hooksHash?: string;
  /** The project files (relative, `/`-separated, sorted) the hash binds besides the hooks file. */
  readonly files: readonly string[];
  readonly reason?: ProjectHooksReason;
  /** Why it does not read as trustable hooks, for a person. */
  readonly invalid?: string;
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
    let program: HookProgram;
    try {
      program = programOf(spec.run[0]!, dir);
    } catch (e) {
      throw new Error(`hook ${name}: ${firstLine(e)}`);
    }
    const [first, ...args] = spec.run as [string, ...string[]];
    const local = "path" in program && program.node !== true && !isAbsolute(first) ? program.path : undefined;
    const run = [local ?? first, ...args];
    const file = spec.file !== undefined ? resolve(dir, spec.file) : local;
    return { layer, source, id: spec.id, event: spec.event, spec, run, program, ...(file === undefined ? {} : { file }), dir };
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

/** A script a hook names is read up to this size (a larger one cannot be trusted). */
const REFERENCED_MAX = 16 * 1024 * 1024;
const sha256 = (text: string) => `sha256:${createHash("sha256").update(text).digest("hex")}`;
/** `rel` (from `relative`) stays inside the folder. */
const inside = (rel: string) => rel !== "" && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);

/** The trusted hash of a hooks file whose bytes hash to `fileHash` and the project files it names. */
export function hooksHashOf(fileHash: string, files: ReadonlyArray<readonly [string, string]>): string {
  const sorted = [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return sha256(JSON.stringify({ file: fileHash, files: sorted }));
}

/** A problem with the project files the hooks name: no hash can be trusted. */
interface Problem {
  readonly reason: "invalidHooks" | "unreadable";
  readonly invalid: string;
}

/** `p` resolved, through links where it exists. */
function real(p: string): string {
  try {
    return realpathSync(p);
  } catch {
    return resolve(p);
  }
}

/**
 * Why a project hook's arguments or program cannot be bound, if they cannot (review of Task 3):
 * a project script comes in only through `run[0]` or `file`, so an argument may not name the
 * project (an absolute path inside it, or any `..` segment); and a program in the project is
 * the hook's own file (Linux executes nothing else from the hook's temp, where it runs a copy).
 */
function unbound(dir: string, h: ConfiguredHook): string | undefined {
  const spec = h.spec;
  if (spec === undefined) return undefined;
  const realDir = real(dir);
  for (const a of spec.run.slice(1)) {
    if (a.split(/[\\/]/).includes("..")) return `hook "${h.id}": the argument ${a} has a .. segment (a project script is named by file)`;
    if (isAbsolute(a) && (inside(relative(dir, resolve(a))) || resolve(a) === resolve(dir) || inside(relative(realDir, real(a))))) {
      return `hook "${h.id}": the argument ${a} names the project folder (a project script is named by file)`;
    }
  }
  const program = h.program !== undefined && "path" in h.program && h.program.node !== true ? h.program.path : undefined;
  if (program !== undefined && inside(relative(dir, program)) && h.file !== undefined && h.file !== program) {
    return `hook "${h.id}": its program ${spec.run[0]} is in the project, so it is the hook's own file (drop file, or run it with node)`;
  }
  return undefined;
}

/** The project files `hooks` name (relative, `/`-separated, sorted), or the first path that leaves the project. */
function referenced(dir: string, hooks: readonly ConfiguredHook[]): { files: string[] } | Problem {
  const files = new Set<string>();
  for (const h of hooks) {
    const why = unbound(dir, h);
    if (why !== undefined) return { reason: "invalidHooks", invalid: why };
    const program = h.spec?.run[0];
    const named = [...(program !== undefined && hasSeparator(program) ? [program] : []), ...(h.spec?.file === undefined ? [] : [h.spec.file])];
    for (const path of named) {
      const rel = relative(dir, resolve(dir, path));
      if (inside(rel)) files.add(rel.split(sep).join("/"));
      else if (!isAbsolute(path)) return { reason: "invalidHooks", invalid: `hook "${h.id}": ${path} is outside the project` };
    }
  }
  return { files: [...files].sort() };
}

/** Each named file's hash, re-validated as the hooks file is; a file whose real path leaves the project is invalid. */
function hashFiles(dir: string, files: readonly string[]): Array<[string, string]> | Problem {
  const pairs: Array<[string, string]> = [];
  for (const rel of files) {
    const path = join(dir, ...rel.split("/"));
    const read = readCachedBytes(path, `hookfile\0${path}`, () => {
      // On every change of the file's key: a symlink (of the file or a folder) out of the project is not project content.
      if (!inside(relative(realpathSync(dir), realpathSync(path)))) throw new Error(`${rel} resolves outside the project`);
      return true;
    }, REFERENCED_MAX);
    if ("absent" in read) return { reason: "unreadable", invalid: `${rel} does not exist` };
    if ("error" in read) return { reason: "unreadable", invalid: `${rel} cannot be read: ${read.error}` };
    if (read.value === undefined) return { reason: "invalidHooks", invalid: read.invalid ?? rel };
    pairs.push([rel, read.hash]);
  }
  return pairs;
}

type ProjectRead =
  | { readonly absent: true }
  | { readonly error: string }
  | { readonly hooks?: ConfiguredHook[]; readonly hash?: string; readonly files: string[]; readonly pairs?: ReadonlyArray<readonly [string, string]>; readonly problem?: Problem; readonly invalid?: string };

/** The project's hooks file and the files it names: the hash to trust, or why there is none. */
function readProjectAll(dir: string): ProjectRead {
  const read = readProject(dir);
  if (!("hash" in read)) return read;
  // Not hooks: its own hash stands (trusting it is refused for what it is).
  if (read.value === undefined) return { hash: hooksHashOf(read.hash, []), files: [], ...(read.invalid === undefined ? {} : { invalid: read.invalid }) };
  const refs = referenced(dir, read.value);
  if ("reason" in refs) return { hooks: read.value, files: [], problem: refs };
  const pairs = hashFiles(dir, refs.files);
  if ("reason" in pairs) return { hooks: read.value, files: refs.files, problem: pairs };
  return { hooks: read.value, hash: hooksHashOf(read.hash, pairs), files: refs.files, pairs };
}

/** Each hook with the hashes of the project files it names (`ConfiguredHook.bound`). */
function withBound(dir: string, hooks: readonly ConfiguredHook[], pairs: ReadonlyArray<readonly [string, string]>): ConfiguredHook[] {
  const byPath = new Map(pairs.map(([rel, hash]) => [join(dir, ...rel.split("/")), hash]));
  return hooks.map((h) => {
    const program = h.program !== undefined && "path" in h.program ? h.program.path : undefined;
    const bound = Object.fromEntries([program, h.file].flatMap((p) => (p !== undefined && byPath.has(p) ? [[p, byPath.get(p)!]] : [])));
    return Object.keys(bound).length === 0 ? h : { ...h, bound };
  });
}

/** A project's hooks file and whether it applies, against its trust record. */
export function projectHooks(dir: string | undefined, trust: ProjectTrust | undefined): ProjectHooks {
  const none: ProjectHooks = { status: "none", hooks: [], ignored: [], files: [] };
  if (dir === undefined) return none;
  const trusted = trust?.hooksHash;
  const read = readProjectAll(dir);
  if ("absent" in read) {
    // A trusted file since deleted is a change (its hooks no longer apply), not "no file".
    return trusted !== undefined && trusted !== NO_FILE ? { ...none, status: "changed", hooksHash: NO_FILE, reason: "deleted" } : { ...none, hooksHash: NO_FILE };
  }
  if ("error" in read) return { ...none, status: trusted === undefined ? "untrusted" : "changed", reason: "unreadable" };
  const status: ProjectHooksStatus = trusted === undefined ? "untrusted" : read.hash !== undefined && trusted === read.hash ? "trusted" : "changed";
  const why = read.problem ?? (read.hooks === undefined ? { reason: "invalidHooks" as const, invalid: read.invalid ?? "" } : undefined);
  const base = { files: read.files, ...(read.hash === undefined ? {} : { hooksHash: read.hash }), ...(why === undefined ? {} : { reason: why.reason, invalid: why.invalid }) };
  if (status === "trusted") return { status, hooks: withBound(dir, read.hooks ?? [], read.pairs ?? []), ignored: [], ...base };
  return { status, hooks: [], ignored: read.hooks ?? [], ...base };
}

/**
 * Checks a project's hooks file and the files it names against what the person saw
 * (`agent.project.trust`'s `expectedHooksHash`): `hooksHashOf` them, or `none` for no file,
 * must be `expected`, else `INVALID_REQUEST {field: expectedHooksHash, reason: changed}`; a file
 * that cannot be read, does not read as hooks, names a missing file or a path out of the
 * project is `INVALID_REQUEST`. Records nothing: the caller does once every check passed.
 * Answers the hash to record.
 */
export function checkTrustedHooks(dir: string, projectId: string, expected: string): string {
  const read = readProjectAll(dir);
  if ("error" in read) throw finError("INVALID_REQUEST", `${HOOKS_FILE} cannot be read`, { projectId, reason: "unreadable" }, `fix or remove ${HOOKS_FILE}, then trust the project again`);
  if (!("absent" in read) && read.problem !== undefined) {
    throw finError("INVALID_REQUEST", `${HOOKS_FILE}: ${read.problem.invalid}`, { projectId, reason: read.problem.reason }, "a hook's own files must be files inside the project; fix it, then trust the project again");
  }
  const hash = "absent" in read ? NO_FILE : (read.hash ?? "");
  if (hash !== expected) {
    throw finError("INVALID_REQUEST", `${HOOKS_FILE} or a file it names changed since it was shown`, { projectId, field: "expectedHooksHash", reason: "changed", hooksHash: hash }, "list the project's hooks again and trust what they show now");
  }
  if (!("absent" in read) && read.hooks === undefined) {
    throw finError("INVALID_REQUEST", `${HOOKS_FILE} is not a hooks file: ${read.invalid ?? ""}`, { projectId, reason: "invalidHooks" }, `a hooks file is {"hooks": [{"id", "event", "run", ...}]}`);
  }
  return hash;
}

/** What makes a configured hook the same hook: where it is declared, as it is configured, as resolved. */
const hookKey = (h: ConfiguredHook) => JSON.stringify([h.layer, h.source, h.dir, h.id, h.event, h.spec, h.program, h.file, h.bound, h.plugin, h.timeoutS, h.unreadable]);

const inert = (event: HookEvent) => (event === "PreRender" || event === "PostRender" ? { inert: "noRenderKind" as const } : {});

/** A configured hook as `agent.hooks.list` shows it. */
export function listedHook(h: ConfiguredHook, applies: boolean): ListedHook {
  const timeoutS = h.spec?.timeoutS ?? h.timeoutS;
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
    ...(h.spec?.file === undefined ? {} : { file: h.spec.file }),
    ...(h.spec?.principals === undefined ? {} : { principals: h.spec.principals }),
    ...(h.spec?.fin === undefined ? {} : { fin: h.spec.fin }),
    ...(timeoutS === undefined ? {} : { timeoutS }),
    ...(h.spec?.onTimeout === undefined ? {} : { onTimeout: h.spec.onTimeout }),
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
    return rows.map((r) => ({ layer: "plugin", source: r.plugin, id: r.handler, event: r.event, plugin: r.plugin, ...(r.timeoutS === undefined ? {} : { timeoutS: r.timeoutS }) }));
  }

  /** The folder of a project (open, else indexed). */
  dirOf(projectId: string): string | undefined {
    return this.#d.dirOf(projectId);
  }

  /**
   * What `hooks(projectId)` depends on, without touching a file (Decision 5's cost contract):
   * the configuration files that apply (managed, user; local for a project; its hooks file only
   * while trusted with a hash other than `none`), the trusted hash and the project's folder.
   */
  sources(projectId: string | undefined): { readonly paths: readonly string[]; readonly trusted?: string; readonly dir?: string } {
    const data = this.#d.serverData;
    const server = [join(data, MANAGED_SETTINGS), settingsPath(data)];
    if (projectId === undefined) return { paths: server };
    const trusted = this.#d.trustOf(projectId)?.hooksHash;
    const dir = trusted === undefined || trusted === NO_FILE ? undefined : this.#d.dirOf(projectId);
    const local = SAFE_ID.test(projectId) ? [join(data, "projects", projectId, LOCAL_HOOKS)] : [];
    return { paths: [...server, ...(dir === undefined ? [] : [join(dir, HOOKS_FILE)]), ...local], ...(trusted === undefined ? {} : { trusted }), ...(dir === undefined ? {} : { dir }) };
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
    return this.resolve(projectId).hooks;
  }

  /**
   * `hooks(projectId)`, and whether a trusted project file is `changed` (deleted, edited, or a
   * script it binds edited): the engine keeps no index while it is (Decision 5).
   */
  resolve(projectId: string | undefined): { readonly hooks: readonly ConfiguredHook[]; readonly projectChanged: boolean } {
    if (projectId === undefined) return { hooks: [...this.#server(), ...this.#plugins()], projectChanged: false };
    const trusted = this.#d.trustOf(projectId)?.hooksHash;
    const project = trusted === undefined || trusted === NO_FILE ? undefined : this.project(projectId);
    return { hooks: [...this.#server(), ...(project?.hooks ?? []), ...this.#local(projectId), ...this.#plugins()], projectChanged: project?.status === "changed" };
  }

  /**
   * Whether `h` (from `hooks(projectId)`) still applies, every layer re-read as a dispatch does:
   * the check right before a hook's process starts (a project hook's file or script changed
   * while it waited for a slot no longer runs).
   */
  applies(projectId: string | undefined, h: ConfiguredHook): boolean {
    // By what it is, not by object: a file changed within `RACY_MS` is parsed again at each read.
    const key = hookKey(h);
    return this.hooks(projectId).some((x) => x === h || hookKey(x) === key);
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
      ...(p.files.length === 0 ? {} : { projectHooksFiles: [...p.files] }),
    };
  }

  /** `start` (a session's start, a spawn, a saved prompt's run) with `warnings(projectId)` added to its warnings. */
  withWarnings<A extends { readonly warnings: readonly AgentWarning[] }, E>(projectId: string, start: Effect.Effect<A, E>): Effect.Effect<A, E> {
    return Effect.map(start, (r) => ({ ...r, warnings: [...r.warnings, ...this.warnings(projectId)] }));
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
