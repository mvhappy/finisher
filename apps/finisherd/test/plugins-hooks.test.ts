// Plugin hook handlers in the hook engine (7F plan Task 10, Decisions 19, 20; replaces the 8B
// plan's Task 17 finisherd half). A real server (`main.ts`'s `serve`: the plugin host, the job
// engine, the chain, the agent runtime), the 8B test probe (`fixtures/plugins/probe`) installed
// by writing its rows into the running server's `control.db`, with a `hooks.json` that says what
// each handler answers and where it logs. Agent sessions run on the scripted `fake` harness
// (`FIN_TEST_HOOKS=1`); command hooks run `direct`.
//
// - A handler receives an event only with a `hooks:observe` grant naming it, checked at every
//   run (grants change without a restart), and only in a project its rights reach.
// - The plugin layer runs last; a handler's identity is `plugin:<id>/<handler>`; a block is
//   `HOOK_BLOCKED` and a `hook.block` row with `layer: plugin` and the plugin's id.
// - No `modify` (Decision 19): a `modify` answer is a failure, with or without `hooks:modify`.
// - The manifest's `timeoutS`, then the event's default `onTimeout` (`PreMerge` blocks).
// - The job's rights are the event's principal ∩ the plugin's grants.
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { HookEvent, JobView, Node, Session } from "@finisher/contracts";
import type { HookContribution, HookRequest, Permission } from "@finisher/core-types";
import { jobKindVersions, sandboxStatus, serverIdentity, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import { controlDbPath, openControlStore } from "../src/control/db.ts";
import type { ConfiguredHook } from "../src/hooks/config.ts";
import type { HookAt } from "../src/hooks/engine.ts";
import { makePluginHost } from "../src/plugins/host/processes.ts";
import { systemInvoker } from "../src/plugins/host/rights.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { CONSOLE } from "../src/principal.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, type Exit, generationOf, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const PROBE = join(import.meta.dirname, "fixtures", "plugins", "probe");
const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });
const PLUGIN = "@test/probe";
const name = (handler: string) => `plugin:${PLUGIN}/${handler}`;

const hooksEnv = process.env.FIN_TEST_HOOKS;
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooksEnv === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooksEnv;
});

let tmp: string;
let serverData: string;
let server: AgentServer | undefined;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "plugins-hooks-")));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
});
afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
/** A run that goes on until interrupted (the session stays `running`). */
const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };

/** A fake command hook in the user layer. */
const userHook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });

const observe = (...hooks: HookEvent[]): Permission => ({ perm: "hooks:observe", hooks });

interface ProbeSpec {
  readonly hooks: readonly HookContribution[];
  readonly grants: readonly Permission[];
  /** What each handler answers (`hooks.json`'s `handlers`). */
  readonly handlers?: Record<string, unknown>;
}

const storeDir = (plugin = PLUGIN) => join(serverData, "plugins", "store", plugin, "1.0.0-t");


function withTables<A>(f: (t: PluginTables) => A): A {
  const control = openControlStore(serverData, { serverId: serverIdentity(serverData).id, checkpoint: false });
  try {
    return f(new PluginTables(control));
  } finally {
    void control.close();
  }
}

const setGrants = (grants: readonly Permission[], plugin = PLUGIN) => withTables((t) => t.setGrants(plugin, "1.0.0", grants.map((g) => JSON.stringify(g)), "console:console", 0));
const setHandlers = (handlers: Record<string, unknown>, plugin = PLUGIN) => writeFileSync(join(storeDir(plugin), "hooks.json"), JSON.stringify({ handlers }));

/** Installs the probe as `plugin` in the running server's store and control.db, enabled, with `spec`'s hooks and grants. */
function installProbe(spec: ProbeSpec, plugin = PLUGIN): void {
  const full = storeDir(plugin);
  mkdirSync(dirname(full), { recursive: true });
  cpSync(PROBE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  const events = [...new Set(spec.hooks.map((h) => h.event))];
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, id: plugin, hostApi: "^1.2", permissions: [{ perm: "project:read" }, observe(...events)], contributes: { hooks: spec.hooks } }));
  setHandlers(spec.handlers ?? {}, plugin);
  withTables((t) => t.addPlugin({ id: plugin, version: "1.0.0", source: "registry", treeHash: "t", dir: `store/${plugin}/1.0.0-t`, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 }));
  setGrants(spec.grants, plugin);
}

let n = 0;
/** The person's client of the running server (reads generations). */
let srvLocal: NdjsonClient | undefined;
const id = () => `01T${String(++n).padStart(23, "0")}`;

interface Booted {
  readonly local: NdjsonClient;
  readonly projectId: string;
}

/** A server whose user layer holds `userHooks`, the probe installed, a project with an asset on `main`. */
async function boot(probe: ProbeSpec, userHooks: unknown[] = [], plugin = PLUGIN): Promise<Booted> {
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: userHooks }));
  server = new AgentServer(serverData, { timers: false, fakeScripts: [WORKING], hooks: { status: DIRECT } });
  await server.start();
  installProbe(probe, plugin);
  const local = await server.local();
  srvLocal = local;
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

async function until<A>(f: () => A | undefined | false | Promise<A | undefined | false>, what: string, ms = 10_000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = await f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

const srv = () => server!;
const status = (sessionId: string) => srv().core.store.get(sessionId)!.status;
const nodes = (sessionId: string) => srv().core.store.nodes(sessionId);
const payloadOf = (node: Node) => node.payload as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const stats = () => srv().agents.hookWiring.engine.stats().events;
const settled = () => srv().agents.hookWiring.engine.settled();
const hookNotices = (sessionId: string) => nodes(sessionId).filter((x) => x.kind === "notice" && payloadOf(x).hook !== undefined).map((x) => payloadOf(x).hook as Record<string, unknown>);

/** A session `local` started on the fake harness, in its first run; its credential's client. */
async function started(b: Booted) {
  const { session } = ok<{ session: Session }>(await b.local.call("agent.session.start", { projectId: b.projectId, harness: "fake", prompt: "go" }));
  await until(() => status(session.id) === "running", `${session.id} running`);
  return { session, a: await srv().agent(credentialPath(srv().core.store.root, session.id)) };
}

const branches = async (c: NdjsonClient, projectId: string) => ok<{ branches: Array<{ id: string; name?: string; head: number }> }>(await c.call("branch.list", { projectId, all: true })).branches;
const head = async (c: NdjsonClient, projectId: string, branch: string) => (await branches(c, projectId)).find((x) => x.id === branch || x.name === branch)!.head;
/** A clip on `on`, as `c`: one `tx.commit` (its history generation read as the person, so the call is the agent's only one). */
const edit = async (c: NdjsonClient, projectId: string, on: string, at = 0) => {
  const generation = await generationOf(srvLocal!, projectId, on);
  return c.call("tx.commit", { projectId, tx: { ...tx(id(), 1, [insert(at)]), branch: on }, generation });
};
const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));
const hookJobs = async (b: Booted) => ok<{ jobs: JobView[] }>(await b.local.call("jobs.list", { kind: "plugin.hook" })).jobs.sort((x, y) => (x.id < y.id ? -1 : 1));
/** What the probe's handlers received (their `plugin.hook` jobs' params), in order. */
const received = async (b: Booted): Promise<Array<{ handler: string; event: HookEvent; request: HookRequest & { payload: Record<string, any> } }>> => // eslint-disable-line @typescript-eslint/no-explicit-any
  (await hookJobs(b)).map((j) => {
    const p = j.params as { handler: string; request: HookRequest & { payload: Record<string, any> } }; // eslint-disable-line @typescript-eslint/no-explicit-any
    return { handler: p.handler, event: p.request.event, request: p.request };
  });

/** Audit rows of `kind`: their principal and details. */
function auditRows(kind: string): Array<{ principalKind: string; principalId: string; details: Record<string, unknown> }> {
  const db = new DatabaseSync(controlDbPath(serverData));
  try {
    const rows = db.prepare("SELECT principalKind, principalId, details FROM audit WHERE kind = ? ORDER BY id").all(kind) as Array<{ principalKind: string; principalId: string; details: string }>;
    return rows.map((r) => ({ ...r, details: JSON.parse(r.details) as Record<string, unknown> }));
  } finally {
    db.close();
  }
}

/** A person's merge of a fresh feature branch into `main`. */
async function merge(b: Booted): Promise<Exit> {
  const feat = ok<{ branch: { id: string } }>(await b.local.call("branch.create", { projectId: b.projectId, txId: id(), author: "x", from: "main", name: `feat-${id()}` })).branch.id;
  ok(await edit(b.local, b.projectId, feat));
  return b.local.call("history.merge", { projectId: b.projectId, txId: id(), author: "x", from: feat, into: "main" });
}

describe("receiving events (Decision 20)", () => {
  test("observe_without_grant_receives_nothing: a handler whose hooks:observe is not granted gets no job", async () => {
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "guard" }], grants: [{ perm: "project:read" }], handlers: { guard: { reply: { reply: "block", reason: "never" } } } });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    ok(await edit(a, b.projectId, root));
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    expect(await received(b)).toEqual([]);
    expect(await hookJobs(b)).toEqual([]);
    expect(stats().PreCommand).toMatchObject({ runs: 0, blocks: 0, failures: 0 });
  });

  test("observe_grant_is_per_event: only the granted events reach their handlers; a grant change applies at the next event", async () => {
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "cmd" }, { event: "PostCommand", handler: "after" }], grants: [observe("PreCommand")] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    ok(await edit(a, b.projectId, root));
    await settled();
    expect((await received(b)).map((r) => [r.handler, r.event])).toEqual([["cmd", "PreCommand"]]);
    // The request is the event's: its session, its project, the call's command names.
    const [req] = await received(b);
    expect(req!.request).toMatchObject({ event: "PreCommand", v: 1, session: { id: session.id }, project: { id: b.projectId } });
    expect(req!.request.payload.commands).toContain("rpc:tx.commit");
    // Granted now: the next event reaches both, without a restart.
    setGrants([observe("PreCommand", "PostCommand")]);
    ok(await edit(a, b.projectId, root, 5));
    await settled();
    await until(async () => (await received(b)).length === 3, "the PostCommand handler");
    expect((await received(b)).slice(1).map((r) => [r.handler, r.event])).toEqual([["cmd", "PreCommand"], ["after", "PostCommand"]]);
    // And taken away again: nothing more.
    setGrants([]);
    ok(await edit(a, b.projectId, root, 10));
    await settled();
    expect(await received(b)).toHaveLength(3);
  });

  test("plugin_block_refuses_pre_command: HOOK_BLOCKED as plugin:<id>/<handler>, nothing written, audited with the plugin's id", async () => {
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "guard" }], grants: [observe("PreCommand")], handlers: { guard: { reply: { reply: "block", reason: "no edits from agents" } } } });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    const e = await edit(a, b.projectId, root);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: name("guard"), reason: "no edits from agents", event: "PreCommand", layer: "plugin" });
    expect(await head(b.local, b.projectId, root)).toBe(before);
    expect(auditRows("hook.block")).toEqual([
      { principalKind: "agent", principalId: session.id, details: { event: "PreCommand", hookId: "guard", layer: "plugin", pluginId: PLUGIN, projectId: b.projectId, session: session.id } },
    ]);
    expect(stats().PreCommand).toMatchObject({ runs: 1, blocks: 1 });
    // A person's call is not an agent's: plugin handlers see agents' commands only.
    ok(await edit(b.local, b.projectId, "main"));
    expect(await received(b)).toHaveLength(1);
  });

  test("plugin_layer_runs_last: a user-layer block stops the chain before any plugin job; contexts join user then plugin", async () => {
    const b = await boot(
      { hooks: [{ event: "PreCommand", handler: "note" }], grants: [observe("PreCommand")], handlers: { note: { reply: { reply: "context", text: "from the plugin" } } } },
      [userHook("no-checkpoint", "PreCommand", ["--reply", "block:not now"], { match: "checkpoint" }), userHook("hint", "PreCommand", ["--reply", "context:from the user"], { match: "clip.insert" })],
    );
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const e = await a.call("history.checkpoint", { projectId: b.projectId, branch: root, name: "c1", txId: id(), author: "x" });
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "no-checkpoint", layer: "user" });
    expect(await received(b)).toEqual([]);
    expect(await hookJobs(b)).toEqual([]);
    // A commit: the user's context, then the plugin's.
    ok(await edit(a, b.projectId, root));
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the steer");
    expect(payloadOf(steered)).toMatchObject({ hookContext: { event: "PreCommand", hooks: ["hint", name("note")] } });
    const text = String(payloadOf(steered).text);
    expect(text.indexOf("from the user")).toBeLessThan(text.indexOf("from the plugin"));
    expect(text.indexOf("from the user")).toBeGreaterThanOrEqual(0);
  });

  test("plugin_order_is_plugin_id_then_manifest_order: two plugins, a block stops every later handler", async () => {
    const ctx = (text: string) => ({ reply: { reply: "context", text } });
    // `@test/zeta` is installed first, `@test/alpha` second; each lists its handlers out of alphabetical order.
    const b = await boot({
      hooks: [{ event: "PreCommand", handler: "y-first" }, { event: "PreCommand", handler: "a-block" }, { event: "PreCommand", handler: "c-after" }],
      grants: [observe("PreCommand")],
      handlers: { "y-first": ctx("zeta y"), "a-block": { reply: { reply: "block", reason: "zeta says no" } }, "c-after": ctx("zeta c") },
    }, [], "@test/zeta");
    installProbe({ hooks: [{ event: "PreCommand", handler: "m-first" }, { event: "PreCommand", handler: "b-second" }], grants: [observe("PreCommand")], handlers: { "m-first": ctx("alpha m"), "b-second": ctx("alpha b") } }, "@test/alpha");
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const e = await edit(a, b.projectId, root);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "plugin:@test/zeta/a-block", layer: "plugin" });
    const ran = async () => (await hookJobs(b)).map((j) => `${(j.via as { plugin: string }).plugin.split("@1")[0]}/${(j.params as { handler: string }).handler}`);
    // By plugin id, then manifest order; nothing after the block is submitted.
    expect(await ran()).toEqual(["@test/alpha/m-first", "@test/alpha/b-second", "@test/zeta/y-first", "@test/zeta/a-block"]);
    // Allowed now: every context, in the same order.
    setHandlers({ "y-first": ctx("zeta y"), "a-block": ctx("zeta a"), "c-after": ctx("zeta c") }, "@test/zeta");
    ok(await edit(a, b.projectId, root));
    expect((await ran()).slice(4)).toEqual(["@test/alpha/m-first", "@test/alpha/b-second", "@test/zeta/y-first", "@test/zeta/a-block", "@test/zeta/c-after"]);
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the steer");
    const names = ["plugin:@test/alpha/m-first", "plugin:@test/alpha/b-second", "plugin:@test/zeta/y-first", "plugin:@test/zeta/a-block", "plugin:@test/zeta/c-after"];
    expect(payloadOf(steered)).toMatchObject({ hookContext: { event: "PreCommand", hooks: names } });
    const text = String(payloadOf(steered).text);
    const at = ["alpha m", "alpha b", "zeta y", "zeta a", "zeta c"].map((t) => text.indexOf(t));
    expect(at.every((x) => x >= 0)).toBe(true);
    expect([...at].sort((x, y) => x - y)).toEqual(at);
  });

  test("disabled_plugin_hooks_stop_at_once: disabling the plugin stops its handlers at the next event", async () => {
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "guard" }], grants: [observe("PreCommand")], handlers: { guard: { reply: { reply: "block", reason: "frozen" } } } });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    expect(code(await edit(a, b.projectId, root))).toBe("HOOK_BLOCKED");
    const listed = ok<{ hooks: Array<{ layer: string; plugin?: string; id: string }> }>(await (await srv().console()).call("agent.hooks.list", { projectId: b.projectId })).hooks;
    expect(listed.filter((h) => h.layer === "plugin")).toMatchObject([{ id: "guard", plugin: PLUGIN }]);
    withTables((t) => t.setState(PLUGIN, false, "disabled", Date.now()));
    ok(await edit(a, b.projectId, root));
    expect(await received(b)).toHaveLength(1);
    const after = ok<{ hooks: Array<{ layer: string }> }>(await (await srv().console()).call("agent.hooks.list", { projectId: b.projectId })).hooks;
    expect(after.filter((h) => h.layer === "plugin")).toEqual([]);
  });
});

describe("no modify (Decision 19)", () => {
  test("oversized_reply_is_too_large: a reply over 64 KiB as sent fails before control characters are stripped; PreCommand passes, PreMerge blocks", { timeout: 30_000 }, async () => {
    // 72 KB of JSON (`\u0000` is six bytes escaped) whose reason is four bytes once stripped.
    const big = { reply: { reply: "block", reason: `${"\u0000".repeat(12_000)}stop` } };
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "loud" }, { event: "PreMerge", handler: "loud-merge" }], grants: [observe("PreCommand", "PreMerge")], handlers: { loud: big, "loud-merge": big } });
    expect(Buffer.byteLength(JSON.stringify(big.reply))).toBeGreaterThan(65_536);
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    ok(await edit(a, b.projectId, root));
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    await until(() => hookNotices(session.id).find((h) => h.failed !== undefined), "the failed notice");
    expect(hookNotices(session.id)).toContainEqual(expect.objectContaining({ event: "PreCommand", id: name("loud"), failed: "tooLarge" }));
    const e = await merge(b);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: name("loud-merge"), reason: "failed" });
    expect(stats().PreMerge).toMatchObject({ failures: 1 });
  });

  test("modify_reply_is_a_failure: a modify answer fails; PreCommand passes with a failed notice, PreMerge blocks", { timeout: 30_000 }, async () => {
    const modify = { reply: { reply: "modify", payload: { rewritten: true } } };
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "rewrite" }, { event: "PreMerge", handler: "gate" }], grants: [observe("PreCommand", "PreMerge")], handlers: { rewrite: modify, gate: modify } });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    ok(await edit(a, b.projectId, root));
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    await until(() => hookNotices(session.id).find((h) => h.failed !== undefined), "the failed notice");
    expect(hookNotices(session.id)).toContainEqual(expect.objectContaining({ event: "PreCommand", id: name("rewrite"), layer: "plugin", failed: "invalid" }));
    expect(stats().PreCommand).toMatchObject({ runs: 1, failures: 1, blocks: 0 });
    // PreMerge fails closed (Decision 3).
    const e = await merge(b);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: name("gate"), reason: "failed", event: "PreMerge", layer: "plugin" });
  });

  test("hooks_modify_grants_nothing: with hooks:modify granted a modify answer is still a failure; no rewrite, no hook.modify row", async () => {
    const b = await boot({
      hooks: [{ event: "PreCommand", handler: "rewrite" }],
      grants: [observe("PreCommand"), { perm: "hooks:modify", hooks: ["PreCommand"] }],
      handlers: { rewrite: { reply: { reply: "modify", payload: { projectId: "elsewhere" } } } },
    });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    ok(await edit(a, b.projectId, root));
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    expect(stats().PreCommand).toMatchObject({ failures: 1 });
    expect(auditRows("hook.modify")).toEqual([]);
    expect(auditRows("hook.block")).toEqual([]);
  });
});

describe("timeouts and rights", () => {
  test("hook_timeout_ordering: the manifest's timeoutS, then the event's default onTimeout; PreMerge blocks", { timeout: 40_000 }, async () => {
    const b = await boot({
      hooks: [{ event: "PreCommand", handler: "slow", timeoutS: 1 }, { event: "PreMerge", handler: "slow-merge", timeoutS: 1 }],
      grants: [observe("PreCommand", "PreMerge")],
      handlers: { slow: { sleepMs: 4000 }, "slow-merge": { sleepMs: 4000 } },
    });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const t0 = Date.now();
    ok(await edit(a, b.projectId, root));
    // Cut at the manifest's 1 s, not the 10 s default nor the kind's 60 s.
    expect(Date.now() - t0).toBeLessThan(3500);
    await until(() => hookNotices(session.id).find((h) => h.code === "HOOK_TIMEOUT"), "the timeout notice");
    expect(hookNotices(session.id)).toContainEqual(expect.objectContaining({ event: "PreCommand", id: name("slow"), layer: "plugin", code: "HOOK_TIMEOUT", timeoutS: 1 }));
    expect(stats().PreCommand.timeouts).toBe(1);
    const e = await merge(b);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: name("slow-merge"), reason: "timeout" });
    expect(stats().PreMerge.timeouts).toBe(1);
    // The timed-out jobs were cancelled, not left running.
    await until(async () => (await hookJobs(b)).every((j) => j.state !== "running" && j.state !== "queued"), "the jobs ended");
  });

  test("plugin_rights_are_invoker_and_grants: a session's event runs as the session ∩ the grants; no JobDone; no project reads", async () => {
    const b = await boot(
      { hooks: [{ event: "PreCommand", handler: "look" }], grants: [{ perm: "project:read" }, observe("PreCommand")], handlers: { look: { calls: [{ method: "project.snapshot" }] } } },
      [userHook("jd", "JobDone", ["--append", join(tmp, "jd.log")])],
    );
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    ok(await edit(a, b.projectId, root));
    const [job] = await until(async () => {
      const js = await hookJobs(b);
      return js.length === 1 && js[0]!.state === "succeeded" ? js : undefined;
    }, "the hook job");
    expect(job!.owner).toEqual({ kind: "agent", id: session.id });
    expect(job!.via).toEqual({ plugin: `${PLUGIN}@1.0.0` });
    expect(job!.project).toBe(b.projectId);
    expect(job!.rights).toMatchObject({ principal: { kind: "agent", id: session.id }, scopes: ["project.read"], projects: [b.projectId], branch: { root, subtree: true } });
    expect((job!.rights as { grants: Permission[] }).grants).toEqual([{ perm: "project:read" }, observe("PreCommand")]);
    // Host calls of a hook job pin no branch: no project reads (least privilege).
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the steer");
    expect(String(payloadOf(steered).text)).toContain('"error":"BRANCH_NOT_ALLOWED"');
    // Its end fires no JobDone (recursion guard, Review Focus 4).
    await settled();
    const jd = join(tmp, "jd.log");
    const kinds = existsSync(jd) ? readFileSync(jd, "utf8").split("\n").filter((l) => l !== "").map((l) => (JSON.parse(l) as { payload: { kind: string } }).payload.kind) : [];
    expect(kinds).not.toContain("plugin.hook");
  });

  test("plugin_hook_job_is_not_submittable_by_rpc: jobs.submit of plugin.hook is JOB_KIND_UNKNOWN", async () => {
    const b = await boot({ hooks: [{ event: "PreCommand", handler: "guard" }], grants: [observe("PreCommand")] });
    const request: HookRequest = { event: "PreCommand", v: 1, project: { id: b.projectId, dir: "" }, payload: {} };
    expect(code(await b.local.call("jobs.submit", { kind: "plugin.hook", params: { handler: "guard", request }, project: b.projectId }))).toBe("JOB_KIND_UNKNOWN");
    expect(await hookJobs(b)).toEqual([]);
  });
});

describe("reach of an event (the source alone)", () => {
  /** The host's plugin hook source over a fresh control.db, its job engine a recorder. */
  async function source(o: { source?: "registry" | "dev" } = {}) {
    const sd = join(tmp, "unit");
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    const control = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
    const tables = new PluginTables(control);
    const dir = join(sd, "plugins", "store", PLUGIN, "1.0.0-t");
    mkdirSync(dirname(dir), { recursive: true });
    cpSync(PROBE, dir, { recursive: true });
    const manifest = JSON.parse(readFileSync(join(dir, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
    writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ ...manifest, hostApi: "^1.2", permissions: [observe("JobDone", "PreCommand")], contributes: { hooks: [{ event: "JobDone", handler: "done" }, { event: "PreCommand", handler: "cmd", timeoutS: 7 }] } }));
    tables.addPlugin({ id: PLUGIN, version: "1.0.0", source: o.source ?? "registry", treeHash: "t", dir: o.source === "dev" ? realpathSync(dir) : `store/${PLUGIN}/1.0.0-t`, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
    tables.setGrants(PLUGIN, "1.0.0", [JSON.stringify(observe("JobDone", "PreCommand"))], "console:console", 0);
    const host = makePluginHost({ serverData: sd, control, tables });
    const submitted: Array<{ owner: unknown; spec: Record<string, unknown> }> = [];
    host.pluginHooks.bind({
      submit: (owner, spec) => Effect.sync(() => (submitted.push({ owner, spec: spec as unknown as Record<string, unknown> }), { id: `j${submitted.length}` }) as unknown as JobView),
      wait: (ids) => Effect.succeed(ids.map((j) => ({ id: j, state: "succeeded", result: { reply: "allow" } }) as unknown as JobView)),
      cancel: () => Effect.die("not cancelled"),
    });
    return { host, submitted, close: () => control.close() };
  }

  const req = (event: HookEvent, projectId?: string): HookRequest => ({ event, v: 1, ...(projectId === undefined ? {} : { project: { id: projectId, dir: "/p" } }), payload: {} });
  const row = (event: HookEvent, handler: string): ConfiguredHook => ({ layer: "plugin", source: PLUGIN, id: handler, event, plugin: PLUGIN });

  test("server_scoped_events_reach_only_rights_over_every_project: system yes, a project's agent no; out-of-reach projects no", async () => {
    const s = await source();
    try {
      expect(s.host.pluginHooks.rows()).toEqual([
        { plugin: PLUGIN, event: "JobDone", handler: "done", timeoutS: 10 },
        { plugin: PLUGIN, event: "PreCommand", handler: "cmd", timeoutS: 7 },
      ]);
      const system: HookAt = { principal: systemInvoker("library") };
      expect(await s.host.pluginHooks.run(row("JobDone", "done"), req("JobDone"), system)).toEqual({ reply: "allow" });
      expect(s.submitted).toHaveLength(1);
      expect(s.submitted[0]!.spec).toMatchObject({ kind: "plugin.hook", params: { handler: "done", request: { event: "JobDone" } }, via: { plugin: `${PLUGIN}@1.0.0` }, priority: "interactive", maxAttempts: 1 });
      expect(s.submitted[0]!.spec["project"]).toBeUndefined();
      // An agent reaching one project: a server-scoped event, and another project's, never reach it.
      const agent: HookAt = { principal: { kind: "agent", id: "s1", scopes: ["project.read"], projects: ["p1"] } };
      expect(await s.host.pluginHooks.run(row("JobDone", "done"), req("JobDone"), agent)).toMatchObject({ notRun: "notReceived" });
      expect(await s.host.pluginHooks.run(row("JobDone", "done"), req("JobDone", "p2"), { ...agent, projectId: "p2" })).toMatchObject({ notRun: "notReceived" });
      // A person reaches every project: a server-scoped event reaches it.
      expect(await s.host.pluginHooks.run(row("JobDone", "done"), req("JobDone"), { principal: CONSOLE })).toEqual({ reply: "allow" });
      expect(s.submitted).toHaveLength(2);
      // A handler the manifest does not declare is not run.
      expect(await s.host.pluginHooks.run(row("PreCommand", "nope"), req("PreCommand"), system)).toMatchObject({ notRun: "notReceived" });
    } finally {
      await s.close();
    }
  });

  test("plugin_hook_params_are_rust_typed: fin-node validates plugin.hook v1 params as HookRun", () => {
    expect(jobKindVersions()).toContainEqual({ kind: "plugin.hook", version: 1 });
    const request: HookRequest = { event: "JobDone", v: 1, payload: {} };
    expect(validateJobParams("plugin.hook", 1, { handler: "done", request })).toMatchObject({ ok: true });
    expect(validateJobParams("plugin.hook", 1, { handler: "done", request: { ...request, event: "PreOp" } })).toMatchObject({ ok: false });
  });

  test("dev_plugin_handlers_never_see_agent_events: a developer plugin's handler gets people's and the system's events only", async () => {
    const s = await source({ source: "dev" });
    try {
      const agent: HookAt = { projectId: "p1", principal: { kind: "agent", id: "s1", scopes: ["project.read"], projects: ["p1"] } };
      expect(await s.host.pluginHooks.run(row("PreCommand", "cmd"), req("PreCommand", "p1"), agent)).toMatchObject({ notRun: "notReceived" });
      expect(await s.host.pluginHooks.run(row("JobDone", "done"), req("JobDone", "p1"), { projectId: "p1", principal: CONSOLE })).toEqual({ reply: "allow" });
      expect(s.submitted).toHaveLength(1);
    } finally {
      await s.close();
    }
  });
});
