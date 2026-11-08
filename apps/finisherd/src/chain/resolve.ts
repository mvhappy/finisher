// Stage 3 of the chain (spec §4.4, plan 5A Decision 15): references become current facts before
// any check. Pure functions of `(rule, payload, deps)`.
//
// Directories, paths and projects resolve before authorisation (reach needs them). Branch
// references resolve after it (`resolveWrites`): they need the project's engine, which an
// unauthorised caller must not get opened, and only the policy stage reads them.
import { constants } from "node:fs";
import { open, realpath } from "node:fs/promises";
import { basename, dirname, join, sep } from "node:path";
import type { FinError, Principal, RpcRule, RuleFacts, Scope } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { requireAbsolute, requireAbsoluteOpt } from "../paths.ts";

/** What the chain needs from the project registry. */
export interface ChainProjects {
  /** The project is open on this server now. */
  isOpen(projectId: string): boolean;
  /** The folder of an open project. */
  dirOf(projectId: string): string | undefined;
  /**
   * The folder a project that is not open would be reopened from (the registry's index), so
   * that reach can be checked before the handler opens it (review of Task 6, residual 10).
   */
  indexedDirOf?(projectId: string): string | undefined;
  /** A branch reference (a name, an id or `name#id`) as its id, through the store. */
  resolveBranch(projectId: string, ref: string): Effect.Effect<string, FinError>;
  /** Every branch with its author (`createdBy`), archived ones too. */
  branches(projectId: string): Effect.Effect<ReadonlyArray<{ readonly id: string; readonly createdBy: string }>, FinError>;
}

/** What the chain needs from the job engine (5C-2 Decision 4): it reads, never changes. */
export interface ChainJobs {
  /** Whose job `id` is to `principal`: `none` when unknown or not visible to it (`jobs/access.ts`). */
  relation(principal: Principal, id: string): "own" | "other" | "none";
  /** The scopes of `kind` when clients may submit it (`KindSpec.rpc`), else `undefined`. */
  kindScopes(kind: string): ReadonlyArray<Scope> | undefined;
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

export function resolve(
  rule: RpcRule,
  payload: unknown,
  deps: ChainProjects,
  job?: { readonly principal: Principal; readonly jobs?: ChainJobs | undefined },
): Effect.Effect<Resolved, FinError> {
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
    // A job's project is checked for reach; nothing is opened for it.
    if (want.has("jobProject")) {
      const v = field(payload, "project");
      if (typeof v === "string") projects.push(v);
    }
    // A rule that reads jobs without the engine's view of them would let every call through
    // as `none` with no kind scopes: a wiring defect, never an open rule (review L1).
    if ((want.has("jobKind") || want.has("job")) && job?.jobs === undefined) {
      return yield* Effect.fail(finError("INTERNAL", "a jobs rule reached the chain without the job engine", { reason: "noJobs" }));
    }
    // A kind clients may not submit has no scopes to add: its handler answers JOB_KIND_UNKNOWN.
    if (want.has("jobKind")) {
      const kind = field(payload, "kind");
      const scopes = typeof kind === "string" ? job?.jobs?.kindScopes(kind) : undefined;
      if (scopes !== undefined) facts = { ...facts, kindScopes: scopes };
    }
    if (want.has("job")) {
      const id = field(payload, "job");
      const rel = typeof id === "string" && job !== undefined && job.jobs !== undefined ? job.jobs.relation(job.principal, id) : "none";
      facts = { ...facts, job: rel };
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
    if (want.has("outDir")) {
      // Files written for `projectId` go inside that project's folder, whatever other projects
      // the principal may reach (as a probe's path does).
      const dir = yield* requireAbsoluteOpt("out", out.out as string | undefined);
      if (dir !== undefined) {
        const real = yield* canonical(dir);
        paths.push(real);
        out = { ...out, out: real };
        if (typeof out.projectId === "string") pathsIn = out.projectId;
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
 * The folders a principal with roots may use (`agent`, `mcp`, `device`: spec §4.4): its
 * projects' folders, each with its `workspace/`, canonical; an open project's from the
 * registry, one that is not open from the index it would be reopened from, and only while that
 * folder still names the project (review of Tasks 8–9, L6: an index entry is stale once the
 * project moved, and `project.create` inside the old folder opens nothing that would notice;
 * the handlers that open it check again, `probe.ts` re-reads the open folder); `only`: just
 * that project's (a probe for it). `undefined`: no roots (any path).
 */
export function rootsOf(principal: Principal, deps: ChainProjects, only?: string): Effect.Effect<ReadonlyArray<string> | undefined> {
  if (principal.projects === "*") return Effect.succeed(undefined);
  const ids = only === undefined ? principal.projects : principal.projects.filter((id) => id === only);
  return Effect.map(
    Effect.forEach(ids, (id) => {
      const open = deps.dirOf(id);
      if (open !== undefined) return Effect.map(canonical(open), (d): string | undefined => d);
      const indexed = deps.indexedDirOf?.(id);
      if (indexed === undefined) return Effect.succeed(undefined);
      return Effect.flatMap(canonical(indexed), (d) => Effect.map(Effect.promise(() => projectIdIn(d)), (named) => (named === id ? d : undefined)));
    }),
    (dirs) => dirs.filter((d): d is string => d !== undefined),
  );
}
