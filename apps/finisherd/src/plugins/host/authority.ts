// Authority keys (sp8 spec §3.3, §5.1, §12, Decision 28; sp8b plan Decisions 4 and 5 as
// amended 2026-11-09): a long-lived plugin process, its private temp and its `pluginData` are
// keyed by the caller's effective authority, not its principal class alone. The authority key
// is `<class>.<rightsHash>.<generation>` (the class with `:` spelled `-`, so the key is one
// file name on every OS):
// - the **class** comes from the job's owner (`principalClass`), never from stored rights;
// - the **rights hash** is recomputed from the job's rights (`rightsHash`: scopes, project,
//   branch and library reach, grants; not the principal), so callers of one class with
//   different rights (a child agent session against its root: another root branch) never
//   share a process;
// - the **policy generation** is the class's, persisted in `control.db` (`plugin_authority`,
//   plugins migration 0005). It increments when a credential of the class is minted or ended
//   (an agent session tree that starts, gains a child or ends; an MCP credential revoked or
//   expired; credentials are never narrowed in place today) and when a device is revoked
//   (`person`). A key's generation is the class's generation **now** (a job queued before a
//   bump runs under the new key; its rights are narrowed again on every call, Task 7), so a
//   retired key is never started again. The host retires the old generation's processes,
//   removes their temp and the class's authority-keyed data (`processes.ts`).
//
// `pluginData` (spec §5.1): `plugins/data/@pub/name/<projectId|_>/` is shared by the `person`
// and `system` classes only while their rights are the class's uniform rights (true today:
// console, local and device principals hold every scope a plugin can map to, and so does host
// work; `uniformClassRights` checks it on every use, and a caller whose rights differ gets an
// authority directory instead); every other caller gets
// `plugins/authdata/@pub/name/<projectId|_>/<authorityKey>/`, removed at its generation's end.
// A directory is created on first use, and only when `fs pluginData` is granted.
import { chmodSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { JobRights, JobView } from "@finisher/contracts";
import { isJobRights } from "@finisher/contracts";
import type { Permission } from "@finisher/core-types";
import type { ControlStore } from "../../control/db.ts";
import { finError } from "../../errors.ts";
import { projectOfJob } from "../../jobs/kinds.ts";
import { CONSOLE } from "../../principal.ts";
import { rightsFor, rightsHash, systemInvoker } from "./rights.ts";

export const PLUGIN_DATA = ["plugins", "data"] as const;
export const PLUGIN_AUTHDATA = ["plugins", "authdata"] as const;

/** A class as one file-name-safe word: `agent:<id>` → `agent-<id>`; anything else is hashed. */
export function classSlug(cls: string): string {
  if (/^[a-z]+(:[A-Za-z0-9_]+)?$/.test(cls)) return cls.replace(":", "-");
  return `x${rightsHash({ class: cls, scopes: [], projects: [], libraries: [], grants: [] }).slice(0, 16)}`;
}

/** `<class slug>.<rights hash>.<generation>`. */
export function authorityKey(cls: string, hash: string, generation: number): string {
  return `${classSlug(cls)}.${hash}.${generation}`;
}

/** An authority key's parts, or `undefined` for anything else. */
export function parseAuthorityKey(key: string): { readonly slug: string; readonly hash: string; readonly generation: number } | undefined {
  const m = /^([A-Za-z0-9_-]+)\.([0-9a-f]{32})\.(\d+)$/.exec(key);
  return m === null ? undefined : { slug: m[1] as string, hash: m[2] as string, generation: Number(m[3]) };
}

/** The authority part of a warm key (`<plugin>@<ver>|<project|none>|<authorityKey>`, maybe `<project>/`-prefixed). */
export function authorityOfWarmKey(key: string): ReturnType<typeof parseAuthorityKey> {
  return parseAuthorityKey(key.slice(key.lastIndexOf("|") + 1));
}

/** Each class's policy generation. */
export interface AuthorityGenerations {
  /** The class's generation now (0 until its first bump). */
  of(cls: string): number;
  /** Increments the class's generation; the new one. */
  bump(cls: string): number;
  /** Raises the class's generation to at least `n` (a job seen at start carries `n`); never lowers it. */
  raise(cls: string, n: number): void;
  /** Every class with a generation above 0, by its slug. */
  bySlug(): ReadonlyMap<string, number>;
}

/** Generations in memory only (tests and families built without a store). */
export function memoryGenerations(): AuthorityGenerations {
  const gens = new Map<string, number>();
  return {
    of: (cls) => gens.get(cls) ?? 0,
    bump: (cls) => {
      const n = (gens.get(cls) ?? 0) + 1;
      gens.set(cls, n);
      return n;
    },
    raise: (cls, n) => {
      if (n > (gens.get(cls) ?? 0)) gens.set(cls, n);
    },
    bySlug: () => new Map([...gens].map(([c, n]) => [classSlug(c), n])),
  };
}

/**
 * Generations in `control.db` (`plugin_authority`), cached in memory. A bump the store cannot
 * write (busy) still takes effect in memory; a restart then reads the older value. Processes
 * and temp do not survive a restart, and the start's sweep removes authority directories of
 * any generation but the current; a job queued under the lost generation raises its class
 * back to it when the host binds the engine (`raise`), so no generation is handed out twice.
 */
export function storedGenerations(store: ControlStore): AuthorityGenerations {
  const cache = new Map<string, number>();
  const read = store.db.prepare("SELECT generation FROM plugin_authority WHERE class = ?");
  const write = store.db.prepare(
    "INSERT INTO plugin_authority (class, generation) VALUES (?, ?) ON CONFLICT(class) DO UPDATE SET generation = MAX(generation, excluded.generation)",
  );
  const of = (cls: string): number => {
    const hit = cache.get(cls);
    if (hit !== undefined) return hit;
    const n = Number((read.get(cls) as { generation?: number } | undefined)?.generation ?? 0);
    cache.set(cls, n);
    return n;
  };
  const set = (cls: string, n: number): void => {
    cache.set(cls, n);
    try {
      write.run(cls, n);
    } catch (e) {
      process.stderr.write(`finisherd: warning: cannot record a plugin authority generation (kept in memory): ${String(e)}\n`);
    }
  };
  return {
    of,
    bump: (cls) => {
      const n = of(cls) + 1;
      set(cls, n);
      return n;
    },
    raise: (cls, n) => {
      if (n > of(cls)) set(cls, n);
    },
    bySlug: () => {
      const out = new Map<string, number>();
      for (const r of store.db.prepare("SELECT class, generation FROM plugin_authority").all() as Array<{ class: string; generation: number }>) {
        out.set(classSlug(r.class), Math.max(Number(r.generation), cache.get(r.class) ?? 0));
      }
      for (const [c, n] of cache) if (!out.has(classSlug(c))) out.set(classSlug(c), n);
      return out;
    },
  };
}

/** The authority a job runs under. */
export interface Authority {
  readonly class: string;
  readonly hash: string;
  readonly generation: number;
  readonly key: string;
}

/**
 * A job's authority: `cls` from its owner, the hash recomputed from its rights, the class's
 * generation now; `undefined` when the job carries no valid rights.
 */
export function jobAuthority(job: Pick<JobView, "rights">, cls: string, gens: AuthorityGenerations): Authority | undefined {
  if (!isJobRights(job.rights)) return undefined;
  const hash = rightsHash({ ...job.rights, class: cls });
  const generation = Math.max(job.rights.generation, gens.of(cls));
  return { class: cls, hash, generation, key: authorityKey(cls, hash, generation) };
}

/**
 * Whether `rights` are their class's uniform rights, which is what lets `person` and `system`
 * share one per-project `pluginData` directory (spec §3.3): the rights a console (`person`) or
 * the host (`system`) holds with the same grants. Every `person` principal holds them today
 * (`plugins-authority.test.ts` pins it); should a member ever hold less, its calls get an
 * authority directory, never the shared one.
 */
export function uniformClassRights(rights: JobRights): boolean {
  if (rights.class !== "person" && rights.class !== "system") return false;
  const invoker = rights.class === "person" ? CONSOLE : systemInvoker("host");
  return rightsFor({ invoker, grants: rights.grants, class: rights.class }).rightsHash === rightsHash(rights);
}

/** The `fs pluginData` grant's access, if granted. */
export function pluginDataAccess(grants: ReadonlyArray<Permission>): "read" | "readWrite" | undefined {
  let access: "read" | "readWrite" | undefined;
  for (const g of grants) if (g.perm === "fs" && g.scope === "pluginData") access = access === "readWrite" || g.access === "readWrite" ? "readWrite" : "read";
  return access;
}

const SAFE_PART = /^[A-Za-z0-9_][A-Za-z0-9._-]*$/;

/** `@pub/name@version` → `["@pub", "name"]`, or `@pub/name`. */
function idParts(plugin: string): [string, string] {
  const id = plugin.lastIndexOf("@") > 0 ? plugin.slice(0, plugin.lastIndexOf("@")) : plugin;
  const [pub, name, ...rest] = id.split("/");
  if (pub === undefined || !pub.startsWith("@") || !SAFE_PART.test(pub.slice(1)) || name === undefined || !SAFE_PART.test(name) || rest.length > 0) {
    throw finError("PLUGIN_NOT_FOUND", "a plugin job names no valid plugin id", { reason: "pluginId" });
  }
  return [pub, name];
}

/** `<projectId|_>` as one path part. */
function projectPart(project: string | undefined): string {
  if (project === undefined) return "_";
  if (!SAFE_PART.test(project)) throw finError("INVALID_REQUEST", "a plugin job's project is not a plain id", { field: "project" });
  return project;
}

/**
 * The caller's `pluginData` directory for `plugin` (`<id>` or `<id>@<version>`) in `project`
 * (spec §5.1): shared for uniform `person`/`system` rights, else the authority's.
 */
export function pluginDataPath(serverData: string, plugin: string, project: string | undefined, rights: JobRights, authority: Authority): string {
  const [pub, name] = idParts(plugin);
  const proj = projectPart(project);
  return uniformClassRights({ ...rights, class: authority.class }) ? join(serverData, ...PLUGIN_DATA, pub, name, proj) : join(serverData, ...PLUGIN_AUTHDATA, pub, name, proj, authority.key);
}

/**
 * `PluginContext.dirs.data` for `job` (Task 7 puts it in the context): its plugin's data
 * directory for the job's authority, created (0700) on this first use; `undefined` unless the
 * job's grants hold `fs pluginData`.
 */
export function jobDataDir(serverData: string, job: JobView, authority: Authority | undefined): { readonly path: string; readonly write: boolean } | undefined {
  if (authority === undefined || !isJobRights(job.rights) || job.via === undefined) return undefined;
  const access = pluginDataAccess(job.rights.grants as ReadonlyArray<Permission>);
  if (access === undefined) return undefined;
  const path = pluginDataPath(serverData, job.via.plugin, projectOfJob(job), job.rights, authority);
  mkdirSync(path, { recursive: true, mode: 0o700 });
  if (process.platform !== "win32") chmodSync(path, 0o700);
  return { path, write: access === "readWrite" };
}

/** `dir`'s entries that are directories; none when it is absent. */
function subdirs(dir: string): string[] {
  try {
    return readdirSync(dir, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
  } catch {
    return [];
  }
}

/** Removes every authority directory for which `stale(key)` holds; how many. */
function removeAuthData(serverData: string, stale: (key: string) => boolean): number {
  const root = join(serverData, ...PLUGIN_AUTHDATA);
  let n = 0;
  for (const pub of subdirs(root))
    for (const name of subdirs(join(root, pub)))
      for (const proj of subdirs(join(root, pub, name)))
        for (const key of subdirs(join(root, pub, name, proj))) {
          if (!stale(key)) continue;
          rmSync(join(root, pub, name, proj, key), { recursive: true, force: true });
          n++;
        }
  return n;
}

/**
 * `cls`'s authority directories of a generation older than `generation` (a bump's, or a
 * session's end). Never a newer one: a later bump's processes may already use theirs.
 */
export function removeClassAuthData(serverData: string, cls: string, generation: number): number {
  const slug = classSlug(cls);
  return removeAuthData(serverData, (key) => {
    const a = parseAuthorityKey(key);
    return a !== undefined && a.slug === slug && a.generation < generation;
  });
}

/** The class a slug names, when `classSlug` kept it readable (`agent-<id>` → `agent:<id>`); a hashed slug names none. */
export function classOfSlug(slug: string): string | undefined {
  if (!/^[a-z]+(-[A-Za-z0-9_]+)?$/.test(slug)) return undefined;
  return slug.replace("-", ":");
}

/**
 * Raises each class to the highest generation its authority directories name (review of Task 7:
 * a bump the store lost while a process of it wrote `pluginData` must not make the start's sweep
 * keep the retired generation's data and remove the live one's). Run before `sweepAuthData`.
 */
export function raiseFromAuthData(serverData: string, gens: AuthorityGenerations): void {
  const highest = new Map<string, number>();
  removeAuthData(serverData, (key) => {
    const a = parseAuthorityKey(key);
    if (a !== undefined && a.generation > (highest.get(a.slug) ?? -1)) highest.set(a.slug, a.generation);
    return false;
  });
  for (const [slug, n] of highest) {
    const cls = classOfSlug(slug);
    if (cls !== undefined) gens.raise(cls, n);
  }
}

/** At start: authority directories of a generation that is not their class's now, or unreadable. */
export function sweepAuthData(serverData: string, gens: AuthorityGenerations): number {
  const now = gens.bySlug();
  mkdirSync(join(serverData, ...PLUGIN_AUTHDATA), { recursive: true, mode: 0o700 });
  return removeAuthData(serverData, (key) => {
    const a = parseAuthorityKey(key);
    return a === undefined || a.generation !== (now.get(a.slug) ?? 0);
  });
}
