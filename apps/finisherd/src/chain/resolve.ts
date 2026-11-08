// Stage 3 of the chain (spec §4.4, plan 5A Decision 15): references become current facts before
// any check. Pure functions of `(rule, payload, deps)`.
//
// Directories, paths and projects resolve before authorisation (reach needs them). Branch
// references resolve after it (`resolveWrites`): they need the project's engine, which an
// unauthorised caller must not get opened, and only the policy stage reads them.
import { readFile, realpath } from "node:fs/promises";
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
}

const field = (payload: unknown, path: string): unknown =>
  path.split(".").reduce<unknown>((v, k) => (v !== null && typeof v === "object" ? (v as Record<string, unknown>)[k] : undefined), payload);

/** The real path of `p`, or `p` itself when it does not exist (the handler then answers). */
const canonical = (p: string): Effect.Effect<string> => Effect.promise(() => realpath(p).catch(() => p));

/** A path to be created: its parent's real path joined with its name. */
const canonicalNew = (p: string): Effect.Effect<string> =>
  Effect.map(canonical(dirname(p)), (parent) => join(parent, basename(p)));

/** The project id `<dir>/finisher.project.json` names, if it can be read. */
const projectIdIn = (dir: string): Effect.Effect<string | undefined> =>
  Effect.promise(() =>
    readFile(join(dir, "finisher.project.json"), "utf8")
      .then((text) => {
        const id = (JSON.parse(text) as { id?: unknown }).id;
        return typeof id === "string" && id.length > 0 ? id : undefined;
      })
      .catch(() => undefined),
  );

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
    }
    if (want.has("openDir")) {
      const dir = yield* Effect.flatMap(requireAbsolute("dir", String(out.dir)), canonical);
      openedProject = yield* projectIdIn(dir);
      if (openedProject !== undefined) projects.push(openedProject);
      facts = { ...facts, attached: openedProject !== undefined && deps.isOpen(openedProject) };
    }
    if (want.has("createDir")) {
      paths.push(yield* Effect.flatMap(requireAbsolute("dir", String(out.dir)), canonicalNew));
    }
    if (want.has("probePath")) {
      const path = yield* Effect.flatMap(requireAbsolute("path", String(out.path)), canonical);
      paths.push(path);
      out = { ...out, path };
    }
    if (want.has("importDir")) {
      const dir = yield* requireAbsoluteOpt("importDir", out.importDir as string | undefined);
      if (dir !== undefined) {
        const real = yield* canonical(dir);
        paths.push(real);
        out = { ...out, importDir: real };
      }
    }
    return { payload: out, projects, paths, facts, ...(openedProject === undefined ? {} : { openedProject }) };
  });
}

/** The ids of the branches the call writes to (its `writes` fields), through the store. */
export function resolveWrites(rule: RpcRule, payload: unknown, deps: ChainProjects): Effect.Effect<ReadonlyArray<string>, FinError> {
  const projectId = field(payload, "projectId");
  const refs = (rule.writes ?? []).map((f) => field(payload, f)).filter((v): v is string => typeof v === "string");
  if (typeof projectId !== "string" || refs.length === 0) return Effect.succeed([]);
  return Effect.forEach(refs, (ref) => deps.resolveBranch(projectId, ref));
}

/** Whether `path` is `root` or inside it. */
export function inside(path: string, root: string): boolean {
  return path === root || path.startsWith(root.endsWith(sep) ? root : root + sep);
}

/**
 * The folders a principal with roots may use (`agent`, `mcp`, `device`: spec §4.4): its open
 * projects' folders, each with its `workspace/`, canonical. `undefined`: no roots (any path).
 */
export function rootsOf(principal: Principal, deps: ChainProjects): Effect.Effect<ReadonlyArray<string> | undefined> {
  if (principal.projects === "*") return Effect.succeed(undefined);
  const dirs = principal.projects.map((id) => deps.dirOf(id)).filter((d): d is string => d !== undefined);
  return Effect.forEach(dirs, canonical);
}
