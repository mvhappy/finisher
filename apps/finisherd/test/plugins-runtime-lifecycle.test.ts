// Code plugins' lifecycle with their processes (sp8b plan Task 5; Decisions 15, 18): enable,
// disable, crash loop and updates, over 8A's registries and a job engine on the same
// `control.db`. The packages carry the test-only probe (`fixtures/plugins/probe`) as their
// `main`; plugin commands are Task 9's, so the runs here are `plugin`-family jobs of the
// probe's `test.echo` kind. Every process here is one this test's engine started.
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { JobOwner, JobView } from "@finisher/contracts";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { Audit } from "../src/control/audit.ts";
import { ControlStoreTag } from "../src/control/db.ts";
import type { JobEngine } from "../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../src/jobs/events.ts";
import type { KindSpec } from "../src/jobs/kinds.ts";
import { buildJobEngine } from "../src/jobs/layer.ts";
import { CRASH_LOOP_WINDOW_MS, makeCrashLoop } from "../src/plugins/host/crash-loop.ts";
import { keyTmpDir } from "../src/plugins/host/keys.ts";
import { makePluginHost, type PluginHost } from "../src/plugins/host/processes.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { advisory, as, code, CONSOLE, codeOf, failure, NET, plugins, run } from "./support/plugins-fixture.ts";
import { rightsFor } from "../src/plugins/host/rights.ts";

const PROBE = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe", "main.mjs"), "utf8");
const ID = "@acme/tool";
const PERSON: JobOwner = { kind: "local", id: "local" };
const ECHO: KindSpec = { kind: "test.echo", version: 1, family: "plugin", scopes: [], resources: [], warm: true, defaultPriority: "interactive", qos: "normal", timeoutS: 60, maxAttempts: 1 };
const probe = (version: string, extra: Record<string, unknown> = {}, permissions?: object[], id = ID) => ({ ...code(id, version, permissions), files: { "dist/plugin.mjs": PROBE }, ...extra });
type RunOpts = { version?: string; priority?: "interactive" | "normal"; id?: string; project?: string };

const f = plugins();
const engines: JobEngine[] = [];
afterEach(async () => {
  for (const e of engines.splice(0)) await run(e.close());
});

interface Rig {
  readonly engine: JobEngine;
  readonly host: PluginHost;
  readonly sink: ReturnType<typeof memorySink>;
  readonly s: Awaited<ReturnType<typeof f.setup>>;
  readonly clock: { now: number };
  /** Submits without waiting. */
  start(op: Record<string, unknown>, o?: RunOpts): Promise<JobView>;
  submit(op: Record<string, unknown>, o?: RunOpts): Promise<JobView>;
  echo(op: Record<string, unknown>, o?: RunOpts): Promise<Record<string, unknown>>;
  install(o?: { update?: boolean; id?: string }): Promise<void>;
}

/** `cpu`: the kind takes the one `cpu` slot, so a second cold job queues behind a running one. */
async function rig(packages: object[], opts: { cpu?: boolean } = {}): Promise<Rig> {
  const serverData = realpathSync(f.serverData);
  const clock = { now: Date.now() };
  const tables = new PluginTables(f.control);
  const host = makePluginHost({ serverData, control: f.control, tables, audit: new Audit(f.control, () => f.now), clock: () => clock.now });
  const sink = memorySink();
  const engine = await Effect.runPromise(
    buildJobEngine(serverData, {
      testKinds: false,
      cgroups: "off",
      cores: opts.cpu === true ? 1 : 4,
      timing: { killWaitMs: 2000, exitDrainMs: 100, hungMs: 1500 },
      register: (r) => {
        host.register(r);
        r.registerKind(opts.cpu === true ? { ...ECHO, resources: ["cpu"] } : ECHO);
      },
    }).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, f.control), Layer.succeed(JobEventSinkTag, sink)))),
  );
  engines.push(engine);
  host.bind(engine);
  const s = await f.setup({ packages }, { host: host.hooks });
  const r: Rig = {
    engine,
    host,
    sink,
    s,
    clock,
    start: (op, o = {}) =>
      run(
        engine.submit(PERSON, {
          kind: "test.echo",
          params: { value: op },
          via: { plugin: `${o.id ?? ID}@${o.version ?? "1.0.0"}` },
          rights: rightsFor({ invoker: { ...PERSON, scopes: [], projects: "*" }, grants: [], class: "person" }),
          ...(o.priority === undefined ? {} : { priority: o.priority }),
          ...(o.project === undefined ? {} : { project: o.project }),
        }),
      ),
    async submit(op, o = {}) {
      const v = await r.start(op, o);
      const [done] = await run(engine.wait([v.id], { timeoutMs: 20_000 }));
      return done!;
    },
    async echo(op, o) {
      const v = await r.submit(op, o);
      expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
      return v.result as Record<string, unknown>;
    },
    async install(o = {}) {
      const plan = (await run(s.life.plan({ id: o.id ?? ID, ...(o.update === true ? { update: true } : {}) }, CONSOLE))).plan;
      await run(s.life.install({ planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } }, CONSOLE));
    },
  };
  return r;
}

const plugin = (r: Rig) => r.engine.workers().warm.filter((w) => w.family === "plugin");
/** The person's warm key of `id`@1.0.0 without a project (the rig's rights). */
const personKey = (r: Rig, id: string): string => r.host.warmKeyOf({ id: "k", owner: PERSON, via: { plugin: `${id}@1.0.0` }, rights: rightsFor({ invoker: { ...PERSON, scopes: [], projects: "*" }, grants: [], class: "person" }) } as unknown as JobView);
const exists = (pid: number): boolean => {
  try {
    process.kill(pid, 0); // signal 0 only asks whether the process exists
    return true;
  } catch {
    return false;
  }
};
/** Whether `pid` is still there after up to 2 s (its exit is reaped just after the pool lets it go). */
const alive = async (pid: number): Promise<boolean> => {
  for (let i = 0; i < 100 && exists(pid); i++) await new Promise((x) => setTimeout(x, 20));
  return exists(pid);
};

describe("enable and disable", () => {
  test("enable_code_plugin_runs_a_command (a plugin job: commands are Task 9's)", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    expect(r.s.tables.installedOne(ID)).toMatchObject({ enabled: false, state: "disabled" });
    expect((await r.submit({ op: "pid" })).error?.code).toBe("PLUGIN_DISABLED");
    const enabled = await run(r.s.h["plugin.enable"]({ id: ID }, as(CONSOLE)));
    expect(enabled.plugin).toMatchObject({ enabled: true, state: "active" });
    expect((await r.echo({ op: "pid" })).pid).toBeGreaterThan(0);
    // `plugin.list` says it runs unconfined (7C's `plugin` profile is not enforced yet).
    const list = await run(r.s.h["plugin.list"]());
    expect(list.plugins.find((p) => p.id === ID)).toMatchObject({ kind: "code", confined: false });
    expect(list.plugins.find((p) => p.id === ID)?.problem).toBeUndefined();
    // Enabling stays the owner's (8A, unchanged).
    expect(r.s.rows().filter((x) => x.kind === "plugin.enable").map((x) => [x.principalKind, x.outcome])).toEqual([["console", "ok"]]);
  });

  test("disable_stops_processes_and_fails_queued", async () => {
    const OTHER = "@acme/other";
    const r = await rig([probe("1.0.0"), probe("1.0.0", {}, undefined, OTHER)], { cpu: true });
    await r.install();
    await r.install({ id: OTHER });
    await run(r.s.life.enable(ID));
    await run(r.s.life.enable(OTHER));
    const idle = (await r.echo({ op: "pid" })).pid as number;
    // Another plugin's cold job holds the one cpu slot for the whole test; this plugin's cold
    // job waits behind it, queued. Its warm job runs (warm dispatches overrun the pool).
    const holder = await r.start({ op: "sleep", ms: 15_000 }, { priority: "normal", id: OTHER });
    const running = await r.start({ op: "sleep", ms: 10_000 });
    for (let i = 0; i < 200 && ((await run(r.engine.get(holder.id))).state !== "running" || (await run(r.engine.get(running.id))).state !== "running"); i++) await new Promise((x) => setTimeout(x, 20));
    const queued = await r.start({ op: "pid" }, { priority: "normal" });
    expect((await run(r.engine.get(queued.id))).state).toBe("queued");
    const tmp = keyTmpDir(realpathSync(f.serverData), personKey(r, ID));
    expect(existsSync(tmp)).toBe(true);
    await run(r.s.life.disable(ID));
    // The queued job failed at once (`job.get` sees it) while the slot it waits for is still held.
    expect(await run(r.engine.get(queued.id))).toMatchObject({ state: "failed", error: { code: "PLUGIN_DISABLED" } });
    expect((await run(r.engine.get(holder.id))).state).toBe("running");
    // The running job is cancelled, every process of the plugin is gone, and so is its key's temp.
    const [ended] = await run(r.engine.wait([running.id], { timeoutMs: 10_000 }));
    expect(ended?.state).toBe("cancelled");
    expect(plugin(r).filter((w) => w.key.startsWith(`${ID}@`))).toEqual([]);
    expect(await alive(idle)).toBe(false);
    expect(existsSync(tmp)).toBe(false);
    // A job after the disable fails without a process.
    expect((await r.submit({ op: "pid" })).error?.code).toBe("PLUGIN_DISABLED");
    await run(r.engine.cancel(holder.id));
  });

  test("disable stops only that plugin's processes (M20)", async () => {
    const OTHER = "@acme/other";
    const r = await rig([probe("1.0.0"), probe("1.0.0", {}, undefined, OTHER)]);
    await r.install();
    await r.install({ id: OTHER });
    await run(r.s.life.enable(ID));
    await run(r.s.life.enable(OTHER));
    const mine = (await r.echo({ op: "pid" })).pid as number;
    const theirs = (await r.echo({ op: "pid" }, { id: OTHER })).pid as number;
    await run(r.s.life.disable(ID));
    expect(await alive(mine)).toBe(false);
    expect(exists(theirs)).toBe(true);
    expect(plugin(r).map((w) => w.key)).toEqual([personKey(r, OTHER)]);
    expect((await r.echo({ op: "pid" }, { id: OTHER })).pid).toBe(theirs);
  });

  test("models_manifest_cannot_enable; a platform it does not list either", async () => {
    const model = { id: "m1", url: "https://models.example.org/m1.onnx", sha256: "a".repeat(64), bytes: 10, license: "MIT", purpose: "plugin.model.purpose" };
    const r = await rig([probe("1.0.0")]);
    await r.install();
    const dir = join(realpathSync(f.serverData), "plugins", r.s.tables.installedFull(ID)!.dir);
    const manifest = JSON.parse(readFileSync(join(dir, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
    const { chmodSync, mkdirSync, writeFileSync } = await import("node:fs");
    // The store's files are read-only as 8A places them; this test changes them on purpose.
    chmodSync(dir, 0o755);
    chmodSync(join(dir, "finisher-plugin.json"), 0o644);
    mkdirSync(join(dir, "messages"));
    writeFileSync(join(dir, "messages", "en.json"), JSON.stringify({ "plugin.model.purpose": "A test model" }));
    writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ ...manifest, messages: "messages", locales: ["en"], models: [model] }));
    const e = await failure(r.s.h["plugin.enable"]({ id: ID }, as(CONSOLE)));
    expect(e).toMatchObject({ code: "PLUGIN_CONTRIBUTION_UNSUPPORTED", details: { kind: "models", hostApi: "1.2.0" } });
    const other = process.platform === "win32" ? "linux-x64" : "win32-x64";
    writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ ...manifest, platforms: [other] }));
    expect(await codeOf(r.s.h["plugin.enable"]({ id: ID }, as(CONSOLE)))).toBe("PLUGIN_PLATFORM");
    expect(r.s.tables.installedOne(ID)).toMatchObject({ enabled: false, state: "disabled" });
  });
});

describe("crash loop", () => {
  test("three_crashes_in_ten_minutes_crash_loop (and two do not)", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    for (const step of [0, CRASH_LOOP_WINDOW_MS, 1]) {
      r.clock.now += step;
      expect((await r.submit({ op: "crash" }, { priority: "normal" })).error?.code).toBe("PLUGIN_CRASHED");
    }
    // The first is ten minutes before the second: two in the window.
    expect(r.s.tables.installedOne(ID)?.state).toBe("active");
    r.clock.now += 1;
    expect((await r.submit({ op: "crash" }, { priority: "normal" })).error?.code).toBe("PLUGIN_CRASHED");
    for (let i = 0; i < 100 && r.s.tables.installedOne(ID)?.state !== "crashLoop"; i++) await new Promise((x) => setTimeout(x, 20));
    expect(r.s.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "crashLoop", problem: "PLUGIN_CRASH_LOOP" });
    expect(r.s.rows().filter((x) => x.kind === "plugin.crashLoop")).toMatchObject([{ principalKind: "system", outcome: "ok", code: "PLUGIN_CRASH_LOOP", details: { pluginId: ID, version: "1.0.0", auto: true } }]);
    expect((await r.submit({ op: "pid" })).error?.code).toBe("PLUGIN_CRASH_LOOP");
    expect((await run(r.s.h["plugin.list"]())).plugins.find((p) => p.id === ID)).toMatchObject({ state: "crashLoop", problem: "PLUGIN_CRASH_LOOP" });
  });

  test("a crash loop stops every key's process, and fails the plugin's queued jobs (M13)", async () => {
    const OTHER = "@acme/other";
    const r = await rig([probe("1.0.0"), probe("1.0.0", {}, undefined, OTHER)], { cpu: true });
    await r.install();
    await r.install({ id: OTHER });
    await run(r.s.life.enable(ID));
    await run(r.s.life.enable(OTHER));
    const other = (await r.echo({ op: "pid" }, { project: "p1" })).pid as number;
    // Another plugin holds the cpu slot; this plugin's cold job waits behind it.
    const holder = await r.start({ op: "sleep", ms: 15_000 }, { priority: "normal", id: OTHER });
    for (let i = 0; i < 200 && (await run(r.engine.get(holder.id))).state !== "running"; i++) await new Promise((x) => setTimeout(x, 20));
    const queued = await r.start({ op: "pid" }, { priority: "normal" });
    // Three warm crashes (warm dispatches overrun the pool), each on a fresh process.
    for (let i = 0; i < 3; i++) expect((await r.submit({ op: "crash" })).error?.code).toBe("PLUGIN_CRASHED");
    for (let i = 0; i < 100 && (await run(r.engine.get(queued.id))).state === "queued"; i++) await new Promise((x) => setTimeout(x, 20));
    expect(r.s.tables.installedOne(ID)?.state).toBe("crashLoop");
    expect(await run(r.engine.get(queued.id))).toMatchObject({ state: "failed", error: { code: "PLUGIN_CRASH_LOOP" } });
    expect((await run(r.engine.get(holder.id))).state).toBe("running");
    // The idle process of another key of the plugin is stopped too.
    expect(await alive(other)).toBe(false);
    expect(plugin(r).filter((w) => w.key.startsWith(`${ID}@`))).toEqual([]);
    await run(r.engine.cancel(holder.id));
  });

  test("an owner's disable and enable clears the crash count", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    for (let i = 0; i < 2; i++) await r.submit({ op: "crash" }, { priority: "normal" });
    await run(r.s.life.disable(ID));
    await run(r.s.life.enable(ID));
    await r.submit({ op: "crash" }, { priority: "normal" });
    await new Promise((x) => setTimeout(x, 200));
    expect(r.s.tables.installedOne(ID)?.state).toBe("active");
  });

  test("crash_loop_cleared_by_owner_enable", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    for (let i = 0; i < 3; i++) await r.submit({ op: "crash" }, { priority: "normal" });
    for (let i = 0; i < 100 && r.s.tables.installedOne(ID)?.state !== "crashLoop"; i++) await new Promise((x) => setTimeout(x, 20));
    expect(r.s.tables.installedOne(ID)?.state).toBe("crashLoop");
    // Only the owner's enable clears it (an agent or `local` cannot: 8A's human-only rule).
    await run(r.s.h["plugin.enable"]({ id: ID }, as(CONSOLE)));
    expect(r.s.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "active" });
    expect(r.s.tables.installedOne(ID)?.problem).toBeUndefined();
    expect((await r.echo({ op: "pid" })).pid).toBeGreaterThan(0);
  });
});

describe("updates", () => {
  test("widening_update_keeps_old_version_serving, then the approved update drains the old process", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    const old = (await r.echo({ op: "pid" })).pid as number;
    f.publish(r.s.reg, { packages: [probe("1.1.0", {}, [{ perm: "project:read" }, NET])] });
    // A refresh and an unapproved plan change nothing: the old version keeps serving.
    await run(r.s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    await run(r.s.life.plan({ id: ID, update: true }, CONSOLE));
    expect(r.s.tables.installedOne(ID)?.version).toBe("1.0.0");
    expect((await r.echo({ op: "pid" })).pid).toBe(old);
    // A person approves: the row switches, stays enabled, the old key's process stops.
    await r.install({ update: true });
    expect(r.s.tables.installedOne(ID)).toMatchObject({ version: "1.1.0", enabled: true, state: "active" });
    expect(plugin(r).filter((w) => w.key.startsWith(`${ID}@1.0.0|`))).toEqual([]);
    expect(await alive(old)).toBe(false);
    const next = (await r.echo({ op: "pid" }, { version: "1.1.0" })).pid;
    expect(next).not.toBe(old);
    expect((await r.submit({ op: "pid" }, { priority: "normal" })).error?.code).toBe("PLUGIN_NOT_FOUND");
  });

  test("an approved update lets a running job of the old version finish (M17)", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    f.publish(r.s.reg, { packages: [probe("1.0.1")] });
    const running = await r.start({ op: "sleep", ms: 1500 });
    for (let i = 0; i < 100 && (await run(r.engine.get(running.id))).state !== "running"; i++) await new Promise((x) => setTimeout(x, 20));
    await r.install({ update: true });
    const [done] = await run(r.engine.wait([running.id], { timeoutMs: 10_000 }));
    expect(done?.state, JSON.stringify(done?.error)).toBe("succeeded");
    expect(r.s.tables.installedOne(ID)?.version).toBe("1.0.1");
  });

  test("an approved update of a crash-looped plugin stays in the crash loop (M19)", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    for (let i = 0; i < 3; i++) await r.submit({ op: "crash" }, { priority: "normal" });
    for (let i = 0; i < 100 && r.s.tables.installedOne(ID)?.state !== "crashLoop"; i++) await new Promise((x) => setTimeout(x, 20));
    f.publish(r.s.reg, { packages: [probe("1.0.1")] });
    await r.install({ update: true });
    expect(r.s.tables.installedOne(ID)).toMatchObject({ version: "1.0.1", enabled: true, state: "crashLoop", problem: "PLUGIN_CRASH_LOOP" });
    expect((await r.submit({ op: "pid" }, { version: "1.0.1" })).error?.code).toBe("PLUGIN_CRASH_LOOP");
  });

  test("third_party_update_still_needs_acceptance", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    f.publish(r.s.reg, { packages: [probe("1.0.1")] });
    const plan = (await run(r.s.life.plan({ id: ID, update: true }, CONSOLE))).plan;
    expect(await codeOf(r.s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE))).toBe("THIRD_PARTY_CODE_NOT_ACCEPTED");
    expect(r.s.tables.installedOne(ID)).toMatchObject({ version: "1.0.0", enabled: true, state: "active" });
  });

  test("a revocation stops the plugin's processes", async () => {
    const r = await rig([probe("1.0.0")]);
    await r.install();
    await run(r.s.life.enable(ID));
    const pid = (await r.echo({ op: "pid" })).pid as number;
    f.publish(r.s.reg, { advisories: [advisory(ID, "<1.1.0")] });
    await run(r.s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    expect(r.s.tables.installedOne(ID)?.state).toBe("revoked");
    expect(plugin(r)).toEqual([]);
    expect(await alive(pid)).toBe(false);
    expect((await r.submit({ op: "pid" })).error?.code).toBe("PLUGIN_REVOKED");
  });
});

describe("the crash loop's window (M3)", () => {
  const job = (at: number): JobView => ({ id: `j${at}`, family: "plugin", via: { plugin: `${ID}@1.0.0` }, error: { code: "PLUGIN_CRASHED", message: "m", details: {} } }) as unknown as JobView;
  const loopAt = (times: number[]): string[] => {
    const clock = { now: 0 };
    const states: string[] = [];
    const row = { id: ID, version: "1.0.0", enabled: true, state: "active" };
    const tables = { installedOne: () => row, store: { tx: (g: () => void) => g() }, setState: (_: string, __: boolean, st: string) => states.push(st), setProblem: () => {} };
    const loop = makeCrashLoop({ tables: tables as never, clock: () => clock.now, stop: () => states.push("stop") });
    for (const t of times) {
      clock.now = t;
      loop.onDone(job(t));
    }
    return states;
  };

  test("a crash exactly ten minutes after the first is outside its window", () => {
    expect(loopAt([0, 1, CRASH_LOOP_WINDOW_MS])).toEqual([]);
    expect(loopAt([0, 1, CRASH_LOOP_WINDOW_MS - 1])).toEqual(["crashLoop", "stop"]);
  });

  test("the owner's enable clears the count", () => {
    const clock = { now: 0 };
    const states: string[] = [];
    const tables = { installedOne: () => ({ id: ID, version: "1.0.0", enabled: true, state: "active" }), store: { tx: (g: () => void) => g() }, setState: (_: string, __: boolean, st: string) => states.push(st), setProblem: () => {} };
    const loop = makeCrashLoop({ tables: tables as never, clock: () => clock.now, stop: () => {} });
    loop.onDone(job(0));
    loop.onDone(job(1));
    loop.clear(ID);
    loop.onDone(job(2));
    expect(states).toEqual([]);
  });
});
