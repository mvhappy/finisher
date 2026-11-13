// Chain events (7F plan Task 7; Decisions 1, 14, 15; spec §9.1): `PreCommand` is a stage-5
// policy rule after the command rules (a rule's `deny` or `ask` comes first, a hook never sees a
// call the rules refuse), for `agent` principals and for everyone when a hook says `principals:
// "all"`; a block is `HOOK_BLOCKED` (exit 7), audited `hook.block`, and gives back a one-time
// grant consumed for the call. `PostCommand` runs off the call's path after a call that answered:
// its context and its block's reason reach the session (steered into a running turn, else
// held), and nothing it does changes the call's answer. `PreMerge`/`PostMerge` on `history.merge`
// and `review.merge` for every principal. A real server, the fake hook (`direct`), sessions on
// the scripted `fake` harness (`FIN_TEST_HOOKS=1`).
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookEvent, Node, ReviewItem, ReviewView, Rule, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import { controlDbPath } from "../src/control/db.ts";
import { CONSOLE } from "../src/principal.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });

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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-chain-")));
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
/** Every turn answers and ends (the session goes `idle`). */
const ENDS: FakeScript = { name: "ends", turns: [[{ emit: [msg("ok"), { type: "turnCompleted", stopReason: "endTurn" }] }]] };

/** The fake hook in the user layer. */
const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });
const block = (reason: string) => ["--reply", `block:${reason}`];
const context = (text: string) => ["--reply", `context:${text}`];
/** Appends each request it gets to `<tmp>/<name>.log`. */
const rec = (name: string) => ["--append", join(tmp, `${name}.log`)];
const requests = (name: string): Array<{ event: HookEvent; session?: { id: string }; payload: Record<string, any> }> => { // eslint-disable-line @typescript-eslint/no-explicit-any
  const p = join(tmp, `${name}.log`);
  return existsSync(p) ? readFileSync(p, "utf8").split("\n").filter((l) => l !== "").map((l) => JSON.parse(l)) : [];
};
const rule = (id: string, effect: "allow" | "ask" | "deny", command: string) => ({ id, effect, command });

/** `<server-data>/settings.json`: the user layer's hooks and rules. */
function settings(o: { hooks?: unknown[]; rules?: unknown[] }): void {
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: o.hooks ?? [], ...(o.rules === undefined ? {} : { agents: { rules: o.rules } }) }));
}

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

interface Booted {
  readonly local: NdjsonClient;
  readonly projectId: string;
}

/** A server whose user layer holds `hooks` (and `rules`), a project with an asset on `main`. */
async function boot(o: { hooks?: unknown[]; rules?: unknown[]; scripts?: FakeScript[] } = {}): Promise<Booted> {
  settings(o);
  server = new AgentServer(serverData, { timers: false, fakeScripts: o.scripts ?? [WORKING], hooks: { status: DIRECT } });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

async function until<A>(f: () => A | undefined | false, what: string, ms = 10_000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
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

/** A session `local` started on the fake harness, in its first run; its credential's client. */
async function started(b: Booted, running = true) {
  const { session } = ok<{ session: Session }>(await b.local.call("agent.session.start", { projectId: b.projectId, harness: "fake", prompt: "go" }));
  await until(() => (running ? status(session.id) === "running" : status(session.id) === "idle" && srv().core.store.runs(session.id).length === 1), `${session.id} ${running ? "running" : "idle"}`);
  return { session, a: await srv().agent(credentialPath(srv().core.store.root, session.id)) };
}

const branches = async (c: NdjsonClient, projectId: string) => ok<{ branches: Array<{ id: string; name?: string; head: number }> }>(await c.call("branch.list", { projectId, all: true })).branches;
const head = async (c: NdjsonClient, projectId: string, branch: string) => (await branches(c, projectId)).find((b) => b.id === branch || b.name === branch)!.head;
/** A clip on `on`, as `c`. */
const edit = (c: NdjsonClient, projectId: string, on: string, at = 0) => commit(c, projectId, { ...tx(id(), 1, [insert(at)]), branch: on });
/** `fin item trim`'s transaction (blocked before the store sees it). */
const trim = (c: NdjsonClient, projectId: string, on: string) => commit(c, projectId, { ...tx(id(), 1, [{ op: "clip.trim", clip: "c1", sourceIn: "0", duration: "1" }]), branch: on });
const checkpoint = (c: NdjsonClient, projectId: string, branch: string, more: Record<string, unknown> = {}) => c.call("history.checkpoint", { projectId, branch, name: "c1", txId: id(), author: "x", ...more });
const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

/** `hook.block` rows: their principal and details. */
function hookBlocks(): Array<{ principalKind: string; principalId: string; details: Record<string, unknown> }> {
  const db = new DatabaseSync(controlDbPath(serverData));
  try {
    const rows = db.prepare("SELECT principalKind, principalId, details FROM audit WHERE kind = 'hook.block' ORDER BY id").all() as Array<{ principalKind: string; principalId: string; details: string }>;
    return rows.map((r) => ({ ...r, details: JSON.parse(r.details) as Record<string, unknown> }));
  } finally {
    db.close();
  }
}

describe("PreCommand (Decision 15)", () => {
  test("pre_command_blocks_agent_call: fin item trim from a session is HOOK_BLOCKED, nothing written, one hook.block row", async () => {
    const b = await boot({ hooks: [hook("no-trim", "PreCommand", block("trims need a person"), { match: "item.trim" })] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    const e = await trim(a, b.projectId, root);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "no-trim", reason: "trims need a person", event: "PreCommand", layer: "user" });
    expect(await head(b.local, b.projectId, root)).toBe(before);
    expect(hookBlocks()).toEqual([{ principalKind: "agent", principalId: session.id, details: { event: "PreCommand", hookId: "no-trim", layer: "user", projectId: b.projectId, session: session.id } }]);
    // Another command does not match: it goes through.
    ok(await edit(a, b.projectId, root));
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    expect(stats().PreCommand).toMatchObject({ runs: 1, blocks: 1 });
  });

  test("pre_command_payload_names_commands: {rpc, command, commands, payload}, the session and its project", async () => {
    const b = await boot({ hooks: [hook("look", "PreCommand", [...rec("look"), ...block("no")], { match: "item.trim" })] });
    const { session, a } = await started(b);
    expect(code(await trim(a, b.projectId, session.rootBranch!))).toBe("HOOK_BLOCKED");
    const [req] = requests("look");
    expect(req!.event).toBe("PreCommand");
    expect(req!.session?.id).toBe(session.id);
    expect(req!.payload).toMatchObject({ rpc: "tx.commit", command: "clip.trim", commands: ["clip.trim", "item.trim", "rpc:tx.commit"], payload: { projectId: b.projectId, tx: { ops: [{ op: "clip.trim", clip: "c1" }] } } });
  });

  test("rules_decide_before_hooks: a deny answers RULE_DENIED and an ask opens its card; the hook never runs", async () => {
    const b = await boot({ hooks: [hook("look", "PreCommand", rec("look"))], rules: [rule("no-cp", "deny", "checkpoint")] });
    const { session, a } = await started(b);
    expect(code(await checkpoint(a, b.projectId, session.rootBranch!))).toBe("RULE_DENIED");
    expect(requests("look")).toEqual([]);
    settings({ hooks: [hook("look", "PreCommand", rec("look"))], rules: [rule("ask-cp", "ask", "checkpoint")] });
    const asked = await checkpoint(a, b.projectId, session.rootBranch!);
    expect(code(asked)).toBe("APPROVAL_PENDING");
    expect(srv().core.store.approval(String(details(asked)!.approval))?.status).toBe("open");
    expect(requests("look")).toEqual([]);
    // A call no rule refuses reaches the hook.
    ok(await edit(a, b.projectId, session.rootBranch!));
    expect(requests("look").map((r) => r.payload.rpc)).toEqual(["snapshot.get", "tx.commit"]);
  });

  test("pre_command_people_only_with_all: a person's calls fire only hooks that say principals: all", async () => {
    const b = await boot();
    settings({ hooks: [hook("agents", "PreCommand", rec("agents")), hook("everyone", "PreCommand", rec("everyone"), { principals: "all" })] });
    ok(await checkpoint(b.local, b.projectId, "main"));
    expect(requests("agents")).toEqual([]);
    expect(requests("everyone").map((r) => [r.payload.rpc, r.session])).toEqual([["history.checkpoint", undefined]]);
    settings({ hooks: [hook("everyone", "PreCommand", block("people too"), { principals: "all" })] });
    const e = await checkpoint(b.local, b.projectId, "main");
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(hookBlocks()).toMatchObject([{ principalKind: "local", principalId: "local", details: { event: "PreCommand", hookId: "everyone" } }]);
  });

  test("dry_run_still_fires_pre_command: a dry run that an ask lets through is still blocked by a hook", async () => {
    const b = await boot({ hooks: [hook("guard", "PreCommand", block("not even a plan"), { match: "checkpoint" })], rules: [rule("ask-cp", "ask", "checkpoint")] });
    const { session, a } = await started(b);
    const e = await checkpoint(a, b.projectId, session.rootBranch!, { dryRun: true });
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "guard", reason: "not even a plan" });
  });

  test("pre_command_context_reaches_the_session: an allow with context is steered into the running turn", async () => {
    const b = await boot({ hooks: [hook("hint", "PreCommand", context("checkpoints are cheap"), { match: "checkpoint" })] });
    const { session, a } = await started(b);
    ok(await checkpoint(a, b.projectId, session.rootBranch!));
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the steer");
    expect(payloadOf(steered)).toMatchObject({ hookContext: { event: "PreCommand", hooks: ["hint"] } });
    expect(String(payloadOf(steered).text)).toContain("checkpoints are cheap");
  });

  test("blocked_write_releases_one_time_grant: a grant consumed for the call is given back when a hook blocks it", async () => {
    const b = await boot();
    const opened = await open(srv().core, b.local, b.projectId);
    const owner = await srv().console();
    ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "checkpoint", sessionId: opened.session.id }));
    srv().core.store.startRun(opened.session.id, { trigger: "prompt" });
    const a = await srv().agent(opened.credentialPath);
    const root = opened.session.rootBranch!;
    const call = (txId = id()) => a.call("history.checkpoint", { projectId: b.projectId, branch: root, name: "c1", txId, author: "x" });
    const approvalId = String(details(await call())!.approval);
    ok(await confirmed(owner, "agent.approval.answer", { id: approvalId, decision: "allow" }));
    settings({ hooks: [hook("guard", "PreCommand", block("frozen"), { match: "checkpoint" })] });
    const before = await head(b.local, b.projectId, root);
    expect(code(await call())).toBe("HOOK_BLOCKED");
    expect(srv().core.store.approval(approvalId)!.consumedAt).toBeUndefined();
    expect(await head(b.local, b.projectId, root)).toBe(before);
    // Without the hook the same request uses the grant, once.
    settings({ hooks: [] });
    ok(await call());
    expect(srv().core.store.approval(approvalId)!.consumedAt).toBeTypeOf("number");
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
  });

  test("hook_fin_call_fires_no_hooks_end_to_end: a principals: all hook's own fin view goes through the chain and fires nothing", { timeout: 60_000 }, async () => {
    const b = await boot();
    const { session, a } = await started(b);
    settings({ hooks: [hook("guard", "PreCommand", [...rec("guard"), "--fin", JSON.stringify(["view"])], { fin: true, principals: "all", timeoutS: 30 })] });
    ok(await a.call("snapshot.get", { projectId: b.projectId, branch: session.rootBranch! }));
    // The hook ran once, for the agent's call: none of its own `fin view`'s calls (all of them
    // project calls through the chain, any of which it would match) fired it again.
    expect(requests("guard")).toHaveLength(1);
    expect(requests("guard")[0]!.session?.id).toBe(session.id);
    expect(stats().PreCommand.runs).toBe(1);
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the hook's observation");
    expect(String(payloadOf(steered).text)).toContain('"fin":{"code":0}');
    expect(requests("guard")).toHaveLength(1);
  });
});

describe("PostCommand (Decisions 1, 15, 16)", () => {
  test("post_command_after_success_only: a refused or failed call fires nothing; one that answered fires once", async () => {
    const b = await boot({ hooks: [hook("after", "PostCommand", rec("after"), { match: "checkpoint" })] });
    const { session, a } = await started(b);
    expect(code(await checkpoint(a, b.projectId, "no-such-branch"))).toBeDefined();
    await settled();
    expect(requests("after")).toEqual([]);
    ok(await checkpoint(a, b.projectId, session.rootBranch!));
    await settled();
    expect(requests("after").map((r) => [r.event, r.payload.rpc, r.payload.command])).toEqual([["PostCommand", "history.checkpoint", "checkpoint"]]);
  });

  test("post_command_never_fails_the_call: a hook that crashes leaves the answer standing", async () => {
    const b = await boot({ hooks: [hook("crash", "PostCommand", ["--kill"], { onTimeout: "block" })] });
    const { session, a } = await started(b);
    const before = await head(b.local, b.projectId, session.rootBranch!);
    ok(await edit(a, b.projectId, session.rootBranch!));
    await settled();
    expect(await head(b.local, b.projectId, session.rootBranch!)).toBe(before + 1);
    expect(stats().PostCommand.failures).toBe(2);
  });

  test("post_command_context_steers_running_session: a userMessage {trigger: hook} in the active run, no new run", async () => {
    const b = await boot({ hooks: [hook("lint", "PostCommand", context("lint: 3 warnings"), { match: "checkpoint" })] });
    const { session, a } = await started(b);
    const [r] = srv().core.store.runs(session.id);
    ok(await checkpoint(a, b.projectId, session.rootBranch!));
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the steer");
    expect(steered.runId).toBe(r!.id);
    expect(payloadOf(steered)).toMatchObject({ hookContext: { event: "PostCommand", hooks: ["lint"] } });
    expect(srv().core.store.runs(session.id)).toHaveLength(1);
  });

  test("post_command_context_held_when_idle: no run starts; delivered with the next prompt", async () => {
    const b = await boot({ hooks: [hook("lint", "PostCommand", context("lint: 3 warnings"), { match: "checkpoint" })], scripts: [ENDS] });
    const { session, a } = await started(b, false);
    ok(await checkpoint(a, b.projectId, session.rootBranch!));
    await settled();
    await new Promise((r) => setTimeout(r, 100));
    expect(srv().core.store.runs(session.id)).toHaveLength(1);
    expect(status(session.id)).toBe("idle");
    ok(await b.local.call("agent.session.prompt", { sessionId: session.id, text: "next" }));
    await until(() => srv().core.store.runs(session.id).length === 2, "the second run");
    const second = srv().core.store.runs(session.id)[1]!;
    const held = await until(() => nodes(session.id).find((x) => x.runId === second.id && x.kind === "notice" && payloadOf(x).hookContext !== undefined), "the held context");
    expect(payloadOf(held).hookContext).toMatchObject({ event: "PostCommand", hooks: ["lint"], text: "lint: 3 warnings" });
  });

  test("post_command_block_reports_to_the_agent: the write stays, later hooks do not run, a notice and the reason in the run", async () => {
    const b = await boot({ hooks: [hook("guard", "PostCommand", block("frozen"), { match: "clip.insert" }), hook("later", "PostCommand", rec("later"))] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const before = await head(b.local, b.projectId, root);
    ok(await edit(a, b.projectId, root));
    await settled();
    expect(await head(b.local, b.projectId, root)).toBe(before + 1);
    // `later` ran for the commit's `snapshot.get` only, never for the blocked commit.
    expect(requests("later").map((r) => r.payload.rpc)).toEqual(["snapshot.get"]);
    const [r] = srv().core.store.runs(session.id);
    const notice = await until(() => nodes(session.id).find((x) => x.kind === "notice" && payloadOf(x).hook?.blocked !== undefined), "the notice");
    expect(notice.runId).toBe(r!.id);
    expect(payloadOf(notice).hook).toEqual({ event: "PostCommand", id: "guard", layer: "user", blocked: "frozen" });
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).trigger === "hook"), "the reason");
    expect(payloadOf(steered)).toMatchObject({ hookContext: { event: "PostCommand", hooks: ["guard"] } });
    expect(String(payloadOf(steered).text)).toContain("The PostCommand hook guard blocked this: frozen");
    expect(hookBlocks()).toEqual([{ principalKind: "agent", principalId: session.id, details: { event: "PostCommand", hookId: "guard", layer: "user", projectId: b.projectId, session: session.id, after: true } }]);
  });
});

describe("PreMerge and PostMerge on RPCs (Decision 14)", () => {
  /** A branch `feat` off `main` with one commit, as `local`. */
  async function feature(b: Booted): Promise<string> {
    const feat = ok<{ branch: { id: string } }>(await b.local.call("branch.create", { projectId: b.projectId, txId: id(), author: "x", from: "main", name: "feat" })).branch.id;
    ok(await edit(b.local, b.projectId, feat));
    return feat;
  }
  const merge = (b: Booted, from: string) => b.local.call("history.merge", { projectId: b.projectId, txId: id(), author: "x", from, into: "main" });

  test("pre_merge_blocks_history_merge_by_a_person: HOOK_BLOCKED, main unchanged, audited as the person", async () => {
    const b = await boot({ hooks: [hook("guard", "PreMerge", [...rec("guard"), ...block("release freeze")])] });
    const feat = await feature(b);
    const before = await head(b.local, b.projectId, "main");
    const e = await merge(b, feat);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "guard", reason: "release freeze", event: "PreMerge" });
    expect(await head(b.local, b.projectId, "main")).toBe(before);
    expect(requests("guard").map((r) => r.payload)).toEqual([{ via: "rpc", rpc: "history.merge", from: feat, into: "main" }]);
    expect(hookBlocks()).toMatchObject([{ principalKind: "local", details: { event: "PreMerge", hookId: "guard", projectId: b.projectId } }]);
  });

  test("pre_merge_timeout_blocks_merge: a PreMerge hook that times out blocks by default", { timeout: 30_000 }, async () => {
    const b = await boot({ hooks: [hook("slow", "PreMerge", ["--sleep", "5000"], { timeoutS: 1 })] });
    const feat = await feature(b);
    const e = await merge(b, feat);
    expect(code(e)).toBe("HOOK_BLOCKED");
    expect(details(e)).toMatchObject({ hook: "slow", reason: "timeout" });
    expect(stats().PreMerge.timeouts).toBe(1);
  });

  test("post_merge_fires_once_per_merge: history.merge, once, after the merge", async () => {
    const b = await boot({ hooks: [hook("pre", "PreMerge", rec("pre")), hook("post", "PostMerge", rec("post"))] });
    const feat = await feature(b);
    const merged = ok<{ mergeId: string }>(await merge(b, feat));
    await settled();
    expect(requests("pre")).toHaveLength(1);
    expect(requests("post").map((r) => r.payload)).toEqual([{ via: "rpc", rpc: "history.merge", from: feat, into: "main", mergeId: merged.mergeId }]);
  });

  test("pre_merge_on_review_merge_has_item_fields: the item read through the reviews workflow; PostMerge once, none from the auto path", async () => {
    const b = await boot({ hooks: [hook("pre", "PreMerge", rec("pre")), hook("post", "PostMerge", rec("post"))] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const t1 = id();
    ok(await commit(a, b.projectId, { ...tx(t1, 1, [insert(0)]), branch: root }));
    const { item } = ok<{ item: ReviewItem }>(await a.call("review.request", { projectId: b.projectId, source: root, title: "Ready" }));
    await srv().core.reviewLink.settled();
    const decided = ok<ReviewView>(await b.local.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [{ txId: t1, decision: "accept" }] }));
    ok(await b.local.call("review.merge", { itemId: item.id, expect: decided.item.snapshot, idemKey: "m1" }));
    await settled();
    const [pre] = requests("pre");
    expect(requests("pre")).toHaveLength(1);
    expect(pre!.session).toBeUndefined();
    expect(pre!.payload).toMatchObject({ via: "rpc", rpc: "review.merge", from: root, into: item.target, item: { id: item.id, kind: item.kind, title: "Ready", source: root, target: item.target } });
    expect(pre!.payload.item.snapshot).toBeDefined();
    const post = requests("post");
    expect(post).toHaveLength(1);
    expect(post[0]!.payload).toMatchObject({ via: "rpc", rpc: "review.merge", itemId: item.id, from: root, into: item.target });
    expect(post[0]!.payload.mergeId).toBeTypeOf("string");
  });

  test("dry_run_merge_fires_no_post_merge: the command hooks still run; a real merge then fires it once", async () => {
    const b = await boot();
    const feat = await feature(b);
    settings({ hooks: [hook("post", "PostMerge", rec("post")), hook("cmd", "PostCommand", rec("cmd"), { principals: "all", match: "rpc:history.merge" })] });
    const planned = ok<{ dryRun: boolean }>(await b.local.call("history.merge", { projectId: b.projectId, txId: id(), author: "x", from: feat, into: "main", dryRun: true }));
    expect(planned.dryRun).toBe(true);
    await settled();
    expect(requests("post")).toEqual([]);
    expect(requests("cmd").map((r) => r.payload.payload.dryRun)).toEqual([true]);
    ok(await merge(b, feat));
    await settled();
    expect(requests("post")).toHaveLength(1);
    expect(requests("cmd")).toHaveLength(2);
  });

  test("merge_retries_fire_post_merge_once: a history.merge retried with its txId answers the same merge, no second event", async () => {
    const b = await boot({ hooks: [hook("post", "PostMerge", rec("post"))] });
    const feat = await feature(b);
    const txId = id();
    const again = () => b.local.call("history.merge", { projectId: b.projectId, txId, author: "x", from: feat, into: "main" });
    const first = ok<{ mergeId: string }>(await again());
    const second = ok<{ mergeId: string }>(await again());
    expect(second.mergeId).toBe(first.mergeId);
    await settled();
    expect(requests("post").map((r) => r.payload.mergeId)).toEqual([first.mergeId]);
  });

  test("review_merge_retry_fires_no_second_pre_or_post_merge: the same idemKey answers the stored merge", async () => {
    const b = await boot({ hooks: [hook("pre", "PreMerge", rec("pre")), hook("post", "PostMerge", rec("post"))] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const t1 = id();
    ok(await commit(a, b.projectId, { ...tx(t1, 1, [insert(0)]), branch: root }));
    const { item } = ok<{ item: ReviewItem }>(await a.call("review.request", { projectId: b.projectId, source: root, title: "Ready" }));
    await srv().core.reviewLink.settled();
    const decided = ok<ReviewView>(await b.local.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [{ txId: t1, decision: "accept" }] }));
    const once = () => b.local.call("review.merge", { itemId: item.id, expect: decided.item.snapshot, idemKey: "m1" });
    const first = ok<{ merge: { mergeId: string } }>(await once());
    const second = ok<{ merge: { mergeId: string } }>(await once());
    expect(second.merge.mergeId).toBe(first.merge.mergeId);
    await settled();
    expect(requests("pre")).toHaveLength(1);
    expect(requests("post").map((r) => r.payload.mergeId)).toEqual([first.merge.mergeId]);
  });
});

describe("server-scoped calls and stage 3's project (Task 7 review)", () => {
  test("server_scoped_pre_command_for_an_agent_and_a_person: no project, the managed and user layers only", { timeout: 30_000 }, async () => {
    const b = await boot();
    const { file } = srv().core.credentials.base.credentials.mint(CONSOLE, { kind: "agent", label: "a script", projects: [b.projectId] });
    const proved = await srv().proveAs(file);
    ok(proved.exit);
    const script = proved.client;
    const local = join(serverData, "projects", b.projectId);
    mkdirSync(local, { recursive: true });
    writeFileSync(join(local, "hooks.json"), JSON.stringify({ hooks: [hook("local", "PreCommand", rec("local"), { principals: "all" })] }));
    settings({
      hooks: [
        hook("agents", "PreCommand", rec("agents"), { match: "rpc:auth.whoami" }),
        hook("everyone", "PreCommand", rec("everyone"), { principals: "all" }),
        hook("finny", "PreCommand", [], { fin: true, match: "rpc:auth.whoami" }),
      ],
    });
    ok(await script.call("auth.whoami", {}));
    expect(requests("agents").map((r) => [r.payload.rpc, "project" in r, r.session])).toEqual([["auth.whoami", false, undefined]]);
    // A `fin` hook needs a project for its credential: a server-scoped event fails it `unavailable`
    // (Decision 3: `allow` by default), and nothing is minted.
    expect(stats().PreCommand.failures).toBe(1);
    expect(srv().core.credentials.base.credentials.list(true).filter((c) => c.label.startsWith("hook "))).toEqual([]);
    ok(await b.local.call("auth.whoami", {}));
    ok(await b.local.call("project.create", { dir: join(tmp, "Q"), name: "Q", rate: "30" }));
    expect(requests("agents")).toHaveLength(1);
    expect(requests("everyone").map((r) => [r.payload.rpc, "project" in r])).toEqual([
      ["auth.whoami", false],
      ["auth.whoami", false],
      ["project.create", false],
    ]);
    // The project's local layer never runs for a call of no project.
    expect(requests("local")).toEqual([]);
  });

  test("project_open_fires_pre_and_post_command_in_its_project: stage 3's project reaches stage 7", async () => {
    const b = await boot();
    settings({ hooks: [hook("pre", "PreCommand", rec("pre"), { principals: "all", match: "rpc:project.open" }), hook("post", "PostCommand", rec("post"), { principals: "all", match: "rpc:project.open" })] });
    ok(await b.local.call("project.open", { dir: join(tmp, "P") }));
    await settled();
    expect(requests("pre").map((r) => (r as { project?: { id: string } }).project?.id)).toEqual([b.projectId]);
    expect(requests("post").map((r) => (r as { project?: { id: string } }).project?.id)).toEqual([b.projectId]);
  });
});

describe("BranchReady through the real session notices (Task 8's owed case)", () => {
  test("branch_ready_block_and_context_are_nodes_in_the_active_run", async () => {
    const b = await boot({ hooks: [hook("say", "BranchReady", context("check the colour")), hook("stop", "BranchReady", block("not on Fridays"))] });
    const { session, a } = await started(b);
    const root = session.rootBranch!;
    const [r] = srv().core.store.runs(session.id);
    ok(await edit(a, b.projectId, root));
    ok(await a.call("review.request", { projectId: b.projectId, source: root, title: "Ready" }));
    await srv().core.reviewLink.settled();
    const notice = await until(() => nodes(session.id).find((x) => x.kind === "notice" && payloadOf(x).hook?.event === "BranchReady"), "the block notice");
    expect(notice.runId).toBe(r!.id);
    expect(payloadOf(notice).hook).toEqual({ event: "BranchReady", id: "stop", layer: "user", blocked: "not on Fridays" });
    const steered = await until(() => nodes(session.id).find((x) => x.kind === "userMessage" && payloadOf(x).hookContext?.event === "BranchReady"), "the context");
    expect(steered.runId).toBe(r!.id);
    expect(payloadOf(steered)).toMatchObject({ trigger: "hook", hookContext: { event: "BranchReady", hooks: ["say"] } });
    expect(String(payloadOf(steered).text)).toContain("check the colour");
  });
});

describe("handlers.ts takes call-site lines only", () => {
  test("within its budget", () => {
    const lines = readFileSync(join(import.meta.dirname, "..", "src", "handlers.ts"), "utf8").split("\n").length;
    expect(lines).toBeLessThanOrEqual(595);
  });
});
