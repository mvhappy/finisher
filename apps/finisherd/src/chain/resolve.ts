// Stage 3 of the chain (spec §4.4, plan 5A Decision 15): references become current facts before
// any check. Pure functions of `(rule, payload, deps)`.
//
// Directories, paths and projects resolve before authorisation (reach needs them). Branch
// references resolve after it (`resolveWrites`): they need the project's engine, which an
// unauthorised caller must not get opened, and only the policy stage reads them.
import { constants } from "node:fs";
import { open, realpath } from "node:fs/promises";
import { basename, dirname, join, sep } from "node:path";
import type { FinError, Principal, RpcRule, RuleFacts } from "@finisher/contracts";
import { Effect } from "effect";
import { requireAbsolute, requireAbsoluteOpt } from "../paths.ts";

/** What the chain needs from the project registry. */
export interface ChainProjects {
  /** The project is open on this server now. */
  isOpen(projectId: string): boolean;
  /** The folder of an open project. */
  dirOf(projectId: string): string | undefined;
  /** A branch reference (a name, an id or `name#id`) as its id, through the store. */
  resolveBranch(projectId: string, ref: string): Effect.Effect<string, FinError>;
  /** Every branch with its author (`createdBy`), archived ones too. */
  branches(projectId: string): Effect.Effect<ReadonlyArray<{ readonly id: string; readonly createdBy: string }>, FinError>;
}

export interface Resolved {
  /** The payload the handler receives (canonical paths in place of the given ones). */
  readonly payload: unknown;
  /** Every project the call touches: reach is checked on each. */
  readonly projects: ReadonlyArray<string>;
  /** Every path it reads or creates, canonical: reach is checked on each. */
  readonly paths: ReadonlyArray<string>;
  readonly facts: RuleFacts;
  /** `project.open`: the project its folder names, when the folder says. */
  readonly openedProject?: string;
  /** `asset.probe`: the project whose folder every path must be in (for principals with roots). */
  readonly pathsIn?: string;
}

const field = (payload: unknown, path: string): unknown =>
  path.split(".").reduce<unknown>((v, k) => (v !== null && typeof v === "object" ? (v as Record<string, unknown>)[k] : undefined), payload);

/**
 * The real path of `p` (absolute and normalised). A path that does not exist (yet) is its
 * nearest existing ancestor's real path joined with the rest, so a symlinked folder above a
 * missing file is still followed (review of Task 5, M2); the rest has no symlink, as it does
 * not exist. A path no ancestor of which resolves stays as given.
 */
export async function canonicalPath(p: string): Promise<string> {
  const rest: string[] = [];
  let at = p;
  for (;;) {
    try {
      const real = await realpath(at);
      return rest.length === 0 ? real : join(real, ...rest.reverse());
    } catch {
      const up = dirname(at);
      if (up === at) return p;
      rest.push(basename(at));
      at = up;
    }
  }
}

const canonical = (p: string): Effect.Effect<string> => Effect.promise(() => canonicalPath(p));

/** The most of `finisher.project.json` read before authorisation. */
export const PROJECT_FILE_MAX = 64 * 1024;
/** Not following a symlink, not blocking on a FIFO (Windows has neither flag: 0). */
const NO_FOLLOW = (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0);

/**
 * The project id `<dir>/finisher.project.json` names, if it can be read: a regular file (not a
 * symlink, FIFO or device), at most 64 KiB, read before any check, so it must not block or
 * flood (review of Task 5, M3).
 */
export async function projectIdIn(dir: string): Promise<string | undefined> {
  let handle;
  try {
    handle = await open(join(dir, "finisher.project.json"), constants.O_RDONLY | NO_FOLLOW);
  } catch {
    return undefined;
  }
  try {
    const st = await handle.stat();
    if (!st.isFile() || st.size > PROJECT_FILE_MAX) return undefined;
    const buf = Buffer.alloc(PROJECT_FILE_MAX + 1);
    const { bytesRead } = await handle.read(buf, 0, buf.length, 0);
    if (bytesRead > PROJECT_FILE_MAX) return undefined;
    const id = (JSON.parse(buf.toString("utf8", 0, bytesRead)) as { id?: unknown } | null)?.id;
    return typeof id === "string" && id.length > 0 ? id : undefined;
  } catch {
    return undefined;
  } finally {
    await handle.close().catch(() => undefined);
  }
}

export function resolve(rule: RpcRule, payload: unknown, deps: ChainProjects): Effect.Effect<Resolved, FinError> {
  return Effect.gen(function* () {
    const want = new Set(rule.resolve ?? []);
    const projects: string[] = [];
    const paths: string[] = [];
    let out = payload as Record<string, unknown>;
    let facts: RuleFacts = {};
    let openedProject: string | undefined;
    if (want.has("project")) {
      for (const k of ["projectId", "pinned"]) {
        const v = field(payload, k);
        if (typeof v === "string") projects.push(v);
      }
      // Serving a project that is not open reopens it (Decision 15): `project.write`.
      if (projects.some((id) => !deps.isOpen(id))) facts = { ...facts, reopens: true };
    }
    // The handler gets the canonical folder, so it opens what was checked (review H2).
    if (want.has("openDir")) {
      const dir = yield* Effect.flatMap(requireAbsolute("dir", String(out.dir)), canonical);
      out = { ...out, dir };
      openedProject = yield* Effect.promise(() => projectIdIn(dir));
      if (openedProject !== undefined) projects.push(openedProject);
      facts = { ...facts, attached: openedProject !== undefined && deps.isOpen(openedProject) };
    }
    if (want.has("createDir")) {
      const dir = yield* Effect.flatMap(requireAbsolute("dir", String(out.dir)), canonical);
      paths.push(dir);
      out = { ...out, dir };
    }
    let pathsIn: string | undefined;
    if (want.has("probePath")) {
      const path = yield* Effect.flatMap(requireAbsolute("path", String(out.path)), canonical);
      paths.push(path);
      out = { ...out, path };
      // Protocol 2: the probe is for this project, so only its folder counts as a root.
      if (typeof out.projectId === "string") pathsIn = out.projectId;
    }
    if (want.has("importDir")) {
      const dir = yield* requireAbsoluteOpt("importDir", out.importDir as string | undefined);
      if (dir !== undefined) {
        const real = yield* canonical(dir);
        paths.push(real);
        out = { ...out, importDir: real };
      }
    }
    return {
      payload: out,
      projects,
      paths,
      facts,
      ...(openedProject === undefined ? {} : { openedProject }),
      ...(pathsIn === undefined ? {} : { pathsIn }),
    };
  });
}

/** The ids of the branches the call writes to or acts on (its `writes` and `targets` fields), through the store. */
export function resolveWrites(rule: RpcRule, payload: unknown, deps: ChainProjects): Effect.Effect<ReadonlyArray<string>, FinError> {
  const projectId = field(payload, "projectId");
  const refs = [...(rule.writes ?? []), ...(rule.targets ?? [])].map((f) => field(payload, f)).filter((v): v is string => typeof v === "string");
  if (typeof projectId !== "string" || refs.length === 0) return Effect.succeed([]);
  return Effect.forEach(refs, (ref) => deps.resolveBranch(projectId, ref));
}

/** Whether `path` is `root` or inside it. */
export function inside(path: string, root: string): boolean {
  return path === root || path.startsWith(root.endsWith(sep) ? root : root + sep);
}

/**
 * The folders a principal with roots may use (`agent`, `mcp`, `device`: spec §4.4): its open
 * projects' folders, each with its `workspace/`, canonical; `only`: just that project's (a probe
 * for it). `undefined`: no roots (any path).
 */
export function rootsOf(principal: Principal, deps: ChainProjects, only?: string): Effect.Effect<ReadonlyArray<string> | undefined> {
  if (principal.projects === "*") return Effect.succeed(undefined);
  const ids = only === undefined ? principal.projects : principal.projects.filter((id) => id === only);
  const dirs = ids.map((id) => deps.dirOf(id)).filter((d): d is string => d !== undefined);
  return Effect.forEach(dirs, canonical);
}
