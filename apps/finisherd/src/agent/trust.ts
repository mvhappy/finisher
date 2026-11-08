// Rule files and project trust (sub-project 7B Decisions 4 and 6; spec §7.2, §9.4).
//
// - **Files.** Three layers are files: `managed` (`<server-data>/managed-settings.json`,
//   `agents.rules`; read-only to the server), `user` (`<server-data>/settings.json`,
//   `agents.rules`) and `project` (`finisher.rules.json` beside `finisher.project.json`,
//   `{"rules": […]}`). A rule in a file is `{id, effect, command, where?}`, as in a role file;
//   the reader fills `layer`, `source` (the file's name) and `createdAt` 0. Files are checked at
//   every decision, so an edit applies at once: each is opened and `fstat`ed, and its parsed
//   rules are kept by `(path, ino, size, mtime, ctime)`; any change re-reads it, hashing and
//   parsing the one buffer read (what is hashed is what applies). A file changed within
//   `RACY_MS` of being read is not kept (a second change in the same timestamp tick could keep
//   its key). A rule that does not decode is named in the error (`rule "<id>": …`).
// - **A layer that applies but cannot be read denies** every agent call (a synthetic rule
//   `<layer>.unreadable`, `deny **`): a broken file never silently drops the rules it held.
//   No file, or no `agents.rules` key, is no rules.
// - **Project trust.** The project file lives in the project folder and travels with it (an
//   agent could edit it), so it applies only once a person trusts it (`finisherd project trust`,
//   `agent.project.trust`), which records the file's hash (`sha256:<hex>` of its bytes, `none`
//   for no file) in `project_trust`. A file whose hash differs from the trusted one is ignored
//   (`changed`) until it is trusted again, and so is a trusted file since deleted (`deleted`) or
//   unreadable (`unreadable`); an untrusted project's rules do not apply at all, neither its
//   `deny` nor its `allow` (owner decision 2026-11-08). Trusting binds what the person saw: the
//   console lists the file with its hash and sends it back (`expectedHash`); a file changed in
//   between is refused (`INVALID_REQUEST {field: expectedHash, reason: changed}`). Trusting a
//   file that does not read as rules is refused, so a trusted file reads (unless a later
//   server's schema refuses one of its rules: it then denies, as an unreadable layer).
import { createHash } from "node:crypto";
import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { type Rule, RuleCommand, RuleEffect, type RuleLayer, RuleWhere } from "@finisher/contracts";
import { Schema } from "effect";
import { settingsPath } from "../config.ts";
import { finError } from "../errors.ts";
import type { ProjectTrust, RuleStore } from "./rule-store.ts";

export const RULES_FILE = "finisher.rules.json";
export const MANAGED_SETTINGS = "managed-settings.json";
/** The hash trust records for a project with no rules file. */
export const NO_FILE = "none";
/** The largest rules file read; a larger one is unreadable. */
export const RULES_FILE_MAX = 256 * 1024;
/** A file changed this recently (ms) is read again at every decision (see the module docs). */
export const RACY_MS = 2000;
/** Files whose parsed rules are kept at most (past it the cache starts again). */
const CACHE_MAX = 1024;

/** A rule as a file writes it (roles, settings, the project file). */
export const FileRule = Schema.Struct({
  id: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
  effect: RuleEffect,
  command: RuleCommand,
  where: Schema.optionalKey(RuleWhere),
});
const decodeRule = Schema.decodeUnknownSync(FileRule);

/** Whether a project's file rules apply: `none` (no file), `trusted`, `untrusted`, `changed`. */
export type ProjectRulesStatus = "none" | "trusted" | "untrusted" | "changed";
/** Why a project's file does not apply as trusted: gone since it was trusted, unreadable, or not rules. */
export type ProjectRulesReason = "deleted" | "unreadable" | "invalidRules";

export interface ProjectRules {
  readonly status: ProjectRulesStatus;
  /** The rules that apply (none unless `trusted`). */
  readonly rules: readonly Rule[];
  /** The file's rules that do not apply (untrusted, changed), for listing. */
  readonly ignored: readonly Rule[];
  /** The file's hash now (`none`: no file), what a person trusts; absent when it cannot be read. */
  readonly rulesHash?: string;
  readonly reason?: ProjectRulesReason;
}

/** The deny-everything rule standing for a layer that cannot be read. */
export function unreadable(layer: RuleLayer, source: string): Rule {
  return { id: `${layer}.unreadable`, effect: "deny", command: "**", layer, source, createdAt: 0 };
}

const isObject = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const firstLine = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/\s+/g, " ").trim();

/** Decodes a file's rules one by one, so an error names the rule (`rule "<id>": …`). */
function decodeRules(list: unknown, layer: RuleLayer, source: string): Rule[] {
  if (!Array.isArray(list)) throw new Error("rules is not a list");
  return list.map((r, i) => {
    try {
      return { ...decodeRule(r), layer, source, createdAt: 0 };
    } catch (e) {
      const id = isObject(r) && typeof r.id === "string" ? JSON.stringify(r.id.slice(0, 64)) : `#${i}`;
      throw new Error(`rule ${id}: ${firstLine(e)}`);
    }
  });
}

/** A file's state: absent, unreadable, or read (its hash, and its rules or why they do not decode). */
type Read = { readonly absent: true } | { readonly error: string } | { readonly hash: string; readonly rules?: readonly Rule[]; readonly invalid?: string };

const cache = new Map<string, { readonly key: string; readonly read: Read }>();
/** Not following a FIFO into a blocking open (Windows has no such flag: 0). */
const NONBLOCK = constants.O_NONBLOCK ?? 0;
const hashOf = (bytes: Buffer) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

/** `path` read and parsed with `parse` (cached per `slot`, see the module docs). */
function readRules(path: string, slot: string, parse: (json: unknown) => Rule[]): Read {
  let fd: number;
  try {
    fd = openSync(path, constants.O_RDONLY | NONBLOCK);
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    return code === "ENOENT" || code === "ENOTDIR" ? { absent: true } : { error: String(code ?? e) };
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) return { error: "not a regular file" };
    if (st.size > RULES_FILE_MAX) return { error: `larger than ${RULES_FILE_MAX} bytes` };
    const key = `${path}\0${st.ino}\0${st.size}\0${st.mtimeMs}\0${st.ctimeMs}`;
    const hit = cache.get(slot);
    if (hit?.key === key) return hit.read;
    const readAt = Date.now();
    const bytes = readFileSync(fd);
    if (bytes.length > RULES_FILE_MAX) return { error: `larger than ${RULES_FILE_MAX} bytes` };
    const hash = hashOf(bytes);
    let read: Read;
    try {
      read = { hash, rules: parse(JSON.parse(bytes.toString("utf8")) as unknown) };
    } catch (e) {
      read = { hash, invalid: firstLine(e) };
    }
    if (Math.max(st.mtimeMs, st.ctimeMs) < readAt - RACY_MS) {
      if (cache.size >= CACHE_MAX) cache.clear();
      cache.set(slot, { key, read });
    } else {
      cache.delete(slot);
    }
    return read;
  } catch (e) {
    return { error: String((e as { code?: unknown }).code ?? e) };
  } finally {
    closeSync(fd);
  }
}

/** `agents.rules` of a settings file (`managed` or `user`): none without the file or the key. */
export function settingsRules(path: string, layer: "managed" | "user", source: string): readonly Rule[] {
  const read = readRules(path, `${layer}\0${path}`, (j) => {
    if (!isObject(j)) throw new Error("not an object");
    const agents = j.agents;
    if (agents === undefined) return [];
    if (!isObject(agents)) throw new Error("agents is not an object");
    return agents.rules === undefined ? [] : decodeRules(agents.rules, layer, source);
  });
  if ("absent" in read) return [];
  return "rules" in read && read.rules !== undefined ? read.rules : [unreadable(layer, source)];
}

/** The managed layer: `<server-data>/managed-settings.json`. */
export const managedRules = (serverData: string) => settingsRules(join(serverData, MANAGED_SETTINGS), "managed", MANAGED_SETTINGS);
/** The user layer: `agents.rules` in `<server-data>/settings.json`. */
export const userRules = (serverData: string) => settingsRules(settingsPath(serverData), "user", "settings.json");

function readProject(dir: string): Read {
  const path = join(dir, RULES_FILE);
  return readRules(path, `project\0${path}`, (j) => {
    if (!isObject(j) || !Array.isArray(j.rules)) throw new Error('a rules file is {"rules": [...]}');
    return decodeRules(j.rules, "project", RULES_FILE);
  });
}

/** A project's file rules and whether they apply, against its trust record. */
export function projectRules(dir: string | undefined, trust: ProjectTrust | undefined): ProjectRules {
  if (dir === undefined) return { status: "none", rules: [], ignored: [] };
  const read = readProject(dir);
  if ("absent" in read) {
    // A trusted file since deleted is a change (its rules no longer apply), not "no file".
    const gone = trust !== undefined && trust.rulesHash !== NO_FILE;
    return gone ? { status: "changed", rules: [], ignored: [], rulesHash: NO_FILE, reason: "deleted" } : { status: "none", rules: [], ignored: [], rulesHash: NO_FILE };
  }
  if ("error" in read) return { status: trust === undefined ? "untrusted" : "changed", rules: [], ignored: [], reason: "unreadable" };
  const status: ProjectRulesStatus = trust === undefined ? "untrusted" : trust.rulesHash === read.hash ? "trusted" : "changed";
  const reason = read.rules === undefined ? { reason: "invalidRules" as const } : {};
  if (status === "trusted") return { status, rules: read.rules ?? [unreadable("project", RULES_FILE)], ignored: [], rulesHash: read.hash, ...reason };
  return { status, rules: [], ignored: read.rules ?? [], rulesHash: read.hash, ...reason };
}

/**
 * Trusts a project's rules file as the person saw it (a person's, `agent.project.trust`): its
 * hash, or `none` for no file, must be `expectedHash`, else `INVALID_REQUEST {field:
 * expectedHash, reason: changed}`. A file that does not read as rules is `INVALID_REQUEST`.
 * Nothing changes on a refusal.
 */
export function trustProject(store: RuleStore, projectId: string, dir: string, expectedHash: string, by: string, at: number): { trust: ProjectTrust; rules: number } {
  const read = readProject(dir);
  if ("error" in read) throw finError("INVALID_REQUEST", `${RULES_FILE} cannot be read`, { projectId, reason: "unreadable" }, `fix or remove ${RULES_FILE}, then trust the project again`);
  const hash = "absent" in read ? NO_FILE : read.hash;
  if (hash !== expectedHash) {
    throw finError("INVALID_REQUEST", `${RULES_FILE} changed since it was shown`, { projectId, field: "expectedHash", reason: "changed", rulesHash: hash }, "list the project's rules again and trust what they show now");
  }
  if ("absent" in read) return { trust: store.trust(projectId, NO_FILE, by, at), rules: 0 };
  if (read.rules === undefined) {
    throw finError("INVALID_REQUEST", `${RULES_FILE} is not a rules file: ${read.invalid ?? ""}`, { projectId, reason: "invalidRules" }, `a rules file is {"rules": [{"id", "effect", "command", "where"?}...]}`);
  }
  return { trust: store.trust(projectId, hash, by, at), rules: read.rules.length };
}
