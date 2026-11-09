// A plugin job's rights (sp8 spec §3.7; sp8b plan Decision 8): the invoker's principal
// intersected with the plugin's grants, computed once when the job is submitted (stored as the
// job's `rights`) and narrowed again on every host call against the invoker as it is now, so a
// revoked credential fails the next call and a narrowed one narrows it.
//
// Mapping (spec §3.7): `project:read` → `project.read`; `project:write` → `project.write`
// (only the `tx.commit` of a command's returned transaction, Task 9); `media:read` →
// `library.read` (and asset reads); `jobs:submit` → `jobs.submit` (the plugin's own kinds,
// 8C). A plugin holds a scope only when the invoker holds it **and** the plugin was granted
// its permission; the other permissions (`net`, `fs`, …) are the plugin's own and are read
// from `grants`. No other scope is representable (`PluginScope` is a closed set of four), so
// no plugin ever holds a human-only scope, `branch.protected`, `review.decide`,
// `project.admin`, `auth.admin` or `diagnostics.*`.
import { createHash } from "node:crypto";
import type { FinError, JobOwner, Principal, Scope } from "@finisher/contracts";
import { HUMAN_ONLY, SCOPES } from "@finisher/contracts";
import type { HostMethod, JobRights, Permission, PluginScope, Reach } from "@finisher/core-types";
import { finError } from "../../errors.ts";
import { CONSOLE, LOCAL } from "../../principal.ts";

/** The permissions that map to a sub-project 5 scope, and that scope (spec §3.7). */
export const SCOPE_OF_PERMISSION = {
  "project:read": "project.read",
  "project:write": "project.write",
  "media:read": "library.read",
  "jobs:submit": "jobs.submit",
} as const satisfies Record<string, PluginScope & Scope>;

export type ScopedPermission = keyof typeof SCOPE_OF_PERMISSION;

/** Every scope a plugin may hold, in a fixed order. */
export const PLUGIN_SCOPES: ReadonlyArray<PluginScope> = Object.values(SCOPE_OF_PERMISSION);

/** The permission each host method of host API 1.1 needs (`log` needs none). */
export const METHOD_PERMISSION: Readonly<Record<HostMethod, Permission["perm"] | undefined>> = {
  "project.snapshot": "project:read",
  "project.view": "project:read",
  "net.fetch": "net",
  "net.read": "net",
  log: undefined,
};

/** The invoker as rights read it: a principal's kind, id, scopes and project reach. */
export type Invoker = Pick<Principal, "id" | "scopes" | "projects"> & { readonly kind: string };

/**
 * Work the host starts itself (`system` owner): every scope a plugin can map to, every project.
 * It is no person, so it never holds a human-only scope either.
 */
export function systemInvoker(id: string): Invoker {
  return { kind: "system", id, scopes: SCOPES.filter((s) => !HUMAN_ONLY.has(s)), projects: "*" };
}

const toReach = (r: "*" | ReadonlyArray<string>): Reach => (r === "*" ? "all" : [...r]);

const intersect = (a: Reach, b: Reach): Reach => (a === "all" ? (b === "all" ? "all" : [...b]) : b === "all" ? [...a] : a.filter((x) => b.includes(x)));

/** Whether `reach` includes `id`. */
export function reaches(reach: Reach, id: string): boolean {
  return reach === "all" || reach.includes(id);
}

/** The rights an authority key hashes (spec §3.3): everything plugin-relevant but the principal. */
export type HashedRights = Pick<JobRights, "class" | "scopes" | "projects" | "branch" | "libraries" | "grants">;

/** JSON with object keys sorted and (with `sortLists`) string lists sorted. */
function canonical(v: unknown, sortLists: boolean): string {
  if (Array.isArray(v)) {
    const items = sortLists && v.every((x) => typeof x === "string") ? [...new Set(v as string[])].sort() : v;
    return `[${items.map((x) => canonical(x, sortLists)).join(",")}]`;
  }
  if (v !== null && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => o[k] !== undefined)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(o[k], sortLists)}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/**
 * The hash of the rights' authority (spec §3.3, Decision 28): the class, the scopes, the
 * project, branch and library reach and the grants, order-insensitive, without the principal,
 * so two callers of one class with identical rights hash the same and any difference splits
 * them. 32 lower-case hex characters.
 */
export function rightsHash(r: HashedRights): string {
  const core = {
    class: r.class,
    scopes: r.scopes,
    projects: r.projects,
    branch: r.branch ?? null,
    libraries: r.libraries,
    grants: r.grants.map((g) => canonical(g, true)).sort(),
  };
  return createHash("sha256").update(canonical(core, true)).digest("hex").slice(0, 32);
}

export interface RightsInput {
  readonly invoker: Invoker;
  /** The plugin's permissions as granted for the version the job runs (`plugin_grants`). */
  readonly grants: ReadonlyArray<Permission>;
  /** The process key's principal class (`principalClass`). */
  readonly class: string;
  /** An agent session's root branch (7A-2): the job's branch reach is its subtree. */
  readonly sessionRoot?: string | undefined;
  /** The class's policy generation now (`AuthorityGenerations.of`); 0 when omitted. */
  readonly generation?: number | undefined;
}

/** Decision 8: invoker ∩ grants, as the job stores it. */
export function rightsFor(input: RightsInput): JobRights {
  const granted = new Set(input.grants.map((g) => g.perm));
  const held = new Set<string>(input.invoker.scopes);
  const scopes = (Object.entries(SCOPE_OF_PERMISSION) as Array<[ScopedPermission, PluginScope]>)
    .filter(([perm, scope]) => granted.has(perm) && held.has(scope))
    .map(([, scope]) => scope);
  // An `agent` credential that is no session's (no root branch) reaches no project: an agent's
  // reads are bounded by its session's subtree, and without one there is nothing to bound them.
  const sessionless = input.invoker.kind === "agent" && input.sessionRoot === undefined;
  // v1 library reach (`library/reach.ts`): `library.read` reaches every library of this server.
  const libraries: Reach = scopes.includes("library.read") && !sessionless ? "all" : [];
  const hashed: HashedRights = {
    class: input.class,
    scopes,
    projects: sessionless ? [] : toReach(input.invoker.projects),
    ...(input.sessionRoot === undefined ? {} : { branch: { root: input.sessionRoot, subtree: true } }),
    libraries,
    grants: input.grants.map((g) => structuredClone(g)),
  };
  return { principal: { kind: input.invoker.kind, id: input.invoker.id }, ...hashed, rightsHash: rightsHash(hashed), generation: input.generation ?? 0 };
}

/**
 * `rights` narrowed by the invoker as it is now (`live`): scopes, projects and libraries are
 * each intersected, so nothing the job was given at submit widens; a revoked or expired
 * invoker (`live` undefined) is `CREDENTIAL_REVOKED`. The grants are the job's own (an update
 * runs a new version under new rights; a revocation stops the plugin, Task 5). `rightsHash`
 * and `generation` stay the job's: they name the authority (and process) the job runs under,
 * not what one call may do now.
 */
export function narrowRights(rights: JobRights, live: Invoker | undefined): JobRights | FinError {
  if (live === undefined || live.kind !== rights.principal.kind || live.id !== rights.principal.id) {
    return finError(
      "CREDENTIAL_REVOKED",
      `the ${rights.principal.kind} this plugin acts for is no longer allowed to act`,
      { principal: rights.principal.kind },
      "Run the command again with a live credential.",
    );
  }
  const now = rightsFor({ invoker: live, grants: rights.grants, class: rights.class, sessionRoot: rights.branch?.root ?? undefined });
  const scopes = rights.scopes.filter((s) => now.scopes.includes(s));
  return {
    ...rights,
    scopes,
    projects: intersect(rights.projects, now.projects),
    libraries: scopes.includes("library.read") ? intersect(rights.libraries, now.libraries) : [],
  };
}

export function permissionDenied(perm: string, missing: "grant" | "invoker"): FinError {
  return finError(
    "PLUGIN_PERMISSION_DENIED",
    missing === "grant" ? `the plugin was not granted ${perm}` : `the plugin has ${perm}, but whoever ran it does not`,
    { perm, missing },
    missing === "grant"
      ? "A person approves a plugin's permissions when installing or updating it: update the plugin to a version that asks for this permission."
      : "Run the command as a principal that holds this right.",
  );
}

/**
 * Whether `rights` hold `perm`: a scope-mapped permission needs the grant and the invoker's
 * scope; any other permission needs the grant. `PLUGIN_PERMISSION_DENIED {perm, missing}`.
 */
export function requirePermission(rights: JobRights, perm: Permission["perm"]): FinError | undefined {
  if (!rights.grants.some((g) => g.perm === perm)) return permissionDenied(perm, "grant");
  const scope = (SCOPE_OF_PERMISSION as Record<string, PluginScope | undefined>)[perm];
  if (scope !== undefined && !rights.scopes.includes(scope)) return permissionDenied(perm, "invoker");
  return undefined;
}

/** The principal of a job's owner as it is now, or `undefined` when it has ended. */
export type LiveInvoker = (owner: JobOwner) => Invoker | undefined;

export interface LiveSources {
  /** `agent` and `mcp` credentials (`Credentials.lookup`): live ones only. */
  readonly credential?: (ref: { kind: "agent" | "mcp"; id: string }) => Principal | undefined;
  /** Devices (`Devices.get` → `principalOf`): live ones only. */
  readonly device?: (id: string) => Principal | undefined;
}

/** The live invoker of each principal kind: `console`, `local` and `system` never end. */
export function liveInvokers(src: LiveSources): LiveInvoker {
  return (owner) => {
    switch (owner.kind) {
      case "console":
        return CONSOLE;
      case "local":
        return LOCAL;
      case "system":
        return systemInvoker(owner.id);
      case "agent":
      case "mcp":
        return src.credential?.({ kind: owner.kind, id: owner.id });
      case "device":
        return src.device?.(owner.id);
      default:
        return undefined;
    }
  };
}

/**
 * Who acts, for audit and the journal (spec §3.7): `plugin:<id>@<version>` acting for the
 * invoker. `via` is the job's `via.plugin`.
 */
export function pluginActor(via: string, rights: JobRights): { readonly kind: "plugin"; readonly id: string; readonly for: { readonly kind: string; readonly id: string } } {
  return { kind: "plugin", id: via, for: { kind: rights.principal.kind, id: rights.principal.id } };
}
