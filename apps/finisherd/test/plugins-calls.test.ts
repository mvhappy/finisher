// Host API 1.1 through `onCall` (sp8b plan Task 7; spec §3.3, §3.6, §11): params decoded by
// the generated types, rights narrowed by the live invoker and checked with reach, the
// authority key taken from the call's own job, snapshots paged from one pinned rev, the
// screenplay view, `log` and stderr scrubbed, rate-limited and never journaled, the reserved
// methods naming their batch, and the `PluginContext` of a job. The engine runs the test-only
// probe (`fixtures/plugins/probe`) under the real plugin host; unit cases call `onCall` directly.
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { JobOwner, JobView } from "@finisher/contracts";
import { SCOPES } from "@finisher/contracts";
import type { Permission } from "@finisher/core-types";
import { Engine } from "@finisher/fin-node";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { type ControlStore, ControlStoreTag, openControlStore } from "../src/control/db.ts";
import type { JobEngine } from "../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../src/jobs/events.ts";
import type { CallCtx, KindSpec } from "../src/jobs/kinds.ts";
import { buildJobEngine } from "../src/jobs/layer.ts";
import { type CallDeps, PAGE_BYTES } from "../src/plugins/host/calls.ts";
import { keyTmpDir, principalClass, sessionRoots } from "../src/plugins/host/keys.ts";
import { LOG_PER_SECOND, makePluginLog, scrubLogText } from "../src/plugins/host/log.ts";
import { makePluginHost, type PluginHost, PLUGIN_HOST_API } from "../src/plugins/host/processes.ts";
import type { Invoker } from "../src/plugins/host/rights.ts";
import { rightsFor } from "../src/plugins/host/rights.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { LOCAL } from "../src/principal.ts";
import { run } from "./support/jobs.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ID = "@test/probe";
const PERSON: JobOwner = { kind: "local", id: "local" };
const COMMAND: KindSpec = { kind: "plugin.command", version: 1, family: "plugin", scopes: [], resources: [], warm: true, defaultPriority: "interactive", qos: "normal", timeoutS: 60, maxAttempts: 1 };
const READ: Permission = { perm: "project:read" };
const DATA_RW: Permission = { perm: "fs", scope: "pluginData", access: "readWrite" };

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

/** A real project's snapshot JSON (fin-node), made once. */
let realProject: unknown;
async function projectJson(): Promise<unknown> {
  if (realProject !== undefined) return realProject;
  const sd = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-proj-")));
  const dir = join(sd, "p");
  const e = await Engine.create(sd, dir, "Calls", "30");
  realProject = JSON.parse(await e.snapshot("main")).project;
  await e.close();
  rmSync(sd, { recursive: true, force: true });
  return realProject;
}

interface World {
  rev: number;
  project: unknown;
  live: (owner: JobOwner) => Invoker | undefined;
  reads: number;
}

interface Rig {
  readonly serverData: string;
  readonly control: ControlStore;
  readonly engine: JobEngine;
  readonly host: PluginHost;
  readonly world: World;
  readonly logs: Array<{ level: string; message: string }>;
  readonly sink: ReturnType<typeof memorySink>;
  jobOf(o?: RunOpts): JobView;
  /** Runs a `plugin.command` job whose `args` is `op`; the job when it ended. */
  command(op: Record<string, unknown>, o?: RunOpts): Promise<JobView>;
}

type RunOpts = { owner?: JobOwner; project?: string; branch?: string; scopes?: ReadonlyArray<string>; projects?: "*" | string[]; grants?: ReadonlyArray<Permission> };

async function rig(): Promise<Rig> {
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-calls-")));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const tables = new PluginTables(control);
  const logs: Array<{ level: string; message: string }> = [];
  const host = makePluginHost({ serverData, control, tables, log: (level, message) => void logs.push({ level, message }) });
  const sink = memorySink();
  const engine = await Effect.runPromise(
    buildJobEngine(serverData, {
      testKinds: false,
      cgroups: "off",
      cores: 4,
      timing: { killWaitMs: 2000, exitDrainMs: 100, hungMs: 1500 },
      register: (r) => {
        host.register(r);
        r.registerKind(COMMAND);
      },
    }).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, sink)))),
  );
  host.bind(engine);
  cleanups.push(async () => {
    await Effect.runPromise(host.close());
    await run(engine.close());
    await control.close();
    rmSync(serverData, { recursive: true, force: true });
  });
  const dir = `store/@test/probe/1.0.0-t`;
  const full = join(serverData, "plugins", ...dir.split("/"));
  mkdirSync(dirname(full), { recursive: true });
  cpSync(FIXTURE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, version: "1.0.0" }));
  tables.addPlugin({ id: ID, version: "1.0.0", source: "registry", treeHash: "t", dir, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
  const world: World = { rev: 7, project: await projectJson(), live: (o) => (o.kind === "local" ? LOCAL : undefined), reads: 0 };
  const deps: CallDeps = {
    live: (o) => world.live(o),
    reach: { subtree: (_p, _root, _project, branch) => Effect.succeed(branch) },
    snapshot: () =>
      Effect.sync(() => {
        world.reads++;
        return { rev: world.rev, project: world.project };
      }),
  };
  host.serve(deps);
  const roots = sessionRoots(control);
  const jobOf = (o: RunOpts = {}): JobView => {
    const owner = o.owner ?? PERSON;
    const cls = principalClass(owner, roots);
    const rights = rightsFor({
      invoker: { ...owner, scopes: (o.scopes ?? SCOPES) as never, projects: o.projects ?? "*" },
      grants: o.grants ?? [READ, DATA_RW],
      class: cls,
      generation: host.generations.of(cls),
    });
    const project = o.project ?? "p1";
    return { id: "k", owner, via: { plugin: `${ID}@1.0.0` }, rights, project, params: { command: "c", args: {}, project, branch: o.branch ?? "main", rev: 1 } } as unknown as JobView;
  };
  return {
    serverData,
    control,
    engine,
    host,
    world,
    logs,
    sink,
    jobOf,
    async command(op, o = {}) {
      const j = jobOf(o);
      const params = { ...(j.params as Record<string, unknown>), args: op };
      const v = await run(engine.submit(j.owner, { kind: "plugin.command", params, via: j.via!, rights: j.rights!, project: j.project! }));
      const [done] = await run(engine.wait([v.id], { timeoutMs: 30_000 }));
      return done!;
    },
  };
}

/** The replies of a `calls` op. */
async function replies(r: Rig, calls: Array<{ method: string; params?: unknown }>, o: RunOpts = {}, pauseMs?: number): Promise<Array<{ value?: Record<string, unknown>; error?: { code: string; details: Record<string, unknown> } }>> {
  const done = await r.command({ op: "calls", calls: calls.map((c) => ({ method: c.method, params: c.params ?? {} })), ...(pauseMs === undefined ? {} : { pauseMs }) }, o);
  expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
  return (done.result as { replies: never }).replies;
}

/** A direct call (no process): `ctx` from `job`, `workerKey` defaulting to the job's key now. */
function direct(r: Rig, job: JobView, method: string, params: unknown, workerKey?: string | null) {
  const ctx: CallCtx = {
    job,
    role: "worker",
    owner: job.owner,
    via: job.via!,
    rights: job.rights as NonNullable<CallCtx["rights"]>,
    ...(workerKey === null ? {} : { workerKey: workerKey ?? r.host.warmKeyOf(job) }),
  };
  return Effect.runPromise(Effect.result(r.host.calls.onCall(ctx, method, params))).then((x) =>
    x._tag === "Failure" ? { _tag: "Left" as const, left: x.failure as { code: string; details: unknown } } : { _tag: "Right" as const, right: x.success },
  );
}

const codeOf = (e: { _tag: string; left?: { code: string } }): string | undefined => (e._tag === "Left" ? e.left?.code : "ok");

describe("host calls", () => {
  test("project_snapshot_and_view_happy_path: canonical JSON pages and the screenplay", async () => {
    const r = await rig();
    const [snap, view] = await replies(r, [{ method: "project.snapshot" }, { method: "project.view", params: { budget: 2000 } }]);
    expect(snap?.value).toMatchObject({ page: 0, pages: 1, rev: 7 });
    expect(JSON.parse(Buffer.from(String(snap?.value?.bytesB64), "base64").toString("utf8"))).toEqual(r.world.project);
    expect(String(view?.value?.text)).toMatch(/timeline/i);
    // Both read the one pinned snapshot.
    expect(r.world.reads).toBe(1);
    expect(r.host.calls.pinnedJobs()).toBe(0);
  });

  test("snapshot_pages_share_rev: a commit between pages does not change them", async () => {
    const r = await rig();
    r.world.project = { big: "x".repeat(2 * PAGE_BYTES + 10) };
    const whole = Buffer.from(JSON.stringify(r.world.project), "utf8");
    const submitted = r.command({ op: "calls", digest: true, pauseMs: 800, calls: [{ method: "project.snapshot", params: {} }, { method: "project.snapshot", params: { page: 1 } }, { method: "project.snapshot", params: { page: 2 } }] });
    for (let i = 0; i < 400 && r.host.calls.pinnedJobs() === 0; i++) await new Promise((x) => setTimeout(x, 25));
    // The branch moves on while the job reads.
    r.world.rev = 8;
    r.world.project = { big: "changed" };
    const done = await submitted;
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    const got = (done.result as { replies: Array<{ value: { sha256: string; length: number; rev: number; pages: number } }> }).replies;
    expect(got.map((x) => x.value.rev)).toEqual([7, 7, 7]);
    expect(got.map((x) => x.value.pages)).toEqual([3, 3, 3]);
    const sha = (b: Buffer) => createHash("sha256").update(b).digest("hex");
    expect(got.map((x) => x.value.sha256)).toEqual([0, 1, 2].map((p) => sha(whole.subarray(p * PAGE_BYTES, (p + 1) * PAGE_BYTES))));
    expect(got.map((x) => x.value.length)).toEqual([PAGE_BYTES, PAGE_BYTES, whole.length - 2 * PAGE_BYTES]);
    expect(r.world.reads).toBe(1);
    // Past the last page.
    const job = r.jobOf();
    expect(codeOf(await direct(r, job, "project.snapshot", { page: 9 }))).toBe("INVALID_REQUEST");
  });

  test("malformed_params_are_protocol_errors: the process keeps serving", async () => {
    const r = await rig();
    const got = await replies(r, [
      { method: "project.snapshot", params: { page: -1 } },
      { method: "project.view", params: { budget: "lots" } },
      { method: "log", params: { level: "loud", msg: "x" } },
      { method: "log", params: null },
      // A plugin cannot name an authority key (or anything else the types do not have).
      { method: "project.snapshot", params: { key: "person.0.0" } },
      { method: "log", params: { level: "info", msg: "still here" } },
    ]);
    expect(got.slice(0, 5).map((x) => x.error?.code)).toEqual(Array(5).fill("PLUGIN_PROTOCOL"));
    expect(got[0]?.error?.details).toMatchObject({ method: "project.snapshot" });
    expect(got[5]?.value).toEqual({});
  });

  test("reserved_methods_name_their_batch; unknown methods are refused", async () => {
    const r = await rig();
    const job = r.jobOf();
    const want: Record<string, string> = { "analysis.get": "8C", "analysis.find": "8C", "media.pcm": "8C", "media.frames": "8C", "job.submit": "8C", "media.file": "8H", "secret.grant": "8G" };
    for (const [method, batch] of Object.entries(want)) {
      const e = await direct(r, job, method, {});
      expect(e._tag).toBe("Left");
      if (e._tag === "Left") expect([e.left.code, (e.left.details as Record<string, unknown>).availableFrom]).toEqual(["INVALID_REQUEST", batch]);
    }
    const unknown = await direct(r, job, "fs.read", {});
    expect(unknown._tag === "Left" && (unknown.left.details as Record<string, unknown>).reason).toBe("unknownMethod");
  });

  test("denials: permission, project reach, live invoker", async () => {
    const r = await rig();
    // No `project:read` grant.
    const noGrant = await direct(r, r.jobOf({ grants: [] }), "project.snapshot", {});
    expect(noGrant._tag === "Left" && [noGrant.left.code, (noGrant.left.details as Record<string, unknown>).missing]).toEqual(["PLUGIN_PERMISSION_DENIED", "grant"]);
    // The invoker lacks `project.read`.
    const noScope = await direct(r, r.jobOf({ scopes: ["library.read"] }), "project.view", {});
    expect(noScope._tag === "Left" && [noScope.left.code, (noScope.left.details as Record<string, unknown>).missing]).toEqual(["PLUGIN_PERMISSION_DENIED", "invoker"]);
    // A project outside the invoker's reach looks like none (even for `log`).
    const other = r.jobOf({ projects: ["p1"], project: "p2" });
    expect(codeOf(await direct(r, other, "project.snapshot", {}))).toBe("PROJECT_NOT_FOUND");
    expect(codeOf(await direct(r, other, "log", { level: "info", msg: "x" }))).toBe("PROJECT_NOT_FOUND");
    // net.* is Task 8's; its permission is checked first.
    expect(codeOf(await direct(r, r.jobOf(), "net.read", { handle: "h", offset: 0, length: 1 }))).toBe("PLUGIN_PERMISSION_DENIED");
    // revoked_invoker_narrows_running_call: the invoker ends between two calls of one job.
    const job = r.jobOf();
    expect(codeOf(await direct(r, job, "project.snapshot", {}))).toBe("ok");
    r.world.live = () => undefined;
    expect(codeOf(await direct(r, job, "project.snapshot", {}))).toBe("CREDENTIAL_REVOKED");
    // An invoker the live sources do not know is ended too.
    expect(codeOf(await direct(r, r.jobOf({ owner: { kind: "console", id: "console" } }), "log", { level: "info", msg: "x" }))).toBe("CREDENTIAL_REVOKED");
  });

  test("call_carries_authority_key: from the job, never the request; another key is retired", async () => {
    const r = await rig();
    const job = r.jobOf();
    const key = r.host.warmKeyOf(job);
    expect(codeOf(await direct(r, job, "log", { level: "info", msg: "x" }, key))).toBe("ok");
    // A confined worker's `<project>/` prefix is the pool's, not the authority's.
    expect(codeOf(await direct(r, job, "log", { level: "info", msg: "x" }, `p1/${key}`))).toBe("ok");
    // A process of another authority (another rights hash) cannot act for this job.
    const narrower = r.host.warmKeyOf(r.jobOf({ scopes: ["library.read"] }));
    expect(narrower).not.toBe(key);
    expect(codeOf(await direct(r, job, "log", { level: "info", msg: "x" }, narrower))).toBe("PLUGIN_AUTHORITY_RETIRED");
  });

  test("retired_generation_call_fails: a bump between two calls of a running job", async () => {
    const r = await rig();
    const submitted = r.command({ op: "calls", pauseMs: 1500, calls: [{ method: "log", params: { level: "info", msg: "one" } }, { method: "log", params: { level: "info", msg: "two" } }] });
    for (let i = 0; i < 400 && !r.logs.some((l) => l.message.endsWith(" one")); i++) await new Promise((x) => setTimeout(x, 25));
    void Effect.runFork(r.host.authorityChanged("person"));
    const done = await submitted;
    const got = (done.result as { replies: Array<{ value?: unknown; error?: { code: string; details: Record<string, unknown> } }> }).replies;
    expect(got[0]?.value).toEqual({});
    expect(got[1]?.error).toMatchObject({ code: "PLUGIN_AUTHORITY_RETIRED", details: { retryable: true } });
    // A cold process's key is checked the same way.
    const job = r.jobOf();
    expect(codeOf(await direct(r, job, "log", { level: "info", msg: "x" }, null))).toBe("ok");
  });

  test("log_is_rate_limited_and_scrubbed", async () => {
    const r = await rig();
    const calls = Array.from({ length: LOG_PER_SECOND + 5 }, (_, i) => ({ method: "log", params: { level: "warn", msg: `m${i} at /home/someone/secret/file.txt\u001b[31m\nnext` } }));
    await replies(r, calls.slice(0, 15));
    await replies(r, calls.slice(15));
    const mine = r.logs.filter((l) => / m\d+ /.test(l.message));
    expect(mine.length).toBe(LOG_PER_SECOND);
    for (const l of mine) {
      expect(l.message).not.toContain("/home/someone");
      expect(l.message).toContain("<path>");
      expect(/[\u0000-\u001f]/.test(l.message)).toBe(false);
      // The plugin's keyed id, not its name.
      expect(l.message).toMatch(/^\[plugin#[0-9a-f]{16}\] /);
      expect(l.message).not.toContain(ID);
      expect(l.level).toBe("warning");
    }
    // Capped at 4 KiB; the dropped ones are counted once the next window opens.
    expect(Buffer.byteLength(scrubLogText("é".repeat(5000)))).toBeLessThanOrEqual(4096);
    let now = 0;
    const out: string[] = [];
    const log = makePluginLog({ write: (_l, m) => void out.push(m), clock: () => now, bundled: (id) => id === "@finisher/captions" });
    for (let i = 0; i < LOG_PER_SECOND + 3; i++) log.call("w", "@finisher/captions@1.0.0", "info", `x${i}`);
    now = 1000;
    log.call("w", "@finisher/captions@1.0.0", "info", "after");
    expect(out.filter((m) => m.includes("dropped"))).toEqual([`[@finisher/captions] 3 log messages dropped (over ${LOG_PER_SECOND} a second)`]);
    expect(out.at(-1)).toBe("[@finisher/captions] after");
  });

  test("stderr_never_journaled: scrubbed into the server log, capped, never in the journal", async () => {
    const r = await rig();
    const marker = "stderr-marker-7f3a";
    const done = await r.command({ op: "stderr", lines: [`${marker} at /home/someone/x/y`, "\u001b[2Jclear"] });
    expect(done.state).toBe("succeeded");
    for (let i = 0; i < 200 && !r.logs.some((l) => l.message.includes(marker)); i++) await new Promise((x) => setTimeout(x, 25));
    const line = r.logs.find((l) => l.message.includes(marker));
    expect(line?.message).toMatch(/^\[plugin#[0-9a-f]{16} stderr\] stderr-marker-7f3a at <path>$/);
    expect(r.logs.some((l) => l.message.includes("\u001b"))).toBe(false);
    // The journal sink saw the job and none of its stderr.
    expect(JSON.stringify([r.sink.done, r.sink.exits, r.sink.forwarded])).not.toContain(marker);
    expect(r.sink.done.length).toBeGreaterThan(0);
    // 64 KiB a minute per process.
    let now = 0;
    const out: string[] = [];
    const log = makePluginLog({ write: (_l, m) => void out.push(m), clock: () => now });
    const kib = "y".repeat(1024);
    for (let i = 0; i < 70; i++) log.stderr("w", `${ID}@1.0.0`, kib);
    expect(out.length).toBeLessThan(65);
    now = 60_000;
    log.stderr("w", `${ID}@1.0.0`, "later");
    expect(out.some((m) => /\d+ lines dropped/.test(m))).toBe(true);
  });

  test("plugin_context: the plugin, grants, package, the caller's data and the key's temp", async () => {
    const r = await rig();
    const job = r.jobOf();
    const c = r.host.contextOf(job, { timeoutS: 10 });
    expect(c).toMatchObject({ hostApi: PLUGIN_HOST_API, plugin: { id: ID, version: "1.0.0" }, grants: [READ, DATA_RW], locale: "en", limits: { timeoutS: 10, messageBytes: 1 << 20, pageBytes: PAGE_BYTES } });
    expect(c.dirs.plugin).toBe(join(r.serverData, "plugins", "store", "@test", "probe", "1.0.0-t"));
    expect(c.dirs.data).toBe(r.host.dataDirOf(job)?.path);
    expect(c.dirs.tmp).toBe(keyTmpDir(r.serverData, r.host.warmKeyOf(job)));
    expect(r.host.contextOf(r.jobOf({ grants: [READ] }), { timeoutS: 10, locale: "de" })).not.toHaveProperty("dirs.data");
  });
});
