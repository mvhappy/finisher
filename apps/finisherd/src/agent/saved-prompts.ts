// Saved prompts (sub-project 7B Task 10, Decision 15; spec §6.4): data a person starts, never an
// agent. `{name, title, role, mode, template, params, budget?}`; the server expands one into
// `agent.session.start` plus a first prompt.
//
// - **Sources and clashes.** The bundle (`prompts/*.json`), the owner's `agents.savedPrompts`
//   in `<server-data>/settings.json` and a project's `prompts/*.json` beside
//   `finisher.project.json`. A name in two sources resolves owner settings over project over
//   bundle, so a project folder (which an agent could edit) can never shadow what the owner
//   wrote. A bundled prompt that does not load fails the server's start (as a role does); a
//   settings or project prompt that does not load is left out and reported (`invalid`), never
//   silently, and never the whole list.
// - **Templates are data.** `{{name}}` names a declared parameter and nothing else: no
//   expressions, no code. A `{{` that is not exactly a declared name's slot is refused at load.
//   Substitution is one pass over the template, so a value holding `{{x}}` is not expanded.
// - **Parameters** are `int` (optional `min`/`max`), `string` or `enum` (one of `values`); one
//   with a `default` is optional. A value is validated at each run (`AGENT_ARGS`, `details.field`
//   the parameter); an `int` may arrive as digits (the console sends text).
// - **Project trust.** A project's prompt files load only after a person trusted them, the trust
//   of project rules (`trust.ts`) but kept as its own hash: `project_trust.promptsHash`, the
//   combined hash of every `prompts/*.json` (sha256 over `<file>\0<file hash>` lines in name
//   order; `none` without any). Trusting binds what the person saw (`expectedPromptsHash`); a
//   rules-only trust leaves the prompt files untrusted. Any file added, edited or removed changes
//   the combined hash, and all the project's prompts stop (`changed`) until trusted again; an
//   untrusted or changed project's prompts are listed nowhere and cannot run. They are separate
//   from `rulesHash` because remembering a rule rewrites the rules file and refreshes that hash
//   alone, which must never trust prompt files.
import { closeSync, constants, fstatSync, openSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { type ListedSavedPrompt, type ProjectRulesStatus, PROMPT_STRING_MAX, SavedPrompt, type SavedPromptParam, type SavedPromptSource } from "@finisher/contracts";
import { Schema } from "effect";
import { finError } from "../errors.ts";
import { readAgentSettings } from "./settings.ts";
import { MODE_RANK, type Role } from "./roles.ts";
import type { ProjectTrust } from "./rule-store.ts";
import { hashOf, NO_FILE } from "./trust.ts";

export const PROMPTS_DIR = "prompts";
/** The most prompt files read from a project, and the largest one (bytes). */
export const PROMPT_FILES_MAX = 64;
export const PROMPT_FILE_MAX = 64 * 1024;

const NONBLOCK = constants.O_NONBLOCK ?? 0;
const NOFOLLOW = constants.O_NOFOLLOW ?? 0;
const SLOT = /\{\{([a-z][A-Za-z0-9]{0,31})\}\}/g;
const firstLine = (e: unknown) => (e instanceof Error ? e.message : String(e)).split("\n")[0]!.replace(/\s+/g, " ").trim();

function invalid(source: string, field: string, reason: string): never {
  throw finError("INVALID_REQUEST", `saved prompt (${source}): ${field === "" ? "" : `${field}: `}${reason}`, { source, field, reason }, "a saved prompt is {name, title, role, mode, template, params} (agent/saved-prompts.ts)");
}

/** A prompt decoded and checked (role, mode, parameters, template slots); `INVALID_REQUEST` otherwise. */
export function loadPrompt(raw: unknown, source: string, roles: ReadonlyMap<string, Role>): SavedPrompt {
  let p: SavedPrompt;
  try {
    p = Schema.decodeUnknownSync(SavedPrompt)(raw, { onExcessProperty: "error" });
  } catch (e) {
    return invalid(source, "", firstLine(e));
  }
  const role = roles.get(p.role);
  if (role === undefined) invalid(source, "role", `no agent role named ${p.role}`);
  if (MODE_RANK[p.mode] > MODE_RANK[role.modeCeiling]) invalid(source, "mode", `the ${role.name} role works in ${role.modeCeiling} mode at most`);
  const declared = new Set<string>();
  for (const q of p.params) {
    if (declared.has(q.name)) invalid(source, `params.${q.name}`, "is declared twice");
    declared.add(q.name);
    checkParam(q, source);
  }
  const rest = p.template.replace(SLOT, (_, n: string) => {
    if (!declared.has(n)) invalid(source, "template", `{{${n}}} is not a declared parameter`);
    return "";
  });
  if (rest.includes("{{")) invalid(source, "template", "a {{ that is not exactly a declared parameter's {{name}} slot (templates have no expressions)");
  return p;
}

function checkParam(q: SavedPromptParam, source: string): void {
  const at = `params.${q.name}`;
  if (q.type === "enum") {
    if (q.values === undefined || q.values.length === 0) invalid(source, at, "an enum needs values");
    if (new Set(q.values).size !== q.values.length) invalid(source, at, "values repeat");
    if (q.min !== undefined || q.max !== undefined) invalid(source, at, "only an int has min and max");
  } else if (q.values !== undefined) invalid(source, at, "only an enum has values");
  if (q.type !== "int" && (q.min !== undefined || q.max !== undefined)) invalid(source, at, "only an int has min and max");
  if (q.min !== undefined && q.max !== undefined && q.min > q.max) invalid(source, at, "min is above max");
  if (q.default !== undefined) {
    try {
      coerce(q, q.default);
    } catch {
      invalid(source, at, "its default does not satisfy the parameter");
    }
  }
}

/** `value` as the parameter's typed string, or throws a reason (`required`, `notInt`, `outOfRange`, ...). */
function coerce(q: SavedPromptParam, value: unknown): string {
  if (q.type === "int") {
    const n = typeof value === "number" ? value : typeof value === "string" && /^-?\d{1,15}$/.test(value) ? Number(value) : Number.NaN;
    if (!Number.isSafeInteger(n)) throw new Error("notInt");
    if ((q.min !== undefined && n < q.min) || (q.max !== undefined && n > q.max)) throw new Error("outOfRange");
    return String(n);
  }
  const s = typeof value === "string" ? value : typeof value === "number" ? String(value) : undefined;
  if (s === undefined) throw new Error("notString");
  if (q.type === "enum") {
    if (!(q.values ?? []).includes(s)) throw new Error("notInValues");
    return s;
  }
  if (s.length > PROMPT_STRING_MAX) throw new Error("tooLong");
  return s;
}

/** The first prompt for a run: the template with each slot's typed value; `AGENT_ARGS` naming the field. */
export function expand(p: SavedPrompt, given: Readonly<Record<string, unknown>>): string {
  const args = (field: string, reason: string, extra: Record<string, unknown> = {}) =>
    finError("AGENT_ARGS", `saved prompt ${p.name}: parameter ${field}: ${reason}`, { prompt: p.name, field, reason, ...extra }, `give ${field} as the prompt's parameter list says (finisherd agent prompts)`);
  const known = new Map(p.params.map((q) => [q.name, q]));
  for (const k of Object.keys(given)) if (!known.has(k)) throw args(k, "unknown", { params: [...known.keys()] });
  const values = new Map<string, string>();
  for (const q of p.params) {
    const v = given[q.name] ?? q.default;
    if (v === undefined) throw args(q.name, "required");
    try {
      values.set(q.name, coerce(q, v));
    } catch (e) {
      throw args(q.name, (e as Error).message, { ...(q.min === undefined ? {} : { min: q.min }), ...(q.max === undefined ? {} : { max: q.max }), ...(q.values === undefined ? {} : { values: q.values }) });
    }
  }
  return p.template.replace(SLOT, (_, n: string) => values.get(n)!);
}

// Files ---------------------------------------------------------------------------------------

/** `dir`'s `*.json`, each read once (regular file, not a link, size-capped); the one buffer is what is hashed and parsed. */
type FileRead = { readonly file: string; readonly hash: string; readonly json?: unknown; readonly invalid?: string };
function readJsonDir(dir: string): { readonly files: readonly FileRead[]; readonly error?: string } | "absent" {
  let names: string[];
  try {
    names = readdirSync(dir).filter((n) => n.endsWith(".json")).sort();
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    return code === "ENOENT" || code === "ENOTDIR" ? "absent" : { files: [], error: String(code ?? e) };
  }
  const files: FileRead[] = [];
  for (const file of names.slice(0, PROMPT_FILES_MAX)) {
    let fd: number | undefined;
    try {
      fd = openSync(join(dir, file), constants.O_RDONLY | NONBLOCK | NOFOLLOW);
      const st = fstatSync(fd);
      if (!st.isFile() || st.size > PROMPT_FILE_MAX) throw new Error(st.isFile() ? `larger than ${PROMPT_FILE_MAX} bytes` : "not a regular file");
      const bytes = readFileSync(fd);
      if (bytes.length > PROMPT_FILE_MAX) throw new Error(`larger than ${PROMPT_FILE_MAX} bytes`);
      try {
        files.push({ file, hash: hashOf(bytes), json: JSON.parse(bytes.toString("utf8")) as unknown });
      } catch (e) {
        files.push({ file, hash: hashOf(bytes), invalid: `not JSON: ${firstLine(e)}` });
      }
    } catch (e) {
      files.push({ file, hash: "unreadable", invalid: firstLine(e) });
    } finally {
      if (fd !== undefined) closeSync(fd);
    }
  }
  const tooMany = names.length > PROMPT_FILES_MAX ? { error: `more than ${PROMPT_FILES_MAX} prompt files` } : {};
  return { files, ...tooMany };
}

const manifestHash = (files: readonly FileRead[]) => (files.length === 0 ? NO_FILE : hashOf(Buffer.from(files.map((f) => `${f.file}\0${f.hash}`).join("\n"))));

export const BUNDLED_DIR = new URL("./prompts/", import.meta.url);

/** The bundled prompts, by name; a bad one fails the start (a build error, as a role file). */
export function loadBundled(roles: ReadonlyMap<string, Role>, dir: URL = BUNDLED_DIR): ReadonlyMap<string, SavedPrompt> {
  const out = new Map<string, SavedPrompt>();
  const read = readJsonDir(fileURLToPath(dir));
  for (const f of read === "absent" ? [] : read.files) {
    if (f.json === undefined) invalid(`bundle ${f.file}`, "", f.invalid ?? "unreadable");
    const p = loadPrompt(f.json, `bundle ${f.file}`, roles);
    if (f.file !== `${p.name}.json`) invalid(`bundle ${f.file}`, "name", `must match the file name (${p.name}.json)`);
    out.set(p.name, p);
  }
  return out;
}

// The layered view -----------------------------------------------------------------------------

export interface PromptListing {
  readonly prompts: readonly ListedSavedPrompt[];
  /** A project's valid prompts that do not apply (untrusted, changed). */
  readonly ignored: readonly ListedSavedPrompt[];
  readonly invalid: ReadonlyArray<{ readonly source: string; readonly name?: string; readonly reason: string }>;
  readonly projectPrompts?: ProjectRulesStatus;
  readonly promptsHash?: string;
}

export interface SavedPromptsDeps {
  readonly serverData: string;
  readonly roles: ReadonlyMap<string, Role>;
  /** The folder of a project (open, else indexed). */
  readonly dirOf: (projectId: string) => string | undefined;
  readonly trustOf: (projectId: string) => ProjectTrust | undefined;
}

/** A project's prompt files against its trust record (`project`: what loads, only once trusted). */
export interface ProjectPrompts {
  readonly status: ProjectRulesStatus;
  /** The combined hash now (`none` without files); absent when the folder cannot be read. */
  readonly promptsHash?: string;
  readonly files: readonly FileRead[];
}

export function projectPrompts(dir: string | undefined, trust: ProjectTrust | undefined): ProjectPrompts {
  const gone = (): ProjectPrompts => ({ status: trust?.promptsHash !== undefined && trust.promptsHash !== NO_FILE ? "changed" : "none", promptsHash: NO_FILE, files: [] });
  if (dir === undefined) return { status: "none", files: [] };
  const read = readJsonDir(join(dir, PROMPTS_DIR));
  if (read === "absent" || (read.files.length === 0 && read.error === undefined)) return gone();
  // A folder that cannot be listed whole (unreadable, or past the file cap) trusts and loads nothing.
  if (read.error !== undefined) return { status: trust?.promptsHash === undefined ? "untrusted" : "changed", files: [] };
  const hash = manifestHash(read.files);
  const status: ProjectRulesStatus = trust?.promptsHash === undefined ? "untrusted" : trust.promptsHash === hash ? "trusted" : "changed";
  return { status, promptsHash: hash, files: read.files };
}

export class SavedPrompts {
  readonly #d: SavedPromptsDeps;
  readonly #bundled: ReadonlyMap<string, SavedPrompt>;

  constructor(d: SavedPromptsDeps) {
    this.#d = d;
    this.#bundled = loadBundled(d.roles);
  }

  /** The prompts `projectId` can run, owner settings over a trusted project's over the bundle. */
  list(projectId?: string): PromptListing {
    const bad: Array<{ source: string; name?: string; reason: string }> = [];
    const ignored: ListedSavedPrompt[] = [];
    const by = new Map<string, ListedSavedPrompt>();
    const add = (p: SavedPrompt, source: SavedPromptSource) => void (!by.has(p.name) && by.set(p.name, { ...p, source }));
    const { settings, error } = readAgentSettings(this.#d.serverData);
    if (error?.includes("savedPrompts") === true) bad.push({ source: "settings.json", reason: error });
    (settings.savedPrompts ?? []).forEach((raw, i) => {
      const name = typeof raw === "object" && raw !== null && typeof (raw as { name?: unknown }).name === "string" ? (raw as { name: string }).name.slice(0, 64) : undefined;
      try {
        add(loadPrompt(raw, `settings.json agents.savedPrompts[${i}]`, this.#d.roles), "settings");
      } catch (e) {
        bad.push({ source: `settings.json agents.savedPrompts[${i}]`, ...(name === undefined ? {} : { name }), reason: firstLine((e as { details?: { reason?: string } }).details?.reason ?? e) });
      }
    });
    const project = projectId === undefined ? undefined : projectPrompts(this.#d.dirOf(projectId), this.#d.trustOf(projectId));
    for (const f of project?.files ?? []) {
      const source = `${PROMPTS_DIR}/${f.file}`;
      try {
        if (f.json === undefined) invalid(source, "", f.invalid ?? "unreadable");
        const p = loadPrompt(f.json, source, this.#d.roles);
        if (f.file !== `${p.name}.json`) invalid(source, "name", `must match the file name (${p.name}.json)`);
        if (project?.status === "trusted") add(p, "project");
        else ignored.push({ ...p, source: "project" });
      } catch (e) {
        bad.push({ source, reason: firstLine((e as { details?: { reason?: string } }).details?.reason ?? e) });
      }
    }
    for (const p of this.#bundled.values()) add(p, "bundle");
    return {
      prompts: [...by.values()].sort((a, b) => a.name.localeCompare(b.name)),
      ignored,
      invalid: bad,
      ...(project === undefined ? {} : { projectPrompts: project.status, ...(project.promptsHash === undefined ? {} : { promptsHash: project.promptsHash }) }),
    };
  }

  /** The prompt `name` for `projectId`, or `SAVED_PROMPT_UNKNOWN`. */
  find(projectId: string, name: string): ListedSavedPrompt {
    const { prompts } = this.list(projectId);
    const p = prompts.find((x) => x.name === name);
    if (p === undefined) {
      throw finError("SAVED_PROMPT_UNKNOWN", `no saved prompt named ${name}`, { name, prompts: prompts.map((x) => x.name) }, "list them with `finisherd agent prompts`; a project's own load once a person trusts the project");
    }
    return p;
  }
}

/**
 * What `agent.project.trust` checks before it changes anything when it carries
 * `expectedPromptsHash`: the folder's combined hash must be what the person saw, and every file
 * must load. Answers the hash to record.
 */
export function checkTrustedPrompts(dir: string, projectId: string, expected: string, roles: ReadonlyMap<string, Role>): string {
  const now = projectPrompts(dir, undefined);
  if (now.promptsHash === undefined) {
    throw finError("INVALID_REQUEST", `${PROMPTS_DIR}/ cannot be read`, { projectId, reason: "unreadable" }, `fix or remove ${PROMPTS_DIR}/, then trust the project again`);
  }
  if (now.promptsHash !== expected) {
    throw finError("INVALID_REQUEST", `${PROMPTS_DIR}/ changed since it was shown`, { projectId, field: "expectedPromptsHash", reason: "changed", promptsHash: now.promptsHash }, "list the project's saved prompts again and trust what they show now");
  }
  for (const f of now.files) {
    if (f.json === undefined) throw finError("INVALID_REQUEST", `${PROMPTS_DIR}/${f.file}: ${f.invalid ?? "unreadable"}`, { projectId, reason: "invalidPrompt", file: f.file }, "fix or remove the file, then trust the project again");
    loadPrompt(f.json, `${PROMPTS_DIR}/${f.file}`, roles);
  }
  return now.promptsHash;
}
