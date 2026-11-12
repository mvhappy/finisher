// Process keys of the `plugin` family (sp8 spec §3.3; sp8b plan Decisions 3, 4 as amended
// 2026-11-09): a long-lived plugin process serves one (plugin@version, project, authority key)
// and nothing else, and its private temp directory is named by the key's hash. The version is
// in the key, so an update never reuses an old process; the authority key (`authority.ts`:
// class + rights hash + policy generation) keeps callers with different rights apart (one
// agent session tree from another, a child session from its root) and retires a key when its
// class's generation moves.
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { JobOwner, JobView } from "@finisher/contracts";
import type { ControlStore } from "../../control/db.ts";
import { finError } from "../../errors.ts";
import { projectOfJob } from "../../jobs/kinds.ts";
import { type AuthorityGenerations, jobAuthority, memoryGenerations } from "./authority.ts";

/** Where the keys' temp directories live, under server data (swept at every start). */
export const PLUGIN_TMP = ["plugins", "tmp"] as const;

/** The principals whose work shares the `person` class (Decision 4). */
const PERSON_KINDS: ReadonlySet<string> = new Set(["console", "local", "device"]);

/** An agent credential's root session (7A-2's session tree), if the server knows it. */
export type RootSessionOf = (credentialId: string) => string | undefined;

/**
 * The principal class of a job's owner (Decision 4): `person` (the `console`, `local` and
 * `device` principals), `agent:<root session id>` (an agent credential is its session; its
 * class is the root of its session tree, else itself), `system` (work the host starts). Any
 * other owner (an MCP credential) is a class of its own, `<kind>:<id>`, so it never shares a
 * process. A job's rights carry the same class (checked at submit, `pluginFamily.checkRights`).
 */
export function principalClass(owner: JobOwner, rootSessionOf: RootSessionOf = () => undefined): string {
  if (PERSON_KINDS.has(owner.kind)) return "person";
  if (owner.kind === "system") return "system";
  if (owner.kind === "agent") return `agent:${rootSessionOf(owner.id) ?? owner.id}`;
  return `${owner.kind}:${owner.id}`;
}

/** The job's `via.plugin` (`<id>@<version>`), or `PLUGIN_NOT_FOUND`: a plugin job always names one. */
export function viaPlugin(job: JobView): string {
  const p = job.via?.plugin;
  if (p === undefined || p === "") throw finError("PLUGIN_NOT_FOUND", `job ${job.id} names no plugin`, { reason: "noVia" });
  return p;
}

/**
 * `<plugin>@<version>|<project or none>|<authority key>` (spec §3.3's process key). The class is
 * always derived from the job's owner and the rights hash recomputed from its rights, never
 * read from stored values; the generation is the class's now (`authority.ts`). A job without
 * valid rights (the submit check refuses those) gets a key of its own, so it shares no process.
 */
export function pluginWarmKey(job: JobView, rootSessionOf?: RootSessionOf, gens: AuthorityGenerations = memoryGenerations()): string {
  const authority = jobAuthority(job, principalClass(job.owner, rootSessionOf), gens);
  return `${viaPlugin(job)}|${projectOfJob(job) ?? "none"}|${authority?.key ?? `job-${job.id}`}`;
}

/** The key's hash: the name of its temp directory (hex, 32 characters). */
/**
 * A warm key without the confined pool's `<project>/` prefix (`warm.ts` adds it under a
 * sandbox): the authority's key, which names the temp directory and every call's check.
 */
export function bareKey(key: string): string {
  return key.startsWith("@") ? key : key.slice(key.indexOf("/") + 1);
}

export function keyHash(key: string): string {
  return createHash("sha256").update(key).digest("hex").slice(0, 32);
}

/** `<serverData>/plugins/tmp/<keyHash>/`. */
export function keyTmpDir(serverData: string, key: string): string {
  return join(serverData, ...PLUGIN_TMP, keyHash(key));
}

/** Makes `dir` (and `plugins/tmp/`) private to the server's user (0700). */
export function ensurePrivateDir(dir: string): void {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  if (process.platform !== "win32") chmodSync(dir, 0o700);
}

/** Empties `<serverData>/plugins/tmp/` at start: nothing a plugin left there reaches a new process. */
export function sweepPluginTmp(serverData: string): void {
  const root = join(serverData, ...PLUGIN_TMP);
  rmSync(root, { recursive: true, force: true });
  ensurePrivateDir(root);
}

/**
 * An agent credential's root session from `control.db` (`sessions.rootSession`; the
 * credential id is its session's id). `undefined` where the agent tables are absent.
 */
export function sessionRoots(store: ControlStore): RootSessionOf {
  let stmt: ReturnType<ControlStore["db"]["prepare"]> | undefined;
  return (id) => {
    try {
      stmt ??= store.db.prepare("SELECT rootSession FROM sessions WHERE id = ?");
      const r = stmt.get(id) as { rootSession?: string | null } | undefined;
      return r?.rootSession ?? undefined;
    } catch {
      return undefined;
    }
  };
}
