// `BranchReady` and `PreMerge` on the `auto` path (7F plan Task 8, Decision 13; spec §9.1): the
// bridge's own `branchReady` listener runs `BranchReady` hooks for every event, its context and
// its block reaching the session through the engine's `SessionNotices` port; the trigger's
// `preMerge(ev, session, item)` awaits the same outcome (one run per event, whichever listener
// asks first) and refuses on its block, then runs `PreMerge` with the item; a merged outcome
// fires `PostMerge`. The engine and hooks are real (the fake hook, `direct`); the session port
// is a recording one here (Task 6 fills the real one). The auto-merge trigger's own cases are in
// `agent-auto-merge.test.ts`.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookEvent, ReviewItem, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { AutoMergeOutcome } from "../src/agent/auto-merge.ts";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import type { BranchReady } from "../src/agent/review-link.ts";
import type { AuditInput } from "../src/control/audit.ts";
import { HookConfig } from "../src/hooks/config.ts";
import { type HookAt, type HookEngineImpl, type HookNotice, type HookOutcome, makeHookEngine } from "../src/hooks/engine.ts";
import { reviewHooks } from "../src/hooks/review.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });

let tmp: string;
const engines: HookEngineImpl[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-review-")));
});
afterEach(async () => {
  for (const e of engines.splice(0)) await e.close();
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });
const ctx = (text: string) => ["--reply", `context:${text}`];
const block = (reason: string) => ["--reply", `block:${reason}`];

const SESSION = { id: "s1", projectId: "p1", harness: "fake", role: "editor", mode: "auto", rootBranch: "b1", credentialId: "c1" } as unknown as Session;
const EV: BranchReady = { sessionId: "s1", projectId: "p1", branch: "b1", headSeq: 3, reviewId: "r1" };
const ITEM = { id: "r1", projectId: "p1", kind: "branchReview", state: "open", source: "b1", target: "main", title: "Ready", rederived: false } as unknown as ReviewItem;

interface Unit {
  readonly serverData: string;
  readonly engine: HookEngineImpl;
  readonly notices: Array<{ at: HookAt; notice: HookNotice }>;
  readonly delivered: Array<{ at: HookAt; event: HookEvent; outcome: HookOutcome }>;
  readonly audits: AuditInput[];
  readonly review: ReturnType<typeof reviewHooks>;
}

/** A real engine over a user layer of `hooks`, a recording session port, the bridge over one session. */
function unit(hooks: unknown[]): Unit {
  const serverData = join(tmp, "server-data");
  const dir = join(tmp, "P");
  mkdirSync(serverData, { recursive: true });
  mkdirSync(dir, { recursive: true });
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks }));
  const config = new HookConfig({ serverData, trustOf: () => undefined, dirOf: (id) => (id === "p1" ? dir : undefined) });
  const notices: Unit["notices"] = [];
  const delivered: Unit["delivered"] = [];
  const audits: AuditInput[] = [];
  const port = { notice: (at: HookAt, notice: HookNotice) => void notices.push({ at, notice }), deliver: (at: HookAt, event: HookEvent, outcome: HookOutcome) => void delivered.push({ at, event, outcome }) };
  const engine = makeHookEngine({ config, serverData, audit: { append: (a) => void audits.push(a) }, notices: port, process: { status: DIRECT } });
  engines.push(engine);
  const review = reviewHooks({ engine, sessions: { get: (id) => (id === SESSION.id ? SESSION : undefined) }, notices: port });
  return { serverData, engine, notices, delivered, audits, review };
}

const preMerge = (u: Unit, ev: BranchReady = EV) => Effect.runPromise(u.review.preMerge(ev, SESSION, ITEM));

describe("BranchReady through the session port (Decision 13)", () => {
  test("branch_ready_context_and_block_reach_the_session: context delivered, a block written as a notice, as the session", async () => {
    const u = unit([hook("say", "BranchReady", ctx("check the colour")), hook("stop", "BranchReady", block("not on Fridays"))]);
    await u.review.branchReady(EV);
    expect(u.delivered).toHaveLength(1);
    expect(u.delivered[0]!.event).toBe("BranchReady");
    expect(u.delivered[0]!.outcome).toMatchObject({ context: "check the colour", contextHooks: ["say"] });
    expect(u.delivered[0]!.outcome.blocked).toBeUndefined();
    expect(u.notices.map((n) => n.notice)).toEqual([{ hook: { event: "BranchReady", id: "stop", layer: "user", blocked: "not on Fridays" } }]);
    const at = u.notices[0]!.at;
    expect(at.projectId).toBe("p1");
    expect(at.session?.id).toBe("s1");
    expect(at.principal).toMatchObject({ kind: "agent", id: "c1" });
    // A blocking event's block is audited as the session's.
    expect(u.audits).toMatchObject([{ kind: "hook.block", principal: { kind: "agent", id: "c1" }, details: { event: "BranchReady", hookId: "stop", session: "s1" } }]);
  });

  test("an event of a session this server does not know runs nothing", async () => {
    const u = unit([hook("say", "BranchReady", ctx("x"))]);
    await u.review.branchReady({ ...EV, sessionId: "nobody" });
    expect(u.engine.stats().events.BranchReady.runs).toBe(0);
  });

  test("branch_ready_runs_once_for_both_listeners: the trigger's point and the listener share one run", async () => {
    const u = unit([hook("say", "BranchReady", ctx("once"))]);
    const [, allowed] = await Promise.all([u.review.branchReady(EV), preMerge(u)]);
    expect(allowed).toBe(true);
    await u.review.branchReady(EV);
    expect(u.engine.stats().events.BranchReady.runs).toBe(1);
    expect(u.delivered.filter((d) => d.event === "BranchReady")).toHaveLength(1);
    // Another event (another object) runs again.
    await u.review.branchReady({ ...EV });
    expect(u.engine.stats().events.BranchReady.runs).toBe(2);
  });
});

describe("PreMerge at the auto merge point", () => {
  test("a BranchReady block refuses the merge before any PreMerge hook runs", async () => {
    const u = unit([hook("stop", "BranchReady", block("no")), hook("pm", "PreMerge", [])]);
    expect(await preMerge(u)).toBe(false);
    expect(u.engine.stats().events.PreMerge.runs).toBe(0);
  });

  test("pre_merge_sees_item: the payload names the item, its source and target", async () => {
    const u = unit([hook("look", "PreMerge", ["--report", "stdin"])]);
    expect(await preMerge(u)).toBe(true);
    const seen = u.delivered.find((d) => d.event === "PreMerge");
    expect(seen).toBeDefined();
    const req = JSON.parse((JSON.parse(seen!.outcome.context!) as { stdin: string }).stdin) as { event: string; session: { id: string }; payload: Record<string, any> }; // eslint-disable-line @typescript-eslint/no-explicit-any
    expect(req.event).toBe("PreMerge");
    expect(req.session.id).toBe("s1");
    expect(req.payload).toMatchObject({ via: "autoMerge", sessionId: "s1", from: "b1", into: "main", item: { id: "r1", kind: "branchReview", source: "b1", target: "main", title: "Ready" } });
  });

  test("a PreMerge block refuses, with a notice in the session and a hook.block row", async () => {
    const u = unit([hook("guard", "PreMerge", block("needs a person"))]);
    expect(await preMerge(u)).toBe(false);
    expect(u.notices.map((n) => n.notice)).toEqual([{ hook: { event: "PreMerge", id: "guard", layer: "user", blocked: "needs a person" } }]);
    expect(u.audits).toMatchObject([{ kind: "hook.block", details: { event: "PreMerge", hookId: "guard" } }]);
  });

  test("afterMerge: a merged outcome fires PostMerge once; a refusal fires nothing", async () => {
    const u = unit([hook("after", "PostMerge", ["--report", "stdin"])]);
    let next: AutoMergeOutcome = { merged: false, reason: "verdictMissing" };
    const trigger = u.review.afterMerge((_ev: BranchReady) => Effect.succeed(next));
    await Effect.runPromise(trigger(EV));
    await u.engine.settled();
    expect(u.engine.stats().events.PostMerge.runs).toBe(0);
    next = { merged: true, mergeId: "m1" };
    expect(await Effect.runPromise(trigger(EV))).toEqual({ merged: true, mergeId: "m1" });
    await u.engine.settled();
    expect(u.engine.stats().events.PostMerge.runs).toBe(1);
    const req = JSON.parse((JSON.parse(u.delivered.find((d) => d.event === "PostMerge")!.outcome.context!) as { stdin: string }).stdin) as { payload: Record<string, unknown> };
    expect(req.payload).toMatchObject({ via: "autoMerge", sessionId: "s1", reviewId: "r1", mergeId: "m1", from: "b1" });
  });
});

describe("on a real server", () => {
  const hooksEnv = process.env.FIN_TEST_HOOKS;
  beforeAll(() => {
    process.env.FIN_TEST_HOOKS = "1";
  });
  afterAll(() => {
    if (hooksEnv === undefined) delete process.env.FIN_TEST_HOOKS;
    else process.env.FIN_TEST_HOOKS = hooksEnv;
  });
  let server: AgentServer | undefined;
  afterEach(async () => {
    await server?.stop();
    server = undefined;
  });
  const WORKING: FakeScript = { name: "working", turns: [[{ emit: [{ type: "message", role: "assistant", blocks: [{ type: "text", text: "working" }] }] }, { awaitAnswer: "never" }]] };
  let n = 0;
  const id = () => `01H${String(++n).padStart(23, "0")}`;
  const edit = async (c: NdjsonClient, projectId: string, on: string, at = 0) => ok(await commit(c, projectId, { ...tx(id(), 1, [insert(at)]), branch: on }));

  test("branch_ready_runs_for_every_event: a draft session's review request and its end with unmerged commits", async () => {
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
    const marker = join(tmp, "ran");
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: [hook("ready", "BranchReady", ["--write", marker])] }));
    server = new AgentServer(serverData, { timers: false, fakeScripts: [WORKING], hooks: { status: DIRECT } });
    await server.start();
    const local = await server.local();
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
    const { session } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go", mode: "draft" }));
    const a = await server.agent(credentialPath(server.core.store.root, session.id));
    const root = session.rootBranch!;
    const tryBranch = ok<{ branch: { id: string } }>(await a.call("branch.create", { projectId, txId: id(), author: "x", from: root, name: "try-1" })).branch.id;
    await edit(a, projectId, tryBranch);
    ok(await a.call("review.request", { projectId, source: tryBranch, title: "Ready" }));
    await server.core.reviewLink.settled();
    expect(server.agents.hookWiring.engine.stats().events.BranchReady.runs).toBe(1);
    await edit(a, projectId, root, 1);
    ok(await local.call("agent.session.end", { sessionId: session.id }));
    await server.core.reviewLink.settled();
    expect(server.agents.hookWiring.engine.stats().events.BranchReady.runs).toBe(2);
    expect(existsSync(marker)).toBe(true);
  });
});
