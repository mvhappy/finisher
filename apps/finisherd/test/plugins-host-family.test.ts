// The `plugin` family (sp8b plan Task 4): programs, environment, grants, limits and warm keys,
// with a test-only plugin process (`fixtures/plugins/probe`, a Node ESM script that speaks the
// envelope by hand) installed by hand into a temp server data's store. Every process here is
// one this test's engine started; nothing else is killed.
import { appendFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinErrorData, JobOwner, JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { exitPluginId, type KindSpec, makeKindRegistry, MAX_PLUGIN_WARM_SLOTS } from "../src/jobs/kinds.ts";
import { warmOverrun } from "../src/jobs/schedule.ts";
import { mapPluginError, PLUGIN_LIMITS, pluginFamily, type PluginFamilyDeps, pluginProgram, registerPluginFamily } from "../src/plugins/host/family.ts";
import { keyTmpDir, pluginWarmKey, principalClass, type RootSessionOf, sessionRoots } from "../src/plugins/host/keys.ts";
import { rightsFor } from "../src/plugins/host/rights.ts";
import { type AuthorityGenerations, authorityOfWarmKey, memoryGenerations } from "../src/plugins/host/authority.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { openControlStore } from "../src/control/db.ts";
import type { ControlStore } from "../src/control/db.ts";
import { DENIED_NODE_FLAGS } from "../src/plugins/host/node-runtime.ts";
import { storeDirOf, unusable } from "../src/plugins/host/program.ts";
import { type InstalledFull, PluginTables } from "../src/plugins/tables.ts";
import { closeTestEngines, run, type TestEngine, testEngine } from "./support/jobs.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ID = "@test/probe";
const PERSON: JobOwner = { kind: "local", id: "local" };
const SYSTEM: JobOwner = { kind: "system", id: "test" };
const LINUX = process.platform === "linux";

/** `test.echo` in the `plugin` family (its params type is the test kinds' `TestEchoParams`). */
const ECHO: KindSpec = { kind: "test.echo", version: 1, family: "plugin", scopes: [], resources: [], warm: true, defaultPriority: "interactive", qos: "normal", timeoutS: 60, maxAttempts: 1 };

const dirs: string[] = [];
afterEach(async () => {
  await closeTestEngines();
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

const tempData = (): string => {
  const d = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-host-")));
  dirs.push(d);
  return d;
};

/** Copies the probe into the store as `version` and returns its row's `dir`. */
function placeProbe(serverData: string, version: string, extra: Record<string, string> = {}): string {
  const dir = `store/@test/probe/${version}-t`;
  const full = join(serverData, "plugins", ...dir.split("/"));
  mkdirSync(dirname(full), { recursive: true });
  cpSync(FIXTURE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, version }));
  for (const [name, body] of Object.entries(extra)) writeFileSync(join(full, name), body);
  return dir;
}

/** `sessionRoot`: an agent owner's session root branch (default `br1`); `projects`: the invoker's reach (default all). */
type SubmitOpts = { owner?: JobOwner; project?: string; version?: string; priority?: "interactive" | "normal"; sessionRoot?: string; projects?: string[] };

interface PluginEngine {
  readonly t: TestEngine;
  readonly serverData: string;
  readonly tables: PluginTables;
  install(version?: string, extra?: Record<string, string>): string;
  readonly gens: AuthorityGenerations;
  submit(op: Record<string, unknown>, spec?: SubmitOpts): Promise<JobView>;
  /** The warm key a job of `spec` gets now. */
  keyOf(spec?: SubmitOpts): string;
  echo(op: Record<string, unknown>, spec?: Parameters<PluginEngine["submit"]>[1]): Promise<Record<string, unknown>>;
}

async function pluginEngine(): Promise<PluginEngine> {
  const serverData = tempData();
  let tables: PluginTables | undefined;
  let roots: RootSessionOf | undefined;
  const gens = memoryGenerations();
  const t = await testEngine({
    serverData,
    testKinds: false,
    timing: { hungMs: 1500 },
    register: (r) => {
      registerPluginFamily(r, { serverData, installed: (id) => tables?.installedFull(id), rootSessionOf: (id) => roots?.(id), generations: gens });
      r.registerKind(ECHO);
    },
  });
  tables = new PluginTables(t.control);
  roots = sessionRoots(t.control);
  const rootOf: RootSessionOf = (id) => roots?.(id);
  const rightsOf = (spec: SubmitOpts) => {
    const owner = spec.owner ?? PERSON;
    const cls = principalClass(owner, rootOf);
    return rightsFor({
      invoker: { ...owner, scopes: [], projects: spec.projects ?? "*" },
      grants: [],
      class: cls,
      sessionRoot: owner.kind === "agent" ? (spec.sessionRoot ?? "br1") : undefined,
      generation: gens.of(cls),
    });
  };
  const pe: PluginEngine = {
    t,
    serverData,
    tables,
    gens,
    keyOf: (spec = {}) =>
      pluginWarmKey({ id: "k", owner: spec.owner ?? PERSON, via: { plugin: `${ID}@${spec.version ?? "1.0.0"}` }, rights: rightsOf(spec), ...(spec.project === undefined ? {} : { project: spec.project }) } as unknown as JobView, rootOf, gens),
    install(version = "1.0.0", extra = {}) {
      const dir = placeProbe(serverData, version, extra);
      const row = { id: ID, version, source: "registry" as const, treeHash: "t", dir, kind: "code" as const, enabled: true, state: "active" as const };
      if (tables!.installedOne(ID) === undefined) tables!.addPlugin({ ...row, installedBy: "console:console", installedAt: 0 });
      else tables!.replacePlugin(row, 1);
      return join(serverData, "plugins", ...dir.split("/"));
    },
    async submit(op, spec = {}) {
      const v = await run(
        t.engine.submit(spec.owner ?? PERSON, {
          kind: "test.echo",
          params: { value: op },
          via: { plugin: `${ID}@${spec.version ?? "1.0.0"}` },
          rights: rightsOf(spec),
          ...(spec.project === undefined ? {} : { project: spec.project }),
          ...(spec.priority === undefined ? {} : { priority: spec.priority }),
        }),
      );
      return t.wait(v.id);
    },
    async echo(op, spec) {
      const v = await pe.submit(op, spec);
      expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
      return v.result as Record<string, unknown>;
    },
  };
  return pe;
}

/** A root agent session and its child (7A-2's session tree) in `control`; their ids. */
async function agentTree(control: ControlStore, dir: string): Promise<{ root: string; child: string }> {
  const clock = manualClock(Date.now());
  const store = new AgentStore(control, dir, clock, systemIds(clock));
  const base: Omit<NewSession, "id"> = {
    projectId: "01project0000000000000000a",
    harness: "builtin",
    provider: "openrouter",
    model: "anthropic/claude-sonnet-5.5",
    weights: "closed",
    role: "editor",
    mode: "draft",
    rootBranch: "br1",
    base: { branch: "main", seq: 1 },
    budget: {},
    createdBy: { principal: { kind: "local", id: "local" } },
  };
  const root = await store.createSession({ ...base, id: systemIds(clock).ulid() });
  const child = await store.createSession({ ...base, id: systemIds(clock).ulid(), role: "fixer", rootBranch: "br2", parent: root.id, rootSession: root.id, depth: 1, createdBy: { principal: { kind: "agent", id: root.id } } });
  return { root: root.id, child: child.id };
}

/** A job of `owner` with rights of its class (no scopes, no grants; an agent's branch `br1`). */
const job = (owner: JobOwner, project?: string, plugin = `${ID}@1.0.0`): JobView => {
  const rights = rightsFor({ invoker: { ...owner, scopes: [], projects: "*" }, grants: [], class: principalClass(owner), sessionRoot: owner.kind === "agent" ? "br1" : undefined });
  return { id: "j1", owner, via: { plugin }, rights, ...(project === undefined ? {} : { project }) } as unknown as JobView;
};
/** The authority part of `job(owner)`'s key. */
const authOf = (owner: JobOwner): string => pluginWarmKey(job(owner)).split("|")[2] as string;

describe("the plugin family's process", () => {
  test("plugin_env_is_the_allow_list", async () => {
    const planted = { FIN_TRAINING: "1", FIN_SERVER_DATA: "/x", OPENAI_API_KEY: "k", FIN_GPU: "0", ELECTRON_RUN_AS_NODE: "1", FIN_TEST_PLANTED: "1" };
    const saved = Object.fromEntries(Object.keys(planted).map((k) => [k, process.env[k]]));
    Object.assign(process.env, planted);
    try {
      const p = await pluginEngine();
      const dir = p.install();
      const r = await p.echo({ op: "env" }, { project: "p1" });
      const env = r.env as Record<string, string>;
      const allowed = new Set(["PATH", "HOME", "LANG", "TMPDIR", "TEMP", "TMP", "FIN_PLUGIN_PROTOCOL", ...(process.platform === "win32" ? ["Path", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "LOCALAPPDATA"] : [])]);
      expect(Object.keys(env).filter((k) => !allowed.has(k) && !k.startsWith("LC_"))).toEqual([]);
      const tmp = keyTmpDir(p.serverData, p.keyOf({ project: "p1" }));
      expect(env).toMatchObject({ FIN_PLUGIN_PROTOCOL: "1", TMPDIR: tmp, TEMP: tmp, TMP: tmp });
      expect(realpathSync(r.cwd as string)).toBe(tmp);
      if (process.platform !== "win32") expect(statSync(tmp).mode & 0o777).toBe(0o700);
      const execArgv = r.execArgv as string[];
      expect(execArgv).toEqual(["--permission", `--allow-fs-read=${dir}`, `--allow-fs-read=${tmp}`, `--allow-fs-write=${tmp}`]);
      for (const f of DENIED_NODE_FLAGS) expect(execArgv.some((a) => a.startsWith(f))).toBe(false);
      expect(r.argv).toEqual([process.execPath, join(dir, "main.mjs"), "--warm"]);
    } finally {
      for (const [k, v] of Object.entries(saved)) {
        if (v === undefined) delete process.env[k];
        else process.env[k] = v;
      }
    }
  });

  test.runIf(LINUX)("only_stdio_inherited (and the open-files limit applies)", async () => {
    const p = await pluginEngine();
    p.install();
    const r = await p.echo({ op: "pid" });
    const pid = r.pid as number;
    // The warm process is still up (`ready`): read its descriptors from outside.
    const fds = readdirSync(`/proc/${pid}/fd`).map(Number).sort((a, b) => a - b);
    const target = (fd: number): string => {
      try {
        return readlinkSync(`/proc/${pid}/fd/${fd}`);
      } catch {
        return "gone";
      }
    };
    for (const fd of [0, 1, 2]) expect(target(fd)).toMatch(/^pipe:/);
    // Node's own handles only (its event loop's pipes and inodes, the `/dev/null` libuv keeps):
    // no file, socket or anything of finisherd's.
    const others = fds.filter((fd) => fd > 2).map((fd) => [fd, target(fd)] as const);
    expect(others.filter(([, t]) => !/^(pipe:|anon_inode:|\/dev\/null$|gone$)/.test(t))).toEqual([]);
    const limits = readFileSync(`/proc/${pid}/limits`, "utf8");
    expect(limits).toMatch(new RegExp(`Max open files\\s+${PLUGIN_LIMITS.openFiles}\\s+${PLUGIN_LIMITS.openFiles}\\s`));
  });

  test("node_permission_flags_on_node_24_22", async () => {
    const p = await pluginEngine();
    const dir = p.install();
    const sibling = join(dirname(dir), "sibling");
    mkdirSync(sibling);
    writeFileSync(join(sibling, "secret"), "x");
    const tmp = keyTmpDir(p.serverData, p.keyOf());
    const r = await p.echo({ op: "probe", read: [join(dir, "main.mjs"), join(sibling, "secret"), join(p.serverData, "control.db")], write: [join(tmp, "ok"), join(dir, "planted")] });
    expect(r).toEqual({
      [`read ${join(dir, "main.mjs")}`]: "ok",
      [`read ${join(sibling, "secret")}`]: "ERR_ACCESS_DENIED",
      [`read ${join(p.serverData, "control.db")}`]: "ERR_ACCESS_DENIED",
      [`write ${join(tmp, "ok")}`]: "ok",
      [`write ${join(dir, "planted")}`]: "ERR_ACCESS_DENIED",
      spawn: "ERR_ACCESS_DENIED",
      worker: "ERR_ACCESS_DENIED",
    });
    expect(existsSync(join(dir, "planted"))).toBe(false);
  });

  test("warm_key_never_crosses: projects, classes, rights and generations each get their own process", async () => {
    const p = await pluginEngine();
    p.install();
    const tree = await agentTree(p.t.control, p.serverData);
    const root: JobOwner = { kind: "agent", id: tree.root };
    const child: JobOwner = { kind: "agent", id: tree.child };
    const mcp: JobOwner = { kind: "mcp", id: "01mcp0000000000000000000000" };
    const cases: SubmitOpts[] = [
      { project: "p1", owner: PERSON },
      { project: "p1", owner: SYSTEM },
      { project: "p2", owner: PERSON },
      { project: "p2", owner: SYSTEM },
      { project: "p1", owner: root, sessionRoot: "br1" },
      // The child session: the root's class, its own root branch (narrower rights).
      { project: "p1", owner: child, sessionRoot: "br2" },
      // One MCP credential with two rights sets: two authorities.
      { project: "p1", owner: mcp },
      { project: "p1", owner: mcp, projects: ["p1"] },
    ];
    const env = async (c: SubmitOpts) => (await p.echo({ op: "env" }, c)) as { pid: number; env: Record<string, string> };
    const first = [];
    for (const c of cases) first.push(await env(c));
    expect(new Set(first.map((r) => r.pid)).size).toBe(cases.length);
    expect(new Set(first.map((r) => r.env.TMPDIR)).size).toBe(cases.length);
    // The child shares its root's class but not its authority: never the root's process.
    expect(p.keyOf(cases[5])).not.toBe(p.keyOf(cases[4]));
    expect(authorityOfWarmKey(p.keyOf(cases[5]))?.slug).toBe(authorityOfWarmKey(p.keyOf(cases[4]))?.slug);
    // Each key's next job goes to its own process, never another key's.
    for (const [i, c] of cases.entries()) expect((await env(c)).pid).toBe(first[i]!.pid);
    const warm = p.t.engine.workers().warm.filter((w) => w.family === "plugin");
    expect(warm.map((w) => w.key).sort()).toEqual(cases.map((c) => p.keyOf(c)).sort());
    // Each process's temp is its own key's, as the engine tagged it.
    for (const [i, c] of cases.entries()) expect(first[i]!.env.TMPDIR).toBe(keyTmpDir(p.serverData, p.keyOf(c)));
    // One caller across a generation bump: a new key, process and temp (retiring the old
    // process and removing its temp is the host's, `plugins-authority.test.ts`).
    p.gens.bump(`agent:${tree.root}`);
    const after = await env(cases[4]!);
    expect(after.pid).not.toBe(first[4]!.pid);
    expect(after.env.TMPDIR).not.toBe(first[4]!.env.TMPDIR);
    expect(authorityOfWarmKey(p.keyOf(cases[4]))?.generation).toBe(1);
    // The person class is untouched by an agent tree's bump.
    expect((await env(cases[0]!)).pid).toBe(first[0]!.pid);
  });

  test("update_uses_a_new_process: the version is in the key", async () => {
    const p = await pluginEngine();
    p.install("1.0.0");
    const old = (await p.echo({ op: "pid" })).pid;
    p.install("1.1.0");
    const next = (await p.echo({ op: "pid" }, { version: "1.1.0" })).pid;
    expect(next).not.toBe(old);
    // A job of the old version never starts the new code (a cold start of it is refused; the
    // old version's warm process drains and stops in Task 5).
    const stale = await p.submit({ op: "pid" }, { priority: "normal" });
    expect(stale.error).toMatchObject({ code: "PLUGIN_NOT_FOUND", details: { version: "1.0.0", installed: "1.1.0" } });
  });

  test("worker_exit_names_the_jobs_plugin (cold and warm)", async () => {
    const p = await pluginEngine();
    p.install();
    await p.echo({ op: "pid" }, { priority: "normal" });
    await p.echo({ op: "pid" });
    await p.t.close();
    const exits = p.t.sink.exits.filter((x) => x.family === "plugin");
    expect(exits.length).toBe(2);
    for (const x of exits) expect(x.pluginId).toBe(ID);
  });

  test("no_rehash_before_cold_start: a store file changed after install still starts", async () => {
    const p = await pluginEngine();
    const dir = p.install();
    appendFileSync(join(dir, "main.mjs"), "\n// changed after install\n");
    writeFileSync(join(dir, "extra.txt"), "planted");
    expect((await p.submit({ op: "pid" }, { priority: "normal" })).state).toBe("succeeded");
  });

  test("errors: a crash is PLUGIN_CRASHED, a hang PLUGIN_TIMEOUT, a hello without the kind PLUGIN_PROTOCOL", async () => {
    const p = await pluginEngine();
    p.install();
    expect((await p.submit({ op: "crash" }, { priority: "normal" })).error).toMatchObject({ code: "PLUGIN_CRASHED", details: { cause: "WORKER_CRASHED" } });
    expect((await p.submit({ op: "hang" }, { priority: "normal" })).error).toMatchObject({ code: "PLUGIN_TIMEOUT", details: { cause: "WORKER_HUNG" } });
    p.install("2.0.0", { "kinds.json": JSON.stringify([{ kind: "test.other", version: 1 }]) });
    for (const priority of ["normal", "interactive"] as const) {
      expect((await p.submit({ op: "pid" }, { version: "2.0.0", priority })).error).toMatchObject({ code: "PLUGIN_PROTOCOL", details: { cause: "WORKER_PROTOCOL_MISMATCH" } });
    }
  });

  test("usable_checked_at_warm_dispatch: a plugin revoked while its process idles serves no more jobs", async () => {
    const p = await pluginEngine();
    p.install();
    const pid = (await p.echo({ op: "pid" })).pid;
    expect(p.t.engine.workers().warm.find((w) => w.family === "plugin")).toMatchObject({ state: "ready", served: 1 });
    p.tables.setState(ID, true, "revoked", 2);
    const refused = await p.submit({ op: "pid" });
    expect(refused.error?.code).toBe("PLUGIN_REVOKED");
    // The job never reached the process, which is retired.
    const exits = async () => p.t.sink.exits.filter((x) => x.family === "plugin");
    for (let i = 0; i < 100 && (await exits()).length === 0; i++) await new Promise((r) => setTimeout(r, 50));
    expect(await exits()).toMatchObject([{ jobsServed: 1, pluginId: ID }]);
    expect(p.t.engine.workers().warm.filter((w) => w.family === "plugin")).toEqual([]);
    expect(pid).toBeGreaterThan(0);
  });

  test("a disabled or revoked plugin starts no process", async () => {
    const p = await pluginEngine();
    p.install();
    p.tables.setState(ID, false, "disabled", 2);
    expect((await p.submit({ op: "pid" }, { priority: "normal" })).error?.code).toBe("PLUGIN_DISABLED");
    p.tables.setState(ID, true, "revoked", 3);
    expect((await p.submit({ op: "pid" })).error?.code).toBe("PLUGIN_REVOKED");
    expect(p.t.sink.exits.filter((x) => x.family === "plugin")).toEqual([]);
  });

  test("a plugin job names its plugin and carries its owner's rights (Task 6)", async () => {
    const p = await pluginEngine();
    p.install();
    const via = { plugin: `${ID}@1.0.0` };
    const rights = rightsFor({ invoker: { ...PERSON, scopes: [], projects: "*" }, grants: [], class: "person" });
    const refused = (spec: Record<string, unknown>) => Effect.runPromise(Effect.flip(p.t.engine.submit(PERSON, { kind: "test.echo", params: { value: { op: "pid" } }, ...spec })));
    expect(await refused({ via })).toMatchObject({ code: "INVALID_REQUEST", details: { field: "rights" } });
    expect(await refused({ rights })).toMatchObject({ code: "INVALID_REQUEST", details: { field: "via" } });
    expect(await refused({ via, rights: { ...rights, scopes: ["project.admin"] } })).toMatchObject({ details: { field: "rights" } });
    expect(await refused({ via, rights: { ...rights, extra: 1 } })).toMatchObject({ details: { field: "rights" } });
    // Rights computed for another principal are not this owner's.
    expect(await refused({ via, rights: { ...rights, principal: { kind: "agent", id: "S" } } })).toMatchObject({ details: { field: "rights" } });
    expect(p.t.sink.exits.filter((x) => x.family === "plugin")).toEqual([]);
    // Rights for another principal class are refused (a person's job cannot take an agent tree's
    // process, nor an MCP credential's job the person's), and the key never reads the stored class.
    expect(await refused({ via, rights: { ...rights, class: "agent:R" } })).toMatchObject({ code: "INVALID_REQUEST", details: { field: "rights", class: "agent:R" } });
    const mcp: JobOwner = { kind: "mcp", id: "M" };
    const asPerson = rightsFor({ invoker: { ...mcp, scopes: [], projects: "*" }, grants: [], class: "person" });
    expect(await Effect.runPromise(Effect.flip(p.t.engine.submit(mcp, { kind: "test.echo", params: { value: { op: "pid" } }, via, rights: asPerson })))).toMatchObject({ details: { class: "person" } });
    expect(pluginWarmKey({ ...job(PERSON, "P"), rights: { ...rights, class: "agent:R" } } as JobView)).toBe(`${ID}@1.0.0|P|${authOf(PERSON)}`);
    expect(authOf(PERSON)).toMatch(/^person\.[0-9a-f]{32}\.0$/);
    // Rights whose hash is not theirs, or whose generation has not happened, are refused.
    expect(await refused({ via, rights: { ...rights, rightsHash: "0".repeat(32) } })).toMatchObject({ details: { field: "rights", reason: "rightsHash" } });
    expect(await refused({ via, rights: { ...rights, generation: 7 } })).toMatchObject({ details: { field: "rights", reason: "generation" } });
    expect(await refused({ via, rights: { ...rights, generation: -1 } })).toMatchObject({ details: { field: "rights" } });
    // An agent's rights without its branch reach are not rights.
    const agent: JobOwner = { kind: "agent", id: "S" };
    const noBranch = { ...rightsFor({ invoker: { ...agent, scopes: [], projects: "*" }, grants: [], class: "agent:S" }) };
    expect(await Effect.runPromise(Effect.flip(p.t.engine.submit(agent, { kind: "test.echo", params: { value: { op: "pid" } }, via, rights: noBranch })))).toMatchObject({ details: { field: "rights" } });
    // A `net` grant without origins, or an unknown permission, is refused too.
    expect(await refused({ via, rights: { ...rights, grants: [{ perm: "net" }] } })).toMatchObject({ details: { field: "rights" } });
    expect(await refused({ via, rights: { ...rights, grants: [{ perm: "root" }] } })).toMatchObject({ details: { field: "rights" } });
  });
});

describe("the plugin family's configuration", () => {
  const row = (over: Partial<InstalledFull> = {}): InstalledFull =>
    ({ id: ID, version: "1.0.0", source: "registry", kind: "code", enabled: true, state: "active", autoUpdate: false, installedBy: "console:console", installedAt: 0, treeHash: "t", dir: "store/@test/probe/1.0.0-t", ...over }) as InstalledFull;
  const deps = (serverData: string, over: Partial<PluginFamilyDeps> = {}): PluginFamilyDeps => ({ serverData, installed: () => row(), ...over });

  test("the SpawnRequest's grants fit 7C's plugin_input (the shape crates/fin-sandbox grants.rs pins)", () => {
    const sd = tempData();
    placeProbe(sd, "1.0.0");
    const program = pluginProgram(deps(sd), job(PERSON, "p1"));
    expect("path" in program && program.path).toBe(process.execPath);
    const own = "path" in program ? program.process : undefined;
    const tmp = keyTmpDir(sd, pluginWarmKey(job(PERSON, "p1")));
    const store = storeDirOf(sd, row());
    expect(own).toEqual({
      env: expect.objectContaining({ TMPDIR: tmp, FIN_PLUGIN_PROTOCOL: "1" }),
      cwd: tmp,
      grants: [
        { path: tmp, mode: "readWrite" },
        { path: store, mode: "read" },
        { path: process.execPath, mode: "read" },
      ],
      limits: { processes: 8, openFiles: 1024 },
    });
    // 7C's rules: `cwd` one directory of server data, not the jobs root; exactly one read
    // grant in `plugins/store/`; reads elsewhere outside server data; no data directory yet.
    const inData = (p: string) => p.startsWith(`${sd}/`) || p.startsWith(`${sd}\\`);
    expect(inData(tmp) && !tmp.startsWith(join(sd, "jobs"))).toBe(true);
    expect(own!.grants.filter((g) => g.mode === "read" && g.path.startsWith(join(sd, "plugins", "store")))).toHaveLength(1);
    expect(own!.grants.filter((g) => g.mode === "read" && inData(g.path) && !g.path.startsWith(join(sd, "plugins", "store")))).toEqual([]);
    expect(own!.grants.filter((g) => g.mode === "readWrite")).toEqual([{ path: tmp, mode: "readWrite" }]);
  });

  test("native runs the platform's executable with no arguments", () => {
    const sd = tempData();
    placeProbe(sd, "1.0.0");
    const dir = storeDirOf(sd, row());
    writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ id: ID, version: "1.0.0", kind: "code", runtime: { type: "native", exec: { "linux-x64": "bin/tool", "win32-x64": "bin/tool.exe" } } }));
    const p = pluginProgram(deps(sd, { platform: "linux-x64", versions: { electron: "33.0.0" } }), job(PERSON));
    expect(p).toMatchObject({ path: join(dir, "bin", "tool"), args: [] });
    expect(("path" in p ? p.process : undefined)?.env.ELECTRON_RUN_AS_NODE).toBeUndefined();
    expect(() => pluginProgram(deps(sd, { platform: "darwin-arm64" }), job(PERSON))).toThrow(/darwin-arm64/);
    writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ id: ID, version: "1.0.0", kind: "code", runtime: { type: "native", exec: { "linux-x64": "../escape" } } }));
    expect(() => pluginProgram(deps(sd, { platform: "linux-x64" }), job(PERSON))).toThrow(/outside its package/);
  });

  test("electron_run_as_node_only_under_electron", () => {
    const sd = tempData();
    placeProbe(sd, "1.0.0");
    const saved = process.env.ELECTRON_RUN_AS_NODE;
    process.env.ELECTRON_RUN_AS_NODE = "1";
    try {
      const envOf = (versions: Record<string, string | undefined>) => {
        const p = pluginProgram(deps(sd, { versions }), job(PERSON));
        return "path" in p ? p.process?.env : undefined;
      };
      expect(envOf({ node: "24.22.2", electron: "33.0.0" })?.ELECTRON_RUN_AS_NODE).toBe("1");
      // finisherd's own variable is never passed through.
      expect(envOf({ node: "24.22.2" })?.ELECTRON_RUN_AS_NODE).toBeUndefined();
    } finally {
      if (saved === undefined) delete process.env.ELECTRON_RUN_AS_NODE;
      else process.env.ELECTRON_RUN_AS_NODE = saved;
    }
  });

  test("warm keys: plugin@version, project or none, authority key", () => {
    const person = authOf(PERSON);
    expect(pluginWarmKey(job(PERSON, "p1"))).toBe(`${ID}@1.0.0|p1|${person}`);
    expect(pluginWarmKey(job({ kind: "console", id: "console" }))).toBe(`${ID}@1.0.0|none|${person}`);
    expect(pluginWarmKey(job({ kind: "device", id: "d1" }))).toBe(`${ID}@1.0.0|none|${person}`);
    expect(authorityOfWarmKey(pluginWarmKey(job(SYSTEM)))).toMatchObject({ slug: "system", generation: 0 });
    expect(pluginWarmKey(job(PERSON, "p1", `${ID}@1.1.0`))).toBe(`${ID}@1.1.0|p1|${person}`);
    // A job without valid rights shares nothing: its key is its own.
    expect(pluginWarmKey({ ...job(PERSON, "p1"), rights: undefined } as unknown as JobView)).toBe(`${ID}@1.0.0|p1|job-j1`);
    // An agent session tree shares its root's class; MCP is a class of its own.
    const roots = (id: string) => (id === "child" ? "root" : undefined);
    expect(principalClass({ kind: "agent", id: "child" }, roots)).toBe("agent:root");
    expect(principalClass({ kind: "agent", id: "solo" }, roots)).toBe("agent:solo");
    expect(principalClass({ kind: "mcp", id: "m1" })).toBe("mcp:m1");
    expect(() => pluginWarmKey({ id: "j", owner: PERSON } as unknown as JobView)).toThrow(/names no plugin/);
  });

  test("sessionRoots reads an agent session tree from a migrated control.db", async () => {
    const sd = tempData();
    const control = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
    try {
      const tree = await agentTree(control, sd);
      const roots = sessionRoots(control);
      expect(roots(tree.child)).toBe(tree.root);
      expect(roots(tree.root)).toBe(tree.root);
      expect(roots("01nosuchsession00000000000")).toBeUndefined();
      expect(principalClass({ kind: "agent", id: tree.child }, roots)).toBe(`agent:${tree.root}`);
    } finally {
      await control.close();
    }
  });

  test("a warm spawn's temp is its tagged key's, without a confined pool's `<project>/` prefix", () => {
    const sd = tempData();
    placeProbe(sd, "1.0.0");
    const p = pluginProgram(deps(sd), job(PERSON, "p1"), "p1/tagged-key");
    expect("path" in p && p.process?.env.TMPDIR).toBe(keyTmpDir(sd, "tagged-key"));
    const q = pluginProgram(deps(sd), job(PERSON, "p1"), "@x/y@1.0.0|p1|person.k.0");
    expect("path" in q && q.process?.env.TMPDIR).toBe(keyTmpDir(sd, "@x/y@1.0.0|p1|person.k.0"));
  });

  test("slots_up_to_16_for_plugin_family; the overrun stays at 2 per family", () => {
    const r = makeKindRegistry();
    const sd = tempData();
    r.registerFamily({ ...pluginFamily(deps(sd)), family: "plugin16", warm: { slots: MAX_PLUGIN_WARM_SLOTS } });
    expect(() => r.registerFamily({ ...pluginFamily(deps(sd)), family: "plugin17", warm: { slots: 17 } })).toThrow(/1 to 16/);
    expect(() => r.registerFamily({ family: "worker3", profile: "worker", program: { binary: "fin-workers", args: [] }, warm: { slots: 3 } })).toThrow(/1 or 2/);
    expect(pluginFamily(deps(sd)).warm).toEqual({ idleS: 300, slots: 16 });
    expect(pluginFamily(deps(sd)).memory).toEqual({ interactive: { fixedMiB: 2048 }, batch: { fixedMiB: 2048 } });
    expect(warmOverrun(new Map([["plugin", { slots: 16, workers: [] }], ["media", { slots: 2, workers: [] }]]))).toEqual({ cpu: 4 });
  });

  test("env_refused_for_plugin_family and rpc_refused_for_plugin_kind still hold", () => {
    const r = makeKindRegistry();
    const sd = tempData();
    expect(() => r.registerFamily({ ...pluginFamily(deps(sd)), env: ["FIN_GPU"] })).toThrow(/finisherd's environment/);
    r.registerFamily(pluginFamily(deps(sd)));
    expect(() => r.registerKind({ ...ECHO, rpc: true, paramsReach: () => [] })).toThrow(/cannot be submitted by RPC/);
  });

  test("worker.exit's plugin: the family's, else the job's via without its version", () => {
    expect(exitPluginId({}, { plugin: `${ID}@1.2.3` })).toBe(ID);
    expect(exitPluginId({ pluginId: "test.plugin" }, { plugin: `${ID}@1.2.3` })).toBe("test.plugin");
    expect(exitPluginId({}, undefined)).toBeUndefined();
  });

  test("error mapping keeps WORKER_OOM; a broken envelope is PLUGIN_PROTOCOL", () => {
    const e = (code: string, details: unknown = {}): FinErrorData => ({ code, message: "m", details });
    expect(mapPluginError(e("WORKER_OOM"))).toEqual(e("WORKER_OOM"));
    expect(mapPluginError(e("WORKER_CRASHED", { protocol: "json" })).code).toBe("PLUGIN_PROTOCOL");
    expect(mapPluginError(e("WORKER_CRASHED", { exitCode: 3 }))).toEqual({ code: "PLUGIN_CRASHED", message: "m", details: { exitCode: 3, cause: "WORKER_CRASHED" } });
    expect(mapPluginError(e("JOB_TIMEOUT")).code).toBe("PLUGIN_TIMEOUT");
    expect(mapPluginError(e("JOB_STALLED")).code).toBe("JOB_STALLED");
  });

  test("unusable rows: disabled, revoked, crash loop, content", () => {
    expect(unusable(row())).toBeUndefined();
    expect(unusable(row({ enabled: false }))?.code).toBe("PLUGIN_DISABLED");
    expect(unusable(row({ state: "revoked" }))?.code).toBe("PLUGIN_REVOKED");
    expect(unusable(row({ state: "crashLoop" }))?.code).toBe("PLUGIN_CRASH_LOOP");
    expect(unusable(row({ kind: "content" }))?.code).toBe("PLUGIN_RUNTIME_UNAVAILABLE");
  });

  test("the keys' temp directories are swept at start", () => {
    const sd = tempData();
    const left = join(sd, "plugins", "tmp", "0000", "left");
    mkdirSync(dirname(left), { recursive: true });
    writeFileSync(left, "x");
    registerPluginFamily(makeKindRegistry(), deps(sd));
    expect(readdirSync(join(sd, "plugins", "tmp"))).toEqual([]);
  });
});
