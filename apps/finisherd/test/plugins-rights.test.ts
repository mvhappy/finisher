// sp8b Task 6: the `plugin` principal's rights (invoker ∩ grants, Decision 8) and reach (spec
// §3.6): every host method × granted or not × own project, another project, another branch
// subtree, another library; a plugin never holds more than its invoker or its grants, never a
// human-only scope; a revoked or narrowed invoker narrows the next call; an agent's reads stay
// in its session's subtree (7A-2). Task 7 calls `narrowRights` and `authorizeMethod` from
// `onCall`; these tests pin the rules it applies.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FinError, HUMAN_ONLY, type JobOwner, type Principal, SCOPES, type Session } from "@finisher/contracts";
import type { HostMethod, JobRights, Permission } from "@finisher/core-types";
import { Effect } from "effect";
import fc from "fast-check";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BranchLineage, BranchReach, type SessionSource } from "../src/agent/reach.ts";
import { Credentials } from "../src/auth/credentials.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { agentSubtree, authorizeCall, authorizeMethod, type ReachDeps } from "../src/plugins/host/reach.ts";
import { type Invoker, liveInvokers, METHOD_PERMISSION, narrowRights, PLUGIN_SCOPES, pluginActor, requirePermission, rightsFor } from "../src/plugins/host/rights.ts";

/** A check's error code, or `undefined` when it passes. */
const code = (e: Effect.Effect<unknown, FinError>): Promise<string | undefined> =>
  Effect.runPromise(Effect.match(e, { onFailure: (err) => err.code, onSuccess: () => undefined }));
/** A check's error (it must fail). */
const failure = (e: Effect.Effect<unknown, FinError>): Promise<FinError> => Effect.runPromise(Effect.flip(e));

const P_OWN = "01POWN";
const P_OTHER = "01POTHER";
const NET: Permission = { perm: "net", origins: ["https://api.example.com"] };
const ALL_GRANTS: ReadonlyArray<Permission> = [{ perm: "project:read" }, { perm: "project:write" }, { perm: "media:read" }, { perm: "jobs:submit" }, NET];

/** The branches of the projects: main → br1 (an agent's root) → br2; main → br3. */
const BRANCHES = [
  { id: "main", parent: null },
  { id: "br1", parent: "main" },
  { id: "br2", parent: "br1" },
  { id: "br3", parent: "main" },
  { id: "kid", parent: "br1" },
  { id: "kid2", parent: "kid" },
];
const projects: ChainProjects = {
  isOpen: () => true,
  dirOf: () => undefined,
  resolveBranch: (_, ref) => Effect.succeed(ref),
  branches: () => Effect.succeed(BRANCHES.map((b) => ({ ...b, createdBy: "x" }))),
};

const SESSION = { id: "01S0000000000000000000000S", projectId: P_OWN, role: "editor", rootBranch: "br1" } as unknown as Session;
const CHILD = { id: "01S0000000000000000000000C", projectId: P_OWN, role: "fixer", rootBranch: "kid", parent: SESSION.id } as unknown as Session;

function sessionSource(live: Session[]): SessionSource {
  return {
    get: (id) => live.find((s) => s.id === id),
    tree: { liveRoots: (pid) => live.filter((s) => s.projectId === pid).map((s) => ({ id: s.id, rootBranch: s.rootBranch as string })) },
    recordTx: () => {},
  };
}

function deps(live: Session[] = [SESSION]): ReachDeps {
  const sessions = sessionSource(live);
  return { subtree: agentSubtree(sessions, new BranchReach(sessions, new BranchLineage(), projects)), projectLibraries: (p, l) => p === P_OWN && l === "libOwn" };
}

const MCP: Invoker = { kind: "mcp", id: "01MCP", scopes: ["project.read", "project.write", "library.read", "jobs.submit"], projects: [P_OWN] };
const AGENT: Invoker = { kind: "agent", id: SESSION.id, scopes: ["project.read", "project.write"], projects: [P_OWN] };

const rightsOf = (invoker: Invoker, grants: ReadonlyArray<Permission>, sessionRoot?: string): JobRights => rightsFor({ invoker, grants, class: "x", sessionRoot });

describe("rights", () => {
  test("rights_are_invoker_intersect_grants: an agent without project.write cannot commit through a plugin that has project:write", () => {
    const reader: Invoker = { ...AGENT, scopes: ["project.read"] };
    const r = rightsOf(reader, ALL_GRANTS, "br1");
    expect(r.scopes).toEqual(["project.read"]);
    expect(requirePermission(r, "project:write")).toMatchObject({ code: "PLUGIN_PERMISSION_DENIED", details: { perm: "project:write", missing: "invoker" } });
    expect(requirePermission(r, "project:read")).toBeUndefined();
    // The plugin without the grant: the invoker's own scope does not lend it.
    const narrow = rightsOf(MCP, [{ perm: "project:read" }]);
    expect(narrow.scopes).toEqual(["project.read"]);
    expect(requirePermission(narrow, "project:write")).toMatchObject({ code: "PLUGIN_PERMISSION_DENIED", details: { perm: "project:write", missing: "grant" } });
    expect(requirePermission(narrow, "net")).toMatchObject({ details: { perm: "net", missing: "grant" } });
    // `net` is the plugin's own permission: no invoker scope maps to it.
    expect(requirePermission(rightsOf(MCP, [NET]), "net")).toBeUndefined();
    // Library reach follows `library.read` ∩ `media:read`.
    expect(rightsOf(MCP, ALL_GRANTS).libraries).toBe("all");
    expect(rightsOf(MCP, [{ perm: "project:read" }]).libraries).toEqual([]);
    expect(rightsOf(AGENT, ALL_GRANTS, "br1").libraries).toEqual([]);
    // Shape: the invoker, its project reach, an agent's subtree, the grants as approved.
    expect(rightsOf(AGENT, ALL_GRANTS, "br1")).toMatchObject({ principal: { kind: "agent", id: SESSION.id }, projects: [P_OWN], branch: { root: "br1", subtree: true }, grants: ALL_GRANTS });
    expect(rightsOf(LOCAL, ALL_GRANTS)).toMatchObject({ projects: "all" });
    expect(rightsOf(LOCAL, ALL_GRANTS).branch).toBeUndefined();
  });

  test("no_human_only_scope_ever: whatever the grants and the invoker, only the four mapped scopes", () => {
    const perms: Permission[] = [...ALL_GRANTS, { perm: "media:file" }, { perm: "gpu" }, { perm: "fs", access: "readWrite", scope: "pluginData" } as Permission, { perm: "secrets", mode: "raw", names: ["k"] }];
    const forbidden = new Set<string>([...HUMAN_ONLY, "branch.protected", "review.decide", "project.admin", "auth.admin", "diagnostics.read", "diagnostics.record", "diagnostics.admin"]);
    fc.assert(
      fc.property(fc.subarray(perms), fc.subarray([...SCOPES]), fc.constantFrom("console", "device", "local", "agent", "mcp", "system"), (grants, scopes, kind) => {
        const r = rightsFor({ invoker: { kind, id: "i", scopes, projects: "*" }, grants, class: "c" });
        for (const s of r.scopes) {
          expect(PLUGIN_SCOPES).toContain(s);
          expect(forbidden.has(s)).toBe(false);
          expect(scopes as string[]).toContain(s);
        }
        // Never wider than either side: each scope needs both its grant and the invoker's scope.
        expect(r.scopes.length).toBeLessThanOrEqual(Math.min(scopes.length, grants.length));
      }),
      { numRuns: 500 },
    );
    // Even an all-scope person (the console, a device) lends a plugin only the four.
    expect(rightsFor({ invoker: { ...CONSOLE, scopes: [...SCOPES] }, grants: ALL_GRANTS, class: "person" }).scopes).toEqual(PLUGIN_SCOPES);
  });

  test("the actor is plugin:<id>@<version> acting for the invoker", () => {
    expect(pluginActor("@acme/tools@1.2.0", rightsOf(MCP, ALL_GRANTS))).toEqual({ kind: "plugin", id: "@acme/tools@1.2.0", for: { kind: "mcp", id: "01MCP" } });
  });
});

describe("reach", () => {
  type Target = "own" | "otherProject" | "otherSubtree" | "otherLibrary";
  const METHODS = Object.keys(METHOD_PERMISSION) as HostMethod[];
  const TARGETS: Record<Target, { project?: string; branch?: string; library?: string }> = {
    own: { project: P_OWN, branch: "br2" },
    otherProject: { project: P_OTHER, branch: "main" },
    otherSubtree: { project: P_OWN, branch: "br3" },
    otherLibrary: { project: P_OWN, branch: "br2", library: "libOther" },
  };

  /** What a call answers, for an agent (in a session's subtree) and an MCP credential (no branch limit). */
  function expected(method: HostMethod, granted: boolean, target: Target, invoker: "agent" | "mcp"): string | undefined {
    const perm = METHOD_PERMISSION[method];
    if (perm !== undefined && !granted) return "PLUGIN_PERMISSION_DENIED";
    if (target === "otherProject") return "PROJECT_NOT_FOUND";
    if (target === "otherSubtree") return invoker === "agent" ? "BRANCH_NOT_ALLOWED" : undefined;
    if (target === "otherLibrary") return "LIBRARY_NOT_FOUND";
    return undefined;
  }

  test("permission_reach_matrix: each host method × granted or not × own, other project, other branch subtree, other library", async () => {
    const rows: string[] = [];
    for (const invoker of ["agent", "mcp"] as const) {
      for (const method of METHODS) {
        for (const granted of [true, false]) {
          const perm = METHOD_PERMISSION[method];
          const grants = granted ? ALL_GRANTS : ALL_GRANTS.filter((g) => g.perm !== perm);
          // The MCP credential without `library.read`: in v1 `library.read` reaches every library
          // (`library/reach.ts`), so the "other library" column needs an invoker without it.
          const rights = invoker === "agent" ? rightsOf(AGENT, grants, "br1") : rightsOf({ ...MCP, scopes: ["project.read", "project.write"] }, grants);
          for (const target of Object.keys(TARGETS) as Target[]) {
            const got = await code((authorizeMethod(rights, method, TARGETS[target], deps())));
            const want = expected(method, granted, target, invoker);
            rows.push(`${invoker} ${method} ${granted ? "granted" : "denied"} ${target}: ${got ?? "ok"}`);
            expect(got, rows.at(-1)).toBe(want);
          }
        }
      }
    }
    expect(rows).toHaveLength(2 * METHODS.length * 2 * 4);
    // The answer for another project is the chain's own answer for an unknown one (`outOfReach`): no id leaks.
    const other = await failure(authorizeMethod(rightsOf(MCP, ALL_GRANTS), "project.snapshot", { project: P_OTHER }, deps()));
    const unknown = await failure(authorizeMethod(rightsOf(MCP, ALL_GRANTS), "project.snapshot", { project: "01PNONE" }, deps()));
    expect(JSON.stringify(other).replaceAll(P_OTHER, "X")).toBe(JSON.stringify(unknown).replaceAll("01PNONE", "X"));
  });

  // The project route matters once library reach is per library (v1: `library.read` reaches
  // every library, so `rightsFor` never builds `library.read` with a list); the rights here are
  // built by hand to pin the rule for then.
  test("library reads: through a project in reach that references it, or a library in reach", async () => {
    const lib = (rights: JobRights, library: string, project?: string) => (authorizeCall(rights, "media:read", { library, ...(project === undefined ? {} : { project }) }, deps()));
    // MCP holds library.read and the plugin media:read: every library.
    expect(await code(lib(rightsOf(MCP, ALL_GRANTS), "libOther"))).toBeUndefined();
    // Without library.read, only through its project, and only a library that project references.
    const noLib: Invoker = { ...MCP, scopes: ["project.read"] };
    const r = { ...rightsOf(noLib, ALL_GRANTS), scopes: ["library.read" as const] };
    expect(await code(lib(r, "libOwn", P_OWN))).toBeUndefined();
    expect(await code(lib(r, "libOther", P_OWN))).toBe("LIBRARY_NOT_FOUND");
    expect(await code(lib(r, "libOwn", P_OTHER))).toBe("PROJECT_NOT_FOUND");
    expect(await code(lib(r, "libOwn"))).toBe("LIBRARY_NOT_FOUND");
  });

  test("an agent's reach needs its session: no root, no project; a branch read names its branch; a root-only limit", async () => {
    const sessionless = rightsOf(AGENT, ALL_GRANTS);
    expect(sessionless).toMatchObject({ projects: [], libraries: [] });
    expect(await code(authorizeMethod(sessionless, "project.view", { project: P_OWN, branch: "br1" }, deps()))).toBe("PROJECT_NOT_FOUND");
    // Rights that lost their branch (not admitted at submit) still fail closed.
    const { branch: _b, ...stripped } = rightsOf(AGENT, ALL_GRANTS, "br1");
    expect(await code(authorizeMethod(stripped, "project.view", { project: P_OWN, branch: "main" }, deps()))).toBe("BRANCH_NOT_ALLOWED");
    // `project.snapshot` and `project.view` with no branch: refused for an agent (no default chosen later), fine for others.
    for (const m of ["project.snapshot", "project.view"] as const) {
      expect(await code(authorizeMethod(rightsOf(AGENT, ALL_GRANTS, "br1"), m, { project: P_OWN }, deps()))).toBe("BRANCH_NOT_ALLOWED");
      expect(await code(authorizeMethod(rightsOf(MCP, ALL_GRANTS), m, { project: P_OWN }, deps()))).toBeUndefined();
    }
    // Non-branch methods with the job's project only: no branch needed.
    expect(await code(authorizeMethod(rightsOf(AGENT, ALL_GRANTS, "br1"), "log", { project: P_OWN }, deps()))).toBeUndefined();
    // `subtree: false` is the root alone.
    const rootOnly = { ...rightsOf(AGENT, ALL_GRANTS, "br1"), branch: { root: "br1", subtree: false } };
    expect(await code(authorizeMethod(rootOnly, "project.view", { project: P_OWN, branch: "br1" }, deps()))).toBeUndefined();
    expect(await code(authorizeMethod(rootOnly, "project.view", { project: P_OWN, branch: "br2" }, deps()))).toBe("BRANCH_NOT_ALLOWED");
  });

  test("agent_branch_subtree_enforced: its root and beneath; not a live child's subtree, nor after the session is gone", async () => {
    const rights = rightsOf(AGENT, ALL_GRANTS, "br1");
    const read = (branch: string, live: Session[] = [SESSION]) => (authorizeMethod(rights, "project.view", { project: P_OWN, branch }, deps(live)));
    for (const b of ["br1", "br2", "kid", "kid2"]) expect(await code(read(b)), b).toBeUndefined();
    for (const b of ["main", "br3", "nope"]) expect(await code(read(b)), b).toBe("BRANCH_NOT_ALLOWED");
    // A live child's root and beneath are the child's alone.
    expect(await code(read("kid", [SESSION, CHILD]))).toBe("BRANCH_NOT_ALLOWED");
    expect(await code(read("kid2", [SESSION, CHILD]))).toBe("BRANCH_NOT_ALLOWED");
    expect(await code(read("br2", [SESSION, CHILD]))).toBeUndefined();
    // The session ended (no longer known): nothing.
    expect(await code(read("br1", []))).toBe("BRANCH_NOT_ALLOWED");
    // Rights naming another root than the session's (a stale snapshot) reach nothing.
    const stale = { ...rights, branch: { root: "br2", subtree: true } };
    expect(await code((authorizeMethod(stale, "project.view", { project: P_OWN, branch: "br2" }, deps())))).toBe("BRANCH_NOT_ALLOWED");
    // The answer names the branch, as the chain's own `BRANCH_NOT_ALLOWED` does.
    expect((await failure(read("br3"))).details).toMatchObject({ branch: "br3" });
  });
});

describe("live invokers", () => {
  let dir: string;
  let store: ControlStore;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "finisherd-plugin-rights-"));
    mkdirSync(join(dir, "data"), { mode: 0o700 });
    store = openControlStore(join(dir, "data"), { serverId: "sid-1", checkpoint: false });
  });
  afterEach(async () => {
    await store.close();
    rmSync(dir, { recursive: true, force: true });
  });

  test("revoked_invoker_narrows_running_call: the next call after the credential is revoked fails; a narrowed one narrows", () => {
    const creds = new Credentials(store, Buffer.alloc(32, 7), new Audit(store));
    const { credential } = creds.mint(CONSOLE, { kind: "mcp", label: "m", scopes: ["project.read", "project.write", "library.read"], projects: [P_OWN, P_OTHER] });
    const owner: JobOwner = { kind: "mcp", id: credential.id };
    const live = liveInvokers({ credential: (ref) => creds.lookup(ref)?.principal });
    // At submit: the rights the job stores.
    const atSubmit = rightsFor({ invoker: live(owner) as Invoker, grants: ALL_GRANTS, class: `mcp:${credential.id}` });
    expect(atSubmit.scopes).toEqual(["project.read", "project.write", "library.read"]);
    // A call while the credential lives: the same rights.
    expect(narrowRights(atSubmit, live(owner))).toEqual(atSubmit);
    // Revoked: the next call fails.
    creds.markEnded({ kind: "mcp", id: credential.id }, "owner");
    expect(live(owner)).toBeUndefined();
    expect(narrowRights(atSubmit, live(owner))).toMatchObject({ code: "CREDENTIAL_REVOKED" });
    creds.dispose();

    // A narrowed invoker (fewer scopes, fewer projects): the next call has the intersection.
    const narrower: Principal = { kind: "mcp", id: credential.id, scopes: ["project.read"], projects: [P_OWN], branches: "*", human: false };
    const n = narrowRights(atSubmit, narrower) as JobRights;
    expect(n.scopes).toEqual(["project.read"]);
    expect(n.projects).toEqual([P_OWN]);
    expect(n.libraries).toEqual([]);
    expect(requirePermission(n, "project:write")).toMatchObject({ code: "PLUGIN_PERMISSION_DENIED" });
    // A wider invoker never widens what the job was given.
    const wider: Principal = { ...narrower, scopes: [...SCOPES], projects: "*" };
    expect(narrowRights(n, wider)).toEqual(n);
    // Another principal is never this job's invoker.
    expect(narrowRights(atSubmit, { ...narrower, id: "someone-else" })).toMatchObject({ code: "CREDENTIAL_REVOKED" });
  });

  test("console, local and system never end; devices and credentials only while live", () => {
    const live = liveInvokers({ device: (id) => (id === "d1" ? { kind: "device", id, scopes: [...SCOPES], projects: "*", branches: "*", human: true } : undefined) });
    expect(live({ kind: "console", id: "console" })).toBe(CONSOLE);
    expect(live({ kind: "local", id: "local" })).toBe(LOCAL);
    const sys = live({ kind: "system", id: "plugins" }) as Invoker;
    expect(sys.scopes.some((s) => HUMAN_ONLY.has(s))).toBe(false);
    expect(live({ kind: "device", id: "d1" })?.id).toBe("d1");
    expect(live({ kind: "device", id: "gone" })).toBeUndefined();
    expect(live({ kind: "agent", id: "x" })).toBeUndefined();
    expect(live({ kind: "anonymous", id: "" })).toBeUndefined();
  });
});
