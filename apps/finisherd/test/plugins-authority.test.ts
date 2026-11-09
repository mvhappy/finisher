// Authority-keyed plugin processes, temp and data (sp8 spec §3.3, §5.1, §12, Decision 28; sp8b
// plan Decisions 4 and 5 as amended 2026-11-09): the rights hash and its uniform-class rule,
// the policy generations and what a bump retires (processes, temp, authority data), and the
// `pluginData` directory per authority. The engine runs the test-only probe
// (`fixtures/plugins/probe`) under the real plugin host (`makePluginHost`); every process here
// is one this test's engine started.
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { JobOwner, JobView } from "@finisher/contracts";
import { SCOPES } from "@finisher/contracts";
import type { Permission } from "@finisher/core-types";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { Credentials } from "../src/auth/credentials.ts";
import type { DeviceEvent } from "../src/auth/devices.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, ControlStoreTag, openControlStore } from "../src/control/db.ts";
import type { JobEngine } from "../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../src/jobs/events.ts";
import type { KindSpec } from "../src/jobs/kinds.ts";
import { buildJobEngine } from "../src/jobs/layer.ts";
import { authorityOfWarmKey, classSlug, memoryGenerations, PLUGIN_AUTHDATA, PLUGIN_DATA, parseAuthorityKey, storedGenerations, sweepAuthData, uniformClassRights } from "../src/plugins/host/authority.ts";
import { keyTmpDir, principalClass, sessionRoots } from "../src/plugins/host/keys.ts";
import { makePluginHost, type PluginHost } from "../src/plugins/host/processes.ts";
import { rightsFor, rightsHash, systemInvoker } from "../src/plugins/host/rights.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { run } from "./support/jobs.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ID = "@test/probe";
const PERSON: JobOwner = { kind: "local", id: "local" };
const ECHO: KindSpec = { kind: "test.echo", version: 1, family: "plugin", scopes: [], resources: [], warm: true, defaultPriority: "interactive", qos: "normal", timeoutS: 60, maxAttempts: 1 };
const DATA_RW: Permission = { perm: "fs", scope: "pluginData", access: "readWrite" };
const ALL_GRANTS: Permission[] = [{ perm: "project:read" }, { perm: "project:write" }, { perm: "media:read" }, { perm: "jobs:submit" }, DATA_RW];

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

interface Rig {
  readonly serverData: string;
  readonly control: ControlStore;
  readonly engine: JobEngine;
  readonly host: PluginHost;
  readonly creds: Credentials;
  /** A device source the test revokes through. */
  revokeDevice(id: string): void;
  /** The job a submit of `o` would make (rights computed now, as the host does). */
  jobOf(o: RunOpts): JobView;
  echo(op: Record<string, unknown>, o: RunOpts): Promise<Record<string, unknown>>;
}

type RunOpts = { owner: JobOwner; project?: string; sessionRoot?: string; scopes?: ReadonlyArray<string>; grants?: ReadonlyArray<Permission> };

async function rig(): Promise<Rig> {
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-auth-")));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const tables = new PluginTables(control);
  const host = makePluginHost({ serverData, control, tables });
  const engine = await Effect.runPromise(
    buildJobEngine(serverData, {
      testKinds: false,
      cgroups: "off",
      cores: 4,
      timing: { killWaitMs: 2000, exitDrainMs: 100, hungMs: 1500 },
      register: (r) => {
        host.register(r);
        r.registerKind(ECHO);
      },
    }).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, memorySink())))),
  );
  host.bind(engine);
  const creds = new Credentials(control, Buffer.alloc(32, 7), new Audit(control));
  const deviceListeners = new Set<(e: DeviceEvent) => void>();
  const unwatch = host.watch({ credentials: creds, devices: { onEvent: (f) => (deviceListeners.add(f), () => void deviceListeners.delete(f)) } });
  cleanups.push(async () => {
    unwatch();
    creds.dispose();
    await run(engine.close());
    await control.close();
    rmSync(serverData, { recursive: true, force: true });
  });
  // The probe, installed by hand.
  const dir = `store/@test/probe/1.0.0-t`;
  const full = join(serverData, "plugins", ...dir.split("/"));
  mkdirSync(dirname(full), { recursive: true });
  cpSync(FIXTURE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, version: "1.0.0" }));
  tables.addPlugin({ id: ID, version: "1.0.0", source: "registry", treeHash: "t", dir, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
  const roots = sessionRoots(control);
  const jobOf = (o: RunOpts): JobView => {
    const cls = principalClass(o.owner, roots);
    const rights = rightsFor({
      invoker: { ...o.owner, scopes: (o.scopes ?? SCOPES) as never, projects: "*" },
      grants: o.grants ?? ALL_GRANTS,
      class: cls,
      sessionRoot: o.owner.kind === "agent" ? (o.sessionRoot ?? "br1") : undefined,
      generation: host.generations.of(cls),
    });
    return { id: "k", owner: o.owner, via: { plugin: `${ID}@1.0.0` }, rights, ...(o.project === undefined ? {} : { project: o.project }) } as unknown as JobView;
  };
  return {
    serverData,
    control,
    engine,
    host,
    creds,
    revokeDevice: (id) => {
      for (const f of deviceListeners) f({ kind: "revoked", id });
    },
    jobOf,
    async echo(op, o) {
      const j = jobOf(o);
      const v = await run(engine.submit(o.owner, { kind: "test.echo", params: { value: op }, via: j.via!, rights: j.rights!, ...(o.project === undefined ? {} : { project: o.project }) }));
      const [done] = await run(engine.wait([v.id], { timeoutMs: 20_000 }));
      expect(done?.state, JSON.stringify(done?.error)).toBe("succeeded");
      return done!.result as Record<string, unknown>;
    },
  };
}

/** A root agent session and its child in `control`; their ids (7A-2's session tree). */
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

const plugin = (r: Rig) => r.engine.workers().warm.filter((w) => w.family === "plugin");

async function until(f: () => boolean, ms = 15_000): Promise<void> {
  for (let waited = 0; !f(); waited += 25) {
    if (waited > ms) throw new Error("timed out");
    await new Promise((x) => setTimeout(x, 25));
  }
}

const processGone = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return false;
  } catch {
    return true;
  }
};

describe("authority keys", () => {
  test("person_class_rights_are_uniform: console, local and devices share one rights hash (the shared-data assumption)", () => {
    // spec §3.3: `person` and `system` share a key and the per-project `pluginData` only while
    // every member holds identical plugin-relevant rights. If this breaks, the class must move
    // to authority directories (`uniformClassRights` then sends the narrower member there).
    const device = { kind: "device", id: "d1", scopes: [...SCOPES], projects: "*" as const };
    for (const grants of [[], ALL_GRANTS, [{ perm: "project:read" } as Permission, { perm: "net", origins: ["https://a.example"] } as Permission]]) {
      const hashes = [CONSOLE, LOCAL, device].map((invoker) => rightsFor({ invoker, grants, class: "person" }));
      expect(new Set(hashes.map((h) => h.rightsHash)).size).toBe(1);
      for (const h of hashes) expect(uniformClassRights(h)).toBe(true);
      expect(uniformClassRights(rightsFor({ invoker: systemInvoker("library"), grants, class: "system" }))).toBe(true);
    }
    // A person narrower than the class (fewer scopes, fewer projects) is not uniform.
    expect(uniformClassRights(rightsFor({ invoker: { ...LOCAL, scopes: ["project.read"] }, grants: ALL_GRANTS, class: "person" }))).toBe(false);
    expect(uniformClassRights(rightsFor({ invoker: { ...LOCAL, projects: ["p1"] }, grants: ALL_GRANTS, class: "person" }))).toBe(false);
    // No other class is ever uniform.
    expect(uniformClassRights(rightsFor({ invoker: { kind: "mcp", id: "m", scopes: [...SCOPES], projects: "*" }, grants: ALL_GRANTS, class: "mcp:m" }))).toBe(false);
  });

  test("rights_hash: order-insensitive, principal-free, every plugin-relevant field counts", () => {
    const base = rightsFor({ invoker: { kind: "mcp", id: "m", scopes: ["project.read", "library.read"], projects: ["p2", "p1"] }, grants: [{ perm: "media:read" }, { perm: "project:read" }], class: "mcp:m" });
    const reordered = rightsFor({ invoker: { kind: "mcp", id: "m", scopes: ["library.read", "project.read"], projects: ["p1", "p2"] }, grants: [{ perm: "project:read" }, { perm: "media:read" }], class: "mcp:m" });
    expect(reordered.rightsHash).toBe(base.rightsHash);
    expect(rightsHash({ ...base, principal: { kind: "mcp", id: "other" } } as never)).toBe(base.rightsHash);
    for (const changed of [{ class: "mcp:n" }, { scopes: ["project.read"] }, { projects: ["p1"] }, { libraries: [] }, { branch: { root: "b", subtree: true } }, { branch: { root: "b", subtree: false } }, { grants: [{ perm: "project:read" }] }]) {
      expect(rightsHash({ ...base, ...changed } as never), JSON.stringify(changed)).not.toBe(base.rightsHash);
    }
    expect(classSlug("agent:01ABC")).toBe("agent-01ABC");
    expect(classSlug("odd/class")).toMatch(/^x[0-9a-f]{16}$/);
    expect(parseAuthorityKey(`agent-01ABC.${base.rightsHash}.3`)).toEqual({ slug: "agent-01ABC", hash: base.rightsHash, generation: 3 });
    expect(parseAuthorityKey("../x")).toBeUndefined();
  });

  test("generations persist in control.db; the start sweep removes authority data of any other generation", async () => {
    const sd = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-gen-")));
    cleanups.push(() => rmSync(sd, { recursive: true, force: true }));
    let control = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
    const gens = storedGenerations(control);
    expect(gens.of("agent:R")).toBe(0);
    expect([gens.bump("agent:R"), gens.bump("agent:R")]).toEqual([1, 2]);
    await control.close();
    control = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
    cleanups.push(() => control.close());
    const again = storedGenerations(control);
    expect(again.of("agent:R")).toBe(2);
    const h = "a".repeat(32);
    const at = (key: string) => join(sd, ...PLUGIN_AUTHDATA, "@test", "probe", "p1", key);
    for (const k of [`agent-R.${h}.1`, `agent-R.${h}.2`, `mcp-M.${h}.0`, `mcp-M.${h}.4`, "junk"]) mkdirSync(at(k), { recursive: true });
    expect(sweepAuthData(sd, again)).toBe(3);
    expect([`agent-R.${h}.2`, `mcp-M.${h}.0`].every((k) => existsSync(at(k)))).toBe(true);
    // Memory generations (families built without a store) start at 0 too.
    expect(memoryGenerations().of("person")).toBe(0);
  });
});

describe("authority-keyed processes", () => {
  test("two agent sessions, a child narrower than its root: separate processes; person shares one", async () => {
    const r = await rig();
    const t1 = await agentTree(r.control, r.serverData);
    const t2 = await agentTree(r.control, r.serverData);
    const pid = async (o: RunOpts) => (await r.echo({ op: "pid" }, o)).pid as number;
    const a = await pid({ owner: { kind: "agent", id: t1.root }, project: "p1" });
    const b = await pid({ owner: { kind: "agent", id: t2.root }, project: "p1" });
    // The child holds fewer scopes and its own root branch: never its root's process.
    const child = await pid({ owner: { kind: "agent", id: t1.child }, project: "p1", sessionRoot: "br2", scopes: ["project.read"] });
    expect(new Set([a, b, child]).size).toBe(3);
    // The person's principals share one process.
    const local = await pid({ owner: PERSON, project: "p1" });
    expect(await pid({ owner: { kind: "console", id: "console" }, project: "p1" })).toBe(local);
    expect(await pid({ owner: { kind: "device", id: "d1" }, project: "p1" })).toBe(local);
    expect(await pid({ owner: { kind: "agent", id: t1.root }, project: "p1" })).toBe(a);
  });

  test("revoking a credential bumps its generation: the old process is retired, its temp and authority data removed", async () => {
    const r = await rig();
    const { credential } = r.creds.mint(CONSOLE, { kind: "mcp", label: "m", scopes: ["project.read"], projects: ["p1"] });
    const owner: JobOwner = { kind: "mcp", id: credential.id };
    const cls = `mcp:${credential.id}`;
    // Minting it bumped its class once already.
    expect(r.host.generations.of(cls)).toBe(1);
    const o: RunOpts = { owner, project: "p1" };
    const key = r.host.warmKeyOf(r.jobOf(o));
    const before = (await r.echo({ op: "env" }, o)) as { pid: number; env: Record<string, string> };
    expect(before.env.TMPDIR).toBe(keyTmpDir(r.serverData, key));
    const data = r.host.dataDirOf(r.jobOf(o))!;
    expect(data.path).toBe(join(r.serverData, ...PLUGIN_AUTHDATA, "@test", "probe", "p1", key.split("|")[2]!));
    expect(authorityOfWarmKey(key)).toMatchObject({ slug: `mcp-${credential.id}`, generation: 1 });
    expect(existsSync(data.path)).toBe(true);
    r.creds.markEnded({ kind: "mcp", id: credential.id }, "owner");
    expect(r.host.generations.of(cls)).toBe(2);
    await until(() => plugin(r).length === 0 && !existsSync(keyTmpDir(r.serverData, key)) && !existsSync(data.path));
    await until(() => processGone(before.pid));
    // Its next key is another (the generation is in it); a job under it never meets the old process.
    expect(r.host.warmKeyOf(r.jobOf(o))).not.toBe(key);
  });

  test("a child session starting bumps its tree's generation; a device revoked bumps person", async () => {
    const r = await rig();
    const tree = await agentTree(r.control, r.serverData);
    const root: RunOpts = { owner: { kind: "agent", id: tree.root }, project: "p1" };
    const rootKey = r.host.warmKeyOf(r.jobOf(root));
    const rootPid = (await r.echo({ op: "pid" }, root)).pid as number;
    // The child's credential (minted with its session's id) belongs to the root's class.
    r.creds.mintWithId({ kind: "agent", id: tree.root, scopes: [], projects: ["p1"], branches: ["br1"], human: false }, tree.child, { kind: "agent", label: "child", scopes: ["project.read"], projects: ["p1"], branches: ["br2"] });
    expect(r.host.generations.of(`agent:${tree.root}`)).toBe(1);
    await until(() => !plugin(r).some((w) => w.key === rootKey) && !existsSync(keyTmpDir(r.serverData, rootKey)));
    expect((await r.echo({ op: "pid" }, root)).pid).not.toBe(rootPid);
    // person: one device revoked retires the class's processes too.
    const person: RunOpts = { owner: PERSON, project: "p1" };
    const personKey = r.host.warmKeyOf(r.jobOf(person));
    const personPid = (await r.echo({ op: "pid" }, person)).pid as number;
    r.revokeDevice("d1");
    await until(() => !plugin(r).some((w) => w.key === personKey));
    expect(r.host.generations.of("person")).toBe(1);
    expect((await r.echo({ op: "pid" }, person)).pid).not.toBe(personPid);
  });
});

describe("pluginData per authority", () => {
  test("shared per project for person and system, per authority for agents, removed when the session ends", async () => {
    const r = await rig();
    const tree = await agentTree(r.control, r.serverData);
    // The root session's credential (its start bumps the tree's generation once).
    r.creds.mintWithId(CONSOLE, tree.root, { kind: "agent", label: "root", scopes: ["project.read"], projects: ["p1"], branches: ["br1"] });
    await until(() => r.host.generations.of(`agent:${tree.root}`) === 1);
    const shared = join(r.serverData, ...PLUGIN_DATA, "@test", "probe", "p1");
    expect(r.host.dataDirOf(r.jobOf({ owner: PERSON, project: "p1" }))?.path).toBe(shared);
    expect(r.host.dataDirOf(r.jobOf({ owner: { kind: "device", id: "d1" }, project: "p1" }))?.path).toBe(shared);
    expect(r.host.dataDirOf(r.jobOf({ owner: { kind: "system", id: "lib" }, project: "p1" }))?.path).toBe(shared);
    expect(r.host.dataDirOf(r.jobOf({ owner: PERSON }))?.path).toBe(join(r.serverData, ...PLUGIN_DATA, "@test", "probe", "_"));
    // A person narrower than the class never gets the shared directory.
    expect(r.host.dataDirOf(r.jobOf({ owner: PERSON, project: "p1", scopes: ["project.read"] }))?.path).not.toBe(shared);
    // An agent root and its child: each its own authority directory.
    const rootJob = r.jobOf({ owner: { kind: "agent", id: tree.root }, project: "p1" });
    const childJob = r.jobOf({ owner: { kind: "agent", id: tree.child }, project: "p1", sessionRoot: "br2" });
    const rootDir = r.host.dataDirOf(rootJob)!.path;
    const childDir = r.host.dataDirOf(childJob)!.path;
    expect(rootDir).toBe(join(r.serverData, ...PLUGIN_AUTHDATA, "@test", "probe", "p1", r.host.authorityOf(rootJob)!.key));
    expect(childDir).not.toBe(rootDir);
    // The process can use its own directory (Node's flags name it) and nothing of the shared one.
    const probe = await r.echo({ op: "probe", read: [], write: [join(rootDir, "note"), join(shared, "planted")] }, { owner: { kind: "agent", id: tree.root }, project: "p1" });
    expect(probe).toMatchObject({ [`write ${join(rootDir, "note")}`]: "ok", [`write ${join(shared, "planted")}`]: "ERR_ACCESS_DENIED" });
    // The session ends (its credential is ended): the tree's authority data is removed; the
    // person's shared directory stays.
    r.creds.markEnded({ kind: "agent", id: tree.root }, "owner");
    await until(() => !existsSync(rootDir) && !existsSync(childDir));
    expect(existsSync(shared)).toBe(true);
  });

  test("no directory without an fs pluginData grant", async () => {
    const r = await rig();
    const read = r.jobOf({ owner: PERSON, project: "p2", grants: [{ perm: "fs", scope: "pluginData", access: "read" }] });
    expect(r.host.dataDirOf(read)).toMatchObject({ write: false });
    const none = r.jobOf({ owner: { kind: "mcp", id: "m" }, project: "p3", grants: [{ perm: "project:read" }] });
    expect(r.host.dataDirOf(none)).toBeUndefined();
    expect(existsSync(join(r.serverData, ...PLUGIN_AUTHDATA, "@test", "probe", "p3"))).toBe(false);
    // A process without the grant gets no data flag either.
    const env = (await r.echo({ op: "env" }, { owner: PERSON, project: "p4", grants: [] })) as { execArgv: string[] };
    expect(env.execArgv.some((a) => a.includes(join("plugins", "data")))).toBe(false);
    expect(existsSync(join(r.serverData, ...PLUGIN_DATA, "@test", "probe", "p4"))).toBe(false);
  });
});
