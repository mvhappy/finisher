// Subagents (sub-project 7B Task 6, Decisions 16–21; spec §5.5, §6.3; Review Focus 1, 4): a
// running session starts children on child branches (`agent.session.start` from its own
// credential), drives its own children only, is bounded in depth, live descendants and the
// server's live slots, hears a child's end as a `subagent` node, waits on children, and its end
// or interrupt cascades. Against a real server and `control.db`, sessions on the scripted `fake`
// harness (a test-hooks build run with `FIN_TEST_HOOKS=1`).
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ChildResult, CredentialFile, Node, Run, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { credentialPath, MODE_SCOPES } from "../src/agent/credentials.ts";
import type { FakeAdapter, FakeScript } from "../src/agent/fake.ts";
import { AgentJournal } from "../src/agent/journal.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import type { AgentServeOptions } from "../src/agent/wiring.ts";
import type { ToolOutcome } from "../src/agent/runtime-deps.ts";
import { argsHash, subjectOf } from "../src/agent/approvals.ts";
import { exitCodeOf } from "../src/console/exit-codes.ts";
import { openControlStore } from "../src/control/db.ts";
import { agentEntries, flush, openStores, P1 } from "./support/agent-journal.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let server: AgentServer;
const hooks = process.env.FIN_TEST_HOOKS;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
const done = { type: "turnCompleted", stopReason: "endTurn" };
/** The parent's script: its first run answers and goes on (a parent spawns from its run). */
const PARENT: FakeScript = { name: "parent", turns: [[{ emit: [msg("on it")] }, { awaitAnswer: "never" }]] };
/** A run that never ends by itself (the session stays `running`). */
const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };
/** A brief that runs to its end: the child ends itself with this summary. */
const brief = (text: string): FakeScript => ({ name: "brief", turns: [[{ emit: [msg(text), done] }]] });
/** A brief cut short (`maxTokens`): the child is idle; its next run stays open. */
/** A run that makes one tool call (the runtime runs it: the test's `fakeTools`), then goes on. */
const TOOL: FakeScript = {
  name: "tool",
  turns: [[{ emit: [{ type: "toolStarted", providerId: "t1", tool: "fin", input: { argv: ["view"] } }] }, { awaitTool: "t1" }, { emit: [msg("after the call")] }, { awaitAnswer: "never" }]],
};
const IDLE: FakeScript = {
  name: "idle",
  turns: [[{ emit: [msg("half"), { type: "turnCompleted", stopReason: "maxTokens" }] }], [{ emit: [msg("more")] }, { awaitAnswer: "never" }]],
};

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-subagents-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

/** A server whose fake opens `scripts` in order (the last repeats); `agents` settings beside. */
async function boot(scripts: readonly FakeScript[], agents?: Record<string, unknown>, options: AgentServeOptions & { privacy?: object } = {}) {
  const { privacy, ...rest } = options;
  if (agents !== undefined || privacy !== undefined) writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents, privacy }));
  server = new AgentServer(serverData, { timers: false, fakeScripts: scripts, ...rest });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

const get = (sessionId: string) => server.core.store.get(sessionId)!;

async function until<A>(f: () => A | undefined | false, what: string, ms = 8000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

const status = (sessionId: string, want: Session["status"]) => until(() => get(sessionId).status === want, `${sessionId} ${want}`);

/** A root session started by a person, in its first run (or idle without a prompt); its credential's client. */
async function root(local: NdjsonClient, projectId: string, more: Record<string, unknown> = {}) {
  const { session } = ok<{ session: Session }>(await local.call("agent.session.start", JSON.parse(JSON.stringify({ projectId, harness: "fake", prompt: "go", ...more }))));
  await until(() => get(session.id).status === "running" || get(session.id).status === "idle", `${session.id} started`);
  return { session, client: await server.agent(credentialPath(server.core.store.root, session.id)) };
}

const spawnRaw = (parent: NdjsonClient, projectId: string, role: string, more: Record<string, unknown> = {}) =>
  parent.call("agent.session.start", { projectId, role, prompt: `brief for a ${role}`, ...more });

/** A child of `parent`: its session, first run and credential's client (none once a brief ended it). */
async function spawn(parent: NdjsonClient, projectId: string, role = "fixer", more: Record<string, unknown> = {}) {
  const r = ok<{ session: Session; run?: Run }>(await spawnRaw(parent, projectId, role, more));
  const ended = () => undefined as unknown as NdjsonClient;
  const client = await server.agent(credentialPath(server.core.store.root, r.session.id)).catch(ended);
  return { session: r.session, run: r.run, client };
}

const branches = async (c: NdjsonClient, projectId: string) =>
  ok<{ branches: Array<{ id: string; name: string; parent?: string; head: number }> }>(await c.call("branch.list", { projectId, all: true })).branches;

const write = (c: NdjsonClient, projectId: string, branch: string, at = 0) => commit(c, projectId, { ...tx(id(), 1, [insert(at)]), branch });

const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

const nodes = (sessionId: string): Node[] => server.core.store.nodes(sessionId, 1, 1000);

const fake = () => server.agents.adapters.get("fake") as unknown as ReturnType<typeof FakeAdapter>;

describe("spawning", () => {
  test("spawn_creates_child_on_child_branch: root from the parent's default-branch head; parent, rootSession, depth; the child's reach is its subtree; it returns at once", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const parentRoot = p.session.rootBranch!;
    const head = (await branches(local, projectId)).find((b) => b.id === parentRoot)!.head;
    const c = await spawn(p.client, projectId, "fixer");
    expect(c.session).toMatchObject({ parent: p.session.id, rootSession: p.session.id, depth: 1, role: "fixer", mode: "draft", harness: "fake", base: { branch: parentRoot, seq: head } });
    expect(c.session.createdBy.principal).toEqual({ kind: "agent", id: p.session.id });
    // At once: the brief's run is still going.
    expect(c.run?.status).toBe("running");
    const all = await branches(local, projectId);
    const made = all.find((b) => b.id === c.session.rootBranch)!;
    expect(made.name).toBe(`${all.find((b) => b.id === parentRoot)!.name}.fixer-1`);
    expect(made.parent).toBe(parentRoot);
    // The child writes its own branch, not its parent's, not main.
    ok(await write(c.client, projectId, c.session.rootBranch!));
    expect(code(await write(c.client, projectId, parentRoot))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await write(c.client, projectId, "main"))).toBe("BRANCH_NOT_ALLOWED");
    // A second child of the same role gets the next name; a base is refused (it starts from the parent's branch).
    expect(code(await spawnRaw(p.client, projectId, "fixer", { base: "main" }))).toBe("INVALID_REQUEST");
    const c2 = await spawn(p.client, projectId, "fixer");
    expect((await branches(local, projectId)).find((b) => b.id === c2.session.rootBranch)!.name).toBe(`${made.name.replace(/\.fixer-1$/, "")}.fixer-2`);
  });

  test("the child starts from the parent's current default branch (a recorded try), at its head", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const t = ok<{ branch: { id: string } }>(await p.client.call("branch.create", { txId: id(), author: "x", projectId, from: p.session.rootBranch!, name: "try-1" })).branch.id;
    ok(await p.client.call("agent.self.try", { action: "record", branch: t }));
    ok(await p.client.call("agent.self.try", { action: "setDefault", branch: t }));
    ok(await write(p.client, projectId, t));
    const head = (await branches(local, projectId)).find((b) => b.id === t)!.head;
    const c = await spawn(p.client, projectId, "fixer");
    expect(c.session.base).toEqual({ branch: t, seq: head });
    expect((await branches(local, projectId)).find((b) => b.id === c.session.rootBranch)!.parent).toBe(t);
  });

  test("pre_7b_credential_cannot_spawn: a credential minted with 7A's draft scopes keeps them; new ones hold agent.run", async () => {
    expect(MODE_SCOPES.draft).toContain("agent.run");
    expect(MODE_SCOPES.auto).toContain("agent.run");
    expect(MODE_SCOPES.plan).not.toContain("agent.run");
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    await server.stop();
    // The credential as 7A minted it: the draft scopes without `agent.run`.
    const db = new DatabaseSync(join(serverData, "control.db"));
    const row = db.prepare("SELECT scopes FROM credentials WHERE id = ?").get(p.session.id) as { scopes: string };
    db.prepare("UPDATE credentials SET scopes = ? WHERE id = ?").run(JSON.stringify((JSON.parse(row.scopes) as string[]).filter((s) => s !== "agent.run")), p.session.id);
    db.close();
    await server.start();
    const a = await server.agent(credentialPath(server.core.store.root, p.session.id));
    const refused = await spawnRaw(a, projectId, "fixer");
    expect(code(refused)).toBe("SCOPE_DENIED");
    expect(details(refused)).toMatchObject({ scopes: ["agent.run"] });
  });
});

describe("a spawn needs a live run", () => {
  test("a session spawns only from a run: an idle, interrupted or ended parent is refused", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const idle = await root(local, projectId, { prompt: undefined });
    expect(get(idle.session.id).status).toBe("idle");
    const fromIdle = await spawnRaw(idle.client, projectId, "fixer");
    expect(code(fromIdle)).toBe("SESSION_INTERRUPTED");
    expect(details(fromIdle)).toMatchObject({ status: "idle" });
    const p = await root(local, projectId);
    ok(await local.call("agent.session.interrupt", { sessionId: p.session.id }));
    const fromInterrupted = await spawnRaw(p.client, projectId, "fixer");
    expect(code(fromInterrupted)).toBe("SESSION_INTERRUPTED");
    expect(details(fromInterrupted)).toMatchObject({ status: "interrupted" });
    expect(server.core.store.tree.liveChildren(p.session.id)).toEqual([]);
  });

  test("a spawn without a brief is INVALID_REQUEST {field: prompt}; a person may still start an idle session", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const e = await p.client.call("agent.session.start", { projectId, role: "fixer" });
    expect(code(e)).toBe("INVALID_REQUEST");
    expect(details(e)).toMatchObject({ field: "prompt" });
    expect(code(await p.client.call("agent.session.start", { projectId, role: "fixer", prompt: "  " }))).toBe("INVALID_REQUEST");
    expect(server.core.store.tree.liveChildren(p.session.id)).toEqual([]);
    expect(ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake" })).session.status).toBe("idle");
  });

  for (const stop of ["interrupt", "end"] as const) {
    test(`a spawn racing its parent's ${stop}: the child the cascade could not see is stopped and the spawn fails`, async () => {
      let opened!: (id: string) => void;
      let release!: () => void;
      const paused = new Promise<string>((r) => (opened = r));
      const gate = new Promise<void>((r) => (release = r));
      const { local, projectId } = await boot([PARENT, WORKING], undefined, {
        afterChildOpened: async (childId) => {
          opened(childId);
          await gate;
        },
      });
      const p = await root(local, projectId);
      const spawning = spawnRaw(p.client, projectId, "fixer");
      const childId = await paused;
      // The child's branch and record exist; the parent stops before the spawn goes on.
      expect(get(childId)).toMatchObject({ parent: p.session.id, status: "idle" });
      ok(await local.call(`agent.session.${stop}`, { sessionId: p.session.id }));
      release();
      const e = await spawning;
      // The spawn fails (an ended parent's own credential is revoked by then: its answer says so).
      expect(code(e)).toBe(stop === "end" ? "CREDENTIAL_REVOKED" : "SESSION_INTERRUPTED");
      const want = stop === "end" ? { status: "ended", statusReason: "rootEnded" } : { status: "interrupted", statusReason: "parentInterrupted" };
      await status(childId, want.status as Session["status"]);
      expect(get(childId).statusReason).toBe(want.statusReason);
      // Its brief never ran.
      expect(server.core.store.runs(childId)).toEqual([]);
      expect(fake().log.turns).toHaveLength(1);
    });
  }

  test("two spawns racing the live limit: one is admitted", async () => {
    const { local, projectId } = await boot([PARENT, WORKING], { subagents: { maxLive: 1 } });
    const p = await root(local, projectId);
    const both = await Promise.all([spawnRaw(p.client, projectId, "fixer"), spawnRaw(p.client, projectId, "fixer")]);
    expect(both.filter((e) => e.ok)).toHaveLength(1);
    const refused = both.find((e) => !e.ok)!;
    expect(code(refused)).toBe("DEPTH_EXCEEDED");
    expect(details(refused)).toMatchObject({ kind: "live", live: 1, max: 1 });
    expect(server.core.store.tree.liveDescendants(p.session.id)).toBe(1);
  });

  test("a waitingApproval session holds a live slot", async () => {
    const { local, projectId } = await boot([WORKING], { maxLiveBuiltin: 1 });
    const a = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job a" })).session;
    await until(() => fake().log.turns.length === 1, "a's turn");
    const sub = subjectOf({ command: "checkpoint", tag: "history.checkpoint", payload: { projectId, branch: a.rootBranch!, name: "c" }, rule: "r1", layer: "local" });
    await Effect.runPromise(server.agents.approvals.ask(get(a.id), "rule", sub, argsHash(server.agents.approvalKey, sub)));
    expect(get(a.id).status).toBe("waitingApproval");
    const b = ok<{ session: Session; run: Run }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job b" }));
    expect(nodes(b.session.id).find((x) => x.kind === "notice")?.payload).toEqual({ code: "SESSION_QUEUED", kind: "builtin", limit: 1, position: 1 });
    expect(fake().log.turns).toHaveLength(1);
    ok(await local.call("agent.session.end", { sessionId: a.id }));
    await until(() => fake().log.turns.length === 2, "b's turn");
    expect(fake().log.turns.at(-1)).toMatchObject({ runId: b.run.id, text: "job b" });
  });
});

describe("own children only", () => {
  test("agent_run_only_for_own_children: prompt, steer, interrupt, end its children; another session's, a sibling's or its parent's are SESSION_NOT_FOUND; resume and delete are SCOPE_DENIED", async () => {
    const { local, projectId } = await boot([PARENT, IDLE]);
    const p = await root(local, projectId);
    const c1 = await spawn(p.client, projectId, "fixer");
    const c2 = await spawn(p.client, projectId, "fixer");
    const other = await root(local, projectId);
    await status(c1.session.id, "idle");
    await status(c2.session.id, "idle");
    const s1 = c1.session.id;
    // The parent drives its own child.
    expect(ok<{ run: Run }>(await p.client.call("agent.session.prompt", { sessionId: s1, text: "go on" })).run.status).toBe("running");
    await status(s1, "running");
    expect(ok<{ queued: boolean }>(await p.client.call("agent.session.steer", { sessionId: s1, text: "faster" }))).toEqual({ queued: false });
    expect(ok<{ session: Session }>(await p.client.call("agent.session.interrupt", { sessionId: s1 })).session.status).toBe("interrupted");
    // A resume and a delete stay people's.
    expect(code(await p.client.call("agent.session.resume", { sessionId: s1 }))).toBe("SCOPE_DENIED");
    expect(code(await p.client.call("agent.session.delete", { sessionId: s1, challenge: "x" }))).toBe("SCOPE_DENIED");
    expect(ok<{ session: Session }>(await p.client.call("agent.session.end", { sessionId: s1 })).session.status).toBe("ended");
    // Nobody else's: a sibling's, the parent's, another tree's.
    for (const [c, target] of [
      [c2.client, s1],
      [c2.client, p.session.id],
      [p.client, other.session.id],
      [other.client, c2.session.id],
    ] as const) {
      for (const tag of ["agent.session.prompt", "agent.session.steer"]) expect(code(await c.call(tag, { sessionId: target, text: "x" })), `${tag} ${target}`).toBe("SESSION_NOT_FOUND");
      for (const tag of ["agent.session.interrupt", "agent.session.end"]) expect(code(await c.call(tag, { sessionId: target })), `${tag} ${target}`).toBe("SESSION_NOT_FOUND");
      expect(code(await c.call("agent.session.wait", { ids: [target], timeoutMs: 0 }))).toBe("SESSION_NOT_FOUND");
    }
    // One of the ids another's: the whole wait is refused.
    expect(code(await p.client.call("agent.session.wait", { ids: [c2.session.id, other.session.id], timeoutMs: 0 }))).toBe("SESSION_NOT_FOUND");
    // `fin agent ls`: its own children only; a child's other reads stay people's.
    const listed = ok<{ sessions: Session[] }>(await p.client.call("agent.session.list", { projectId }));
    expect(listed.sessions.map((s) => s.id).sort()).toEqual([s1, c2.session.id].sort());
    expect(ok<{ sessions: Session[] }>(await c2.client.call("agent.session.list", {})).sessions).toEqual([]);
    expect(code(await p.client.call("agent.session.get", { sessionId: s1 }))).toBe("SCOPE_DENIED");
    // A person drives any session in reach.
    expect(ok<{ session: Session }>(await local.call("agent.session.end", { sessionId: c2.session.id })).session.status).toBe("ended");
  });

  test("an agent credential that is no session's starts and drives nothing (SCOPE_DENIED noSession)", async () => {
    const { local, projectId } = await boot([PARENT]);
    const p = await root(local, projectId);
    const owner = await server.console();
    const minted = ok<{ file: CredentialFile }>(
      await confirmed(owner, "auth.credential.mint", { kind: "agent", label: "hand-made", scopes: ["agent.run", "project.read", "project.write"], projects: [projectId] }),
    );
    const { client, exit } = await server.proveAs(minted.file);
    ok(exit);
    for (const [tag, payload] of [
      ["agent.session.start", { projectId, role: "fixer", prompt: "x" }],
      ["agent.session.prompt", { sessionId: p.session.id, text: "x" }],
      ["agent.session.end", { sessionId: p.session.id }],
      ["agent.session.wait", { ids: [p.session.id], timeoutMs: 0 }],
      // 7B Task 8: nor rolls one back.
      ["agent.session.rollback", { sessionId: p.session.id, toRun: 0 }],
    ] as const) {
      const e = await client.call(tag, payload);
      expect(code(e), tag).toBe("SCOPE_DENIED");
      expect(details(e), tag).toMatchObject({ reason: "noSession" });
    }
    expect(get(p.session.id).status).toBe("running");
  });

  test("parent_cannot_write_live_child_branch: the child's subtree is the child's while it lives; the parent's again once it ended", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const c = await spawn(p.client, projectId, "fixer");
    const childRoot = c.session.rootBranch!;
    expect(code(await write(p.client, projectId, childRoot))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await p.client.call("branch.create", { txId: id(), author: "x", projectId, from: childRoot, name: "mine" }))).toBe("BRANCH_NOT_ALLOWED");
    // Its own branch stays its own.
    ok(await write(p.client, projectId, p.session.rootBranch!));
    // The child's try branch is the child's too.
    const t = ok<{ branch: { id: string } }>(await c.client.call("branch.create", { txId: id(), author: "x", projectId, from: childRoot, name: "try-c" })).branch.id;
    expect(code(await write(p.client, projectId, t))).toBe("BRANCH_NOT_ALLOWED");
    ok(await local.call("agent.session.end", { sessionId: c.session.id }));
    ok(await write(p.client, projectId, childRoot));
    ok(await write(p.client, projectId, t));
  });

  test("parent_merges_child_into_own: a merge of the child's branch into the parent's is no outcome; the parent's into main is", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const c = await spawn(p.client, projectId, "fixer");
    const txId = id();
    ok(await commit(c.client, projectId, { ...tx(txId, 1, [insert(0)]), branch: c.session.rootBranch! }));
    // While the child lives: its branch is read, the parent's written.
    const merged = ok<{ committed: Array<{ tx: { txId: string } }> }>(await p.client.call("history.merge", { txId: id(), author: "x", projectId, from: c.session.rootBranch!, into: p.session.rootBranch! }));
    expect(merged.committed.length).toBeGreaterThan(0);
    const outcomes = (t: string) => server.core.store.outcomes.of(t).map((o) => ({ verdict: o.verdict, via: o.via, by: o.by }));
    expect(outcomes(txId)).toEqual([]);
    expect(server.core.store.sessionOfTx([merged.committed[0]!.tx.txId]).get(merged.committed[0]!.tx.txId)?.sessionId).toBe(p.session.id);
    // The child's transaction is the child's in session_txs.
    expect(server.core.store.txsOfSession(c.session.id).map((t) => t.txId)).toEqual([txId]);
    ok(await local.call("history.merge", { txId: id(), author: "someone", projectId, from: p.session.rootBranch!, into: "main" }));
    expect(outcomes(txId)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
  });
});

describe("bounds", () => {
  test("child_mode_at_most_parent and child_role_in_spawn_list (and the role's maxChildren)", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId);
    const tooHigh = await spawnRaw(p.client, projectId, "fixer", { mode: "auto" });
    expect(code(tooHigh)).toBe("MODE_INVALID");
    expect(details(tooHigh)).toMatchObject({ mode: "auto", parentMode: "draft" });
    // Defaults: the role's mode, at most the parent's.
    const critic = await spawn(p.client, projectId, "critic");
    expect(critic.session).toMatchObject({ mode: "plan" });
    expect(critic.session.rootBranch).toBeUndefined();
    const fixer = await spawn(p.client, projectId, "fixer");
    expect(fixer.session.mode).toBe("draft");
    // Only the roles the parent's role may spawn.
    for (const role of ["editor", "nobody"]) {
      const r = await spawnRaw(p.client, projectId, role);
      expect(code(r), role).toBe("ROLE_UNKNOWN");
      expect(details(r)).toMatchObject({ parentRole: "editor" });
    }
    expect(code(await p.client.call("agent.session.start", { projectId, prompt: "no role" }))).toBe("ROLE_UNKNOWN");
    // A fixer spawns nothing.
    expect(code(await spawnRaw(fixer.client, projectId, "fixer"))).toBe("ROLE_UNKNOWN");
    // An editor has at most 4 live children.
    await spawn(p.client, projectId, "mixer");
    await spawn(p.client, projectId, "colourist");
    const fifth = await spawnRaw(p.client, projectId, "fixer");
    expect(code(fifth)).toBe("DEPTH_EXCEEDED");
    expect(details(fifth)).toMatchObject({ kind: "children", children: 4, max: 4 });
    expect(exitCodeOf("DEPTH_EXCEEDED")).toBe(7);
    // One ends: room again.
    ok(await local.call("agent.session.end", { sessionId: critic.session.id }));
    await spawn(p.client, projectId, "fixer");
  });

  test("depth_and_live_limits: depth 4 and the 9th live descendant are DEPTH_EXCEEDED", async () => {
    const { local, projectId } = await boot([PARENT, WORKING], { maxLiveBuiltin: 32 });
    const r = await root(local, projectId);
    const d1 = await spawn(r.client, projectId, "clip-finder");
    const d2 = await spawn(d1.client, projectId, "clip-finder");
    const d3 = await spawn(d2.client, projectId, "clip-finder");
    expect([d1, d2, d3].map((c) => [c.session.depth, c.session.rootSession])).toEqual([1, 2, 3].map((d) => [d, r.session.id]));
    const deep = await spawnRaw(d3.client, projectId, "clip-finder");
    expect(code(deep)).toBe("DEPTH_EXCEEDED");
    expect(details(deep)).toMatchObject({ kind: "depth", depth: 4, max: 3 });
    // 3 live; the root's three more and two of a grandchild's make 8.
    const more = [await spawn(r.client, projectId, "clip-finder"), await spawn(r.client, projectId, "clip-finder"), await spawn(r.client, projectId, "clip-finder")];
    await spawn(more[0]!.client, projectId, "clip-finder");
    await spawn(more[0]!.client, projectId, "clip-finder");
    expect(server.core.store.tree.liveDescendants(r.session.id)).toBe(8);
    const ninth = await spawnRaw(more[1]!.client, projectId, "clip-finder");
    expect(code(ninth)).toBe("DEPTH_EXCEEDED");
    expect(details(ninth)).toMatchObject({ kind: "live", live: 8, max: 8, rootSession: r.session.id });
    // Another tree has its own count.
    const r2 = await root(local, projectId);
    await spawn(r2.client, projectId, "fixer");
  });

  test("the limits are settings: agents.subagents.maxDepth and .maxLive", async () => {
    const { local, projectId } = await boot([PARENT, WORKING], { subagents: { maxDepth: 1, maxLive: 2 } });
    const r = await root(local, projectId);
    const c = await spawn(r.client, projectId, "clip-finder");
    expect(details(await spawnRaw(c.client, projectId, "clip-finder"))).toMatchObject({ kind: "depth", depth: 2, max: 1 });
    await spawn(r.client, projectId, "clip-finder");
    expect(details(await spawnRaw(r.client, projectId, "clip-finder"))).toMatchObject({ kind: "live", live: 2, max: 2 });
  });

  test("live_limits_queue: the 9th built-in session's run waits with SESSION_QUEUED and starts when one ends; its held steer follows", async () => {
    const { local, projectId } = await boot([WORKING]);
    const started: Session[] = [];
    for (let i = 0; i < 8; i++) {
      started.push(ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: `job ${i}` })).session);
    }
    await until(() => fake().log.turns.length === 8, "8 turns");
    // The 9th: the call succeeds, its run is recorded, its turn waits.
    const ninth = ok<{ session: Session; run: Run }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job 8" }));
    expect(ninth.run.status).toBe("running");
    const queued = nodes(ninth.session.id).find((x) => x.kind === "notice");
    expect(queued?.payload).toEqual({ code: "SESSION_QUEUED", kind: "builtin", limit: 8, position: 1 });
    expect(fake().log.turns).toHaveLength(8);
    expect(ok<{ queued: boolean }>(await local.call("agent.session.steer", { sessionId: ninth.session.id, text: "and hurry" }))).toEqual({ queued: true });
    // A slot frees: the oldest waiting run starts, with the held steer.
    ok(await local.call("agent.session.end", { sessionId: started[3]!.id }));
    await until(() => fake().log.turns.length === 9, "the 9th turn");
    expect(fake().log.turns.at(-1)).toMatchObject({ runId: ninth.run.id, text: "job 8" });
    await until(() => fake().log.steers.some((x) => x.text === "and hurry"), "the held steer");
    // A queued run that is interrupted leaves the queue: the next free slot goes to the one after it.
    const tenth = ok<{ session: Session; run: Run }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job 9" }));
    const eleventh = ok<{ session: Session; run: Run }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job 10" }));
    expect(nodes(eleventh.session.id).find((x) => x.kind === "notice")?.payload).toMatchObject({ position: 2 });
    ok(await local.call("agent.session.interrupt", { sessionId: tenth.session.id }));
    ok(await local.call("agent.session.end", { sessionId: started[0]!.id }));
    await until(() => fake().log.turns.length === 10, "the 11th's turn");
    expect(fake().log.turns.at(-1)).toMatchObject({ runId: eleventh.run.id, text: "job 10" });
  });
});

describe("ending and waiting", () => {
  test("child_ends_itself_with_summary and the parent's subagent node", async () => {
    const { local, projectId } = await boot([PARENT, brief("found 3 cuts")]);
    const p = await root(local, projectId);
    const c = await spawn(p.client, projectId, "clip-finder");
    await status(c.session.id, "ended");
    expect(get(c.session.id).statusReason).toBe("briefDone");
    const want: ChildResult = { child: c.session.id, status: "ended", reason: "briefDone", branch: c.session.rootBranch!, summary: "found 3 cuts" };
    const node = await until(() => nodes(p.session.id).find((x) => x.kind === "subagent"), "the subagent node");
    expect(node.payload).toEqual(want);
    // The parent's run is the one it had: nothing was injected into a turn.
    expect(node.runId).toBe(server.core.store.runs(p.session.id).at(-1)!.id);
    expect(ok<{ children: ChildResult[] }>(await p.client.call("agent.session.wait", { ids: [c.session.id], timeoutMs: 1000 })).children).toEqual([want]);
    // Its credential ended with it.
    await expect(server.agent(credentialPath(server.core.store.root, c.session.id))).rejects.toThrow(/CREDENTIAL_REVOKED/);
  });

  test("wait_any_and_timeout: --any returns the done ones; WAIT_TIMEOUT (exit 5) when none is, repeatable", async () => {
    const { local, projectId } = await boot([PARENT, WORKING, brief("b done")]);
    const p = await root(local, projectId);
    const a = await spawn(p.client, projectId, "fixer");
    const b = await spawn(p.client, projectId, "fixer");
    const any = ok<{ children: ChildResult[] }>(await p.client.call("agent.session.wait", { ids: [a.session.id, b.session.id], any: true, timeoutMs: 5000 }));
    expect(any.children).toEqual([{ child: b.session.id, status: "ended", reason: "briefDone", branch: b.session.rootBranch!, summary: "b done" }]);
    for (let i = 0; i < 2; i++) {
      const timedOut = await p.client.call("agent.session.wait", { ids: [a.session.id], timeoutMs: 50 });
      expect(code(timedOut)).toBe("WAIT_TIMEOUT");
      expect(details(timedOut)).toMatchObject({ ids: [a.session.id], children: [{ child: a.session.id, status: "running" }] });
    }
    expect(exitCodeOf("WAIT_TIMEOUT")).toBe(5);
    // All of them: one still runs.
    expect(code(await p.client.call("agent.session.wait", { ids: [a.session.id, b.session.id], timeoutMs: 50 }))).toBe("WAIT_TIMEOUT");
    // An interrupted child is done (a person may resume it).
    ok(await p.client.call("agent.session.interrupt", { sessionId: a.session.id }));
    const all = ok<{ children: ChildResult[] }>(await p.client.call("agent.session.wait", { ids: [a.session.id, b.session.id], timeoutMs: 1000 }));
    expect(all.children.map((c) => c.status)).toEqual(["interrupted", "ended"]);
  });

  test("parent_end_interrupts_children; a parent's interrupt too (resumable by a person); a root's end ends every live descendant", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const r = await root(local, projectId);
    const c1 = await spawn(r.client, projectId, "clip-finder");
    const g = await spawn(c1.client, projectId, "clip-finder");
    // A child that ends interrupts its children.
    ok(await r.client.call("agent.session.end", { sessionId: c1.session.id }));
    await status(g.session.id, "interrupted");
    expect(get(g.session.id).statusReason).toBe("parentEnded");
    // A parent's interrupt interrupts its live children; a person resumes one.
    const c2 = await spawn(r.client, projectId, "fixer");
    ok(await local.call("agent.session.interrupt", { sessionId: r.session.id }));
    await status(c2.session.id, "interrupted");
    expect(get(c2.session.id).statusReason).toBe("parentInterrupted");
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: c2.session.id })).session.status).toBe("idle");
    // The parent hears what it did not cause, never its own cascade.
    const heard = nodes(r.session.id).filter((x) => x.kind === "subagent").map((x) => (x.payload as ChildResult).child);
    expect(heard).toEqual([c1.session.id]);
    // The root ends: every live descendant ends, an interrupted grandchild of an ended child too.
    ok(await local.call("agent.session.end", { sessionId: r.session.id }));
    await status(c2.session.id, "ended");
    await status(g.session.id, "ended");
    expect([get(c2.session.id).statusReason, get(g.session.id).statusReason]).toEqual(["rootEnded", "rootEnded"]);
  });

  test("a root's end ends a descendant after its current tool call: the call's result is recorded, then the child ends and its credential with it", async () => {
    let started!: () => void;
    let finish!: () => void;
    const calling = new Promise<void>((r) => (started = r));
    const finished = new Promise<void>((r) => (finish = r));
    const fakeTools = { run: async (): Promise<ToolOutcome> => (started(), await finished, { ok: true, output: { exit: 0, stdout: "viewed" } }) };
    const { local, projectId } = await boot([PARENT, TOOL], undefined, { fakeTools });
    const r = await root(local, projectId);
    const c = await spawn(r.client, projectId, "clip-finder");
    const g = await spawn(c.client, projectId, "clip-finder");
    await calling;
    ok(await local.call("agent.session.end", { sessionId: r.session.id }));
    // Mid-call: the child goes on, its credential still answers; a new spawn from it is refused.
    await new Promise((x) => setTimeout(x, 100));
    expect(get(c.session.id).status).toBe("running");
    ok(await c.client.call("agent.session.list", {}));
    expect(code(await spawnRaw(c.client, projectId, "clip-finder"))).toBe("SESSION_ENDED");
    finish();
    await status(c.session.id, "ended");
    expect(get(c.session.id).statusReason).toBe("rootEnded");
    const result = nodes(c.session.id).find((x) => x.kind === "toolResult")!;
    expect(result.payload).toEqual({ ok: true, output: { exit: 0, stdout: "viewed" } });
    await expect(server.agent(credentialPath(server.core.store.root, c.session.id))).rejects.toThrow(/CREDENTIAL_REVOKED/);
    // Its own child (in a call too) ended with the root, not interrupted by its parent's end.
    await status(g.session.id, "ended");
    expect(get(g.session.id).statusReason).toBe("rootEnded");
  });

  test("a root's end waits for a descendant's call at most the stop grace, then ends it (the call gets the synthetic result)", async () => {
    const fakeTools = { run: () => new Promise<ToolOutcome>(() => {}) };
    const { local, projectId } = await boot([PARENT, TOOL], undefined, { fakeTools, stopGraceMs: 50 });
    const r = await root(local, projectId);
    const c = await spawn(r.client, projectId, "fixer");
    await until(() => nodes(c.session.id).some((x) => x.kind === "toolCall"), "the child's call");
    ok(await local.call("agent.session.end", { sessionId: r.session.id }));
    await status(c.session.id, "ended");
    expect(get(c.session.id).statusReason).toBe("rootEnded");
    expect(nodes(c.session.id).find((x) => x.kind === "toolResult")?.payload).toMatchObject({ ok: false, code: "SESSION_INTERRUPTED", reason: "rootEnded", synthetic: true });
  });

  test("deleting a parent interrupts its children", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const r = await root(local, projectId);
    const c = await spawn(r.client, projectId, "fixer");
    const owner = await server.console();
    ok(await confirmed(owner, "agent.session.delete", { sessionId: r.session.id }));
    await status(c.session.id, "interrupted");
    expect(get(c.session.id).parent).toBe(r.session.id);
  });
});

describe("the journal", () => {
  test("a real spawn through the RPC journals the child's agent.session with parent and trigger: subagent", async () => {
    const { local, projectId } = await boot([PARENT, WORKING], undefined, { privacy: { diagnostics: "on" } });
    const p = await root(local, projectId);
    const c = await spawn(p.client, projectId, "fixer");
    const of = (sessionId: string) => agentEntries(serverData, "events").find((e) => e.kind === "agent.session" && e.payload.sessionId === sessionId && e.payload.phase === "started");
    const child = await until(() => of(c.session.id), "the child's agent.session");
    expect(child.payload).toMatchObject({ phase: "started", parent: p.session.id, trigger: "subagent", role: "fixer" });
    const top = await until(() => of(p.session.id), "the root's agent.session");
    expect(top.payload).toMatchObject({ trigger: "user" });
    expect(top.payload.parent).toBeUndefined();
  });

  test("a child's agent.session has parent and trigger: subagent; a root's trigger is user", async () => {
    const dir = join(tmp, "journal");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const stores = openStores(dir, { diagnostics: "on" });
    const clock = manualClock(Date.now());
    const control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    try {
      const store = new AgentStore(control, dir, clock, systemIds(clock));
      const journal = new AgentJournal({ diagnostics: stores.diag, traces: stores.traces, store, ids: systemIds(clock), clock });
      const base: Omit<NewSession, "id"> = {
        projectId: P1,
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
      const rootSession = await store.createSession({ ...base, id: systemIds(clock).ulid() });
      const child = await store.createSession({ ...base, id: systemIds(clock).ulid(), role: "fixer", rootBranch: "br2", parent: rootSession.id, rootSession: rootSession.id, depth: 1, createdBy: { principal: { kind: "agent", id: rootSession.id } } });
      expect(rootSession.rootSession).toBeUndefined();
      expect(child).toMatchObject({ parent: rootSession.id, rootSession: rootSession.id, depth: 1 });
      // The migration's trigger: a root names itself in its row.
      expect((control.db.prepare("SELECT rootSession, depth FROM sessions WHERE id = ?").get(rootSession.id) as { rootSession: string; depth: number })).toEqual({ rootSession: rootSession.id, depth: 0 });
      for (const s of [rootSession, child]) journal.session(s.id, store.apply(s.id, { kind: "ready" }).session, "started", null);
      await flush(stores);
      const phases = agentEntries(dir, "events").filter((e) => e.kind === "agent.session");
      expect(phases.find((e) => e.payload.sessionId === child.id)?.payload).toMatchObject({ phase: "started", parent: rootSession.id, trigger: "subagent", role: "fixer" });
      const top = phases.find((e) => e.payload.sessionId === rootSession.id)?.payload;
      expect(top).toMatchObject({ trigger: "user" });
      expect(top?.parent).toBeUndefined();
    } finally {
      await stores.close();
      await control.close();
    }
  });
});
