// Rolling a session back to a run (sub-project 7B Task 8, Decision 23; spec §4.2, §5.3; Review
// Focus 5): against a real server, engine and `control.db`, sessions on the scripted `fake`
// harness (a test-hooks build run with `FIN_TEST_HOOKS=1`) whose runs commit through the
// session's own credential in a tool call; the built-in loop through the runtime with a scripted
// model client.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { CredentialFile, Node, Run, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { BuiltinAdapter } from "../src/agent/builtin/loop.ts";
import type { ModelClient, ModelResult, ModelStreamEvent } from "../src/agent/builtin/compat.ts";
import type { ModelRequest } from "../src/agent/builtin/request.ts";
import { Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { nodeLine } from "../src/agent/console.ts";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeAdapter, FakeScript } from "../src/agent/fake.ts";
import { OutcomeTracker } from "../src/agent/outcomes.ts";
import { BranchLineage } from "../src/agent/reach.ts";
import { type RollbackRegistry, Rollbacks } from "../src/agent/rollback.ts";
import { finError } from "../src/errors.ts";
import { cataloguePricing, type ToolCall, type ToolOutcome } from "../src/agent/runtime-deps.ts";
import { AgentStore } from "../src/agent/store.ts";
import { openControlStore } from "../src/control/db.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";
import { FakeRegistry, makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

let tmp: string;
let serverData: string;
let server: AgentServer | undefined;
const hooks = process.env.FIN_TEST_HOOKS;

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-rollback-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
/** One run: a tool call (the test's work for that session, run by `fakeTools`), then a reply; `stop` how the turn ends. */
const workTurn = (k: number, stop = "endTurn") => [
  { emit: [{ type: "toolStarted", providerId: `t${k}`, tool: "fin", input: { argv: ["edit"] } }] },
  { awaitTool: `t${k}` },
  { emit: [msg(`run ${k} done`), { type: "turnCompleted", stopReason: stop }] },
];
/** A root session's runs (`endTurn` leaves a root idle). */
const worker = (capabilities: FakeScript["capabilities"] = {}): FakeScript => ({ name: "worker", capabilities, turns: [1, 2, 3, 4, 5].map((k) => workTurn(k)) });
/** A child's runs: `maxTokens`, so a child stays idle after each (an `endTurn` brief ends it). */
const CHILD: FakeScript = { name: "child", turns: [1, 2, 3, 4].map((k) => workTurn(k, "maxTokens")) };
/** A run that never ends by itself. */
const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };
const PARENT = WORKING;

/** What each session's next tool call does (commits through its credential). */
const work = new Map<string, Array<() => Promise<void>>>();
/** What a session's first call does when nothing was queued for it yet (a child's brief, spawned meanwhile). */
const firstWork: Array<(sessionId: string) => Promise<void>> = [];
const fakeTools = {
  run: async (c: ToolCall): Promise<ToolOutcome> => {
    const queued = work.get(c.sessionId)?.shift();
    if (queued !== undefined) await queued();
    else await firstWork.shift()?.(c.sessionId);
    return { ok: true, output: { exit: 0 } };
  },
};

async function boot(scripts: readonly FakeScript[]) {
  work.clear();
  firstWork.length = 0;
  const s = new AgentServer(serverData, { timers: false, fakeScripts: scripts, fakeTools });
  server = s;
  await s.start();
  const local = await s.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

const srv = () => server!;
const get = (sessionId: string) => srv().core.store.get(sessionId)!;
const runs = (sessionId: string): Run[] => srv().core.store.runs(sessionId);
const nodes = (sessionId: string): Node[] => srv().core.store.nodes(sessionId, 1, 1000);
const fake = () => srv().agents.adapters.get("fake") as unknown as ReturnType<typeof FakeAdapter>;
const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

async function until<A>(f: () => A | undefined | false, what: string, ms = 8000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** `sessionId` is idle (or `want`) after `count` runs. */
const settled = (sessionId: string, count: number, want: Session["status"] = "idle") =>
  until(() => runs(sessionId).length === count && runs(sessionId).at(-1)!.status !== "running" && get(sessionId).status === want, `${sessionId}: ${count} runs, ${want}`);

const clientOf = (sessionId: string) => srv().agent(credentialPath(srv().core.store.root, sessionId));

/** A root session a person started without a prompt (no run yet), and its credential's client. */
async function idleRoot(local: NdjsonClient, projectId: string) {
  const { session } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake" }));
  await until(() => get(session.id).status === "idle", "idle");
  return { session, client: await clientOf(session.id) };
}

/** Run `count + 1` of `sessionId`, whose tool call does `f`; prompted by `by`. */
async function runWith(by: NdjsonClient, sessionId: string, f: () => Promise<void>, want: Session["status"] = "idle") {
  const count = runs(sessionId).length;
  work.set(sessionId, [...(work.get(sessionId) ?? []), f]);
  ok(await by.call("agent.session.prompt", { sessionId, text: `run ${count + 1}` }));
  await settled(sessionId, count + 1, want);
}

/** A commit on `branch` as `c`; its txId. */
async function edit(c: NdjsonClient, projectId: string, branch: string, at = 0): Promise<string> {
  const txId = id();
  ok(await commit(c, projectId, { ...tx(txId, 1, [insert(at)]), branch }));
  return txId;
}

/** A branch under `from` made by `c` (a session's `fin try`); its id. */
async function tryBranch(c: NdjsonClient, projectId: string, from: string, name: string): Promise<string> {
  const made = ok<{ branch: { id: string } }>(await c.call("branch.create", { projectId, txId: id(), author: "x", from, name }));
  ok(await c.call("agent.self.try", { action: "record", branch: made.branch.id }));
  return made.branch.id;
}

const snapshot = async (c: NdjsonClient, projectId: string, branch: string) => ok<{ project: unknown }>(await c.call("snapshot.get", { projectId, branch })).project;
const heads = async (c: NdjsonClient, projectId: string) =>
  new Map(ok<{ branches: Array<{ id: string; head: number }> }>(await c.call("branch.list", { projectId, all: true })).branches.map((b) => [b.id, b.head] as const));

type Rolled = { session: Session; undone: string[]; branches: string[]; conversation: "rolledBack" | "kept" };

/**
 * Three runs: run 1 commits A on the root; run 2 commits B on the root, makes a try branch and
 * commits C on it; run 3 commits D on the root and E on the try. `snapshot`s after run 1.
 */
async function threeRuns(local: NdjsonClient, projectId: string) {
  const s = await idleRoot(local, projectId);
  const root = s.session.rootBranch!;
  const tx: Record<string, string> = {};
  let tryId = "";
  await runWith(local, s.session.id, async () => void (tx.A = await edit(s.client, projectId, root, 0)));
  const afterRun1 = { root: await snapshot(local, projectId, root) };
  let tryAtFork: unknown;
  await runWith(local, s.session.id, async () => {
    tx.B = await edit(s.client, projectId, root, 1);
    tryId = await tryBranch(s.client, projectId, root, "try-1");
    tryAtFork = await snapshot(local, projectId, tryId);
    tx.C = await edit(s.client, projectId, tryId, 2);
  });
  await runWith(local, s.session.id, async () => {
    tx.D = await edit(s.client, projectId, root, 3);
    tx.E = await edit(s.client, projectId, tryId, 4);
  });
  return { ...s, root, tryId, tx, afterRun1, tryAtFork };
}

describe("rollback through the runtime and the engine", () => {
  test("rollback --to-run 1 undoes runs 2 and 3's transactions on both branches, newest first, as the session's author", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await threeRuns(local, projectId);
    const { tx: t } = s;
    // Run-attributed: each row names the run it was made in.
    const byRun = (txId: string) => runs(s.session.id).find((r) => r.id === srv().core.store.sessionOfTx([txId]).get(txId)?.runId)?.seq;
    expect([t.A, t.B, t.C, t.D, t.E].map((x) => byRun(x!))).toEqual([1, 2, 2, 3, 3]);
    const before = await heads(local, projectId);
    const r = ok<Rolled>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    // The try branch's newest transaction is the newest: it goes first; each branch newest first.
    expect(r.undone).toEqual([t.E, t.C, t.D, t.B]);
    expect(r.branches).toEqual([s.tryId, s.root]);
    expect(await snapshot(local, projectId, s.root)).toEqual(s.afterRun1.root);
    expect(await snapshot(local, projectId, s.tryId)).toEqual(s.tryAtFork);
    // One undo record per branch, under the session's author: the session's own redo takes it back.
    const after = await heads(local, projectId);
    for (const b of [s.root, s.tryId]) expect(after.get(b), b).toBe(before.get(b)! + 1);
    expect(code(await local.call("history.redo", { projectId, txId: id(), author: "someone", branch: s.root }))).toBe("NOTHING_TO_REDO");
    // The outcomes: `undone`, by a person, with `details.rollback`; run 1's stays open.
    for (const x of [t.B, t.C, t.D, t.E]) {
      expect(srv().core.store.outcomes.of(x!)).toEqual([expect.objectContaining({ verdict: "undone", via: "undo", by: "user", details: { rollback: { toRun: 1, by: "local:local" } } })]);
    }
    expect(srv().core.store.outcomes.of(t.A!)).toEqual([]);
    expect(r.session.status).toBe("idle");
    // A run to roll back to must be before the last, and not one rolled back already.
    expect(code(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 3 }))).toBe("INVALID_REQUEST");
    expect(code(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 9 }))).toBe("INVALID_REQUEST");
    ok(await s.client.call("history.redo", { projectId, txId: id(), author: "x", branch: s.root }));
    expect(await snapshot(local, projectId, s.root)).not.toEqual(s.afterRun1.root);
  });

  test("the session's author writes the undo records and the session goes on: a later run, then a second rollback to 0", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await threeRuns(local, projectId);
    ok(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    // Run 4 takes the next seq; the rolled-back runs stay.
    await runWith(local, s.session.id, async () => void (s.tx.F = await edit(s.client, projectId, s.root, 0)));
    expect(runs(s.session.id).map((r) => r.seq)).toEqual([1, 2, 3, 4]);
    expect(code(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 2 }))).toBe("INVALID_REQUEST");
    const r = ok<Rolled>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 0 }));
    expect(r.undone).toEqual([s.tx.F, s.tx.A]);
    expect(r.branches).toEqual([s.root]);
    const fresh = await idleRoot(local, projectId);
    // Back to where the session started: its root as a new session's root (same base).
    expect(await snapshot(local, projectId, s.root)).toEqual(await snapshot(local, projectId, fresh.session.rootBranch!));
  });

  test("conflict_leaves_both_sides: a person's later commit conflicting with one undo is CONFLICT; every branch as before, the one already undone too", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, root, 0)));
    let tryId = "";
    let clip = "";
    await runWith(local, s.session.id, async () => {
      tryId = await tryBranch(s.client, projectId, root, "try-1");
      const c = ok<{ bindings: Record<string, string> }>(await commit(s.client, projectId, { ...tx(id(), 1, [insert(2)]), branch: tryId }));
      clip = c.bindings.$c!;
    });
    // Run 3's on the root: the root's undo is the newest, so it is made first.
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, root, 1)));
    // A person deletes the clip the session's run 2 inserted on the try.
    const head = (await heads(local, projectId)).get(tryId)!;
    ok(await commit(local, projectId, { ...tx(id(), head, [{ op: "item.delete", item: clip, mode: "ripple" }]), branch: tryId }));
    const before = { root: await snapshot(local, projectId, root), try: await snapshot(local, projectId, tryId) };
    const e = await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 });
    expect(code(e)).toBe("CONFLICT");
    expect(details(e)).toMatchObject({ branch: tryId, redone: [root], toRun: 1, sessionId: s.session.id });
    expect(details(e)?.redoFailed).toBeUndefined();
    expect(await snapshot(local, projectId, root)).toEqual(before.root);
    expect(await snapshot(local, projectId, tryId)).toEqual(before.try);
    // No outcome, no marks, the conversation as it was; the session can still run.
    expect(srv().core.store.txsOfSession(s.session.id).flatMap((t) => srv().core.store.outcomes.of(t.txId))).toEqual([]);
    expect(nodes(s.session.id).some((x) => x.rolledBack === true || (x.payload as { rollback?: unknown } | undefined)?.rollback !== undefined)).toBe(false);
    expect(get(s.session.id).status).toBe("idle");
  });

  test("refused_while_running_or_with_live_child: SESSION_BUSY {status} and {reason: liveChildren}; an ended one SESSION_ENDED", async () => {
    const { local, projectId } = await boot([worker(), WORKING]);
    const s = await idleRoot(local, projectId);
    await runWith(local, s.session.id, async () => {});
    // Run 2: its tool call spawns a child (it stays running) and the run ends.
    await runWith(local, s.session.id, async () => void ok(await s.client.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })));
    const [child] = srv().core.store.tree.liveChildren(s.session.id);
    expect(child).toBeDefined();
    let e = await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 });
    expect(code(e)).toBe("SESSION_BUSY");
    expect(details(e)).toMatchObject({ reason: "liveChildren", children: [child] });
    // The child runs.
    e = await local.call("agent.session.rollback", { sessionId: child!, toRun: 0 });
    expect(code(e)).toBe("SESSION_BUSY");
    expect(details(e)).toMatchObject({ status: "running" });
    ok(await local.call("agent.session.end", { sessionId: child! }));
    // Ended: none.
    await runWith(local, s.session.id, async () => {});
    ok(await local.call("agent.session.end", { sessionId: s.session.id }));
    expect(code(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }))).toBe("SESSION_ENDED");
  });

  test("fake_provider_rollback_called: with the capability the provider rolls back to the turn and the later runs' nodes are marked", async () => {
    const { local, projectId } = await boot([worker({ rollback: true })]);
    const s = await threeRuns(local, projectId);
    const r = ok<Rolled>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.conversation).toBe("rolledBack");
    expect(fake().log.rollbacks).toEqual([expect.objectContaining({ toTurn: 1 })]);
    const [run1, ...later] = runs(s.session.id);
    expect(nodes(s.session.id).filter((x) => x.runId === run1!.id).every((x) => x.rolledBack === undefined)).toBe(true);
    const marked = nodes(s.session.id).filter((x) => later.some((r2) => r2.id === x.runId) && x.kind !== "notice");
    expect(marked.length).toBeGreaterThan(0);
    expect(marked.every((x) => x.rolledBack === true)).toBe(true);
    const notice = nodes(s.session.id).at(-1)!;
    expect(notice).toMatchObject({ kind: "notice", payload: { rollback: { toRun: 1, undone: 4, conversation: "rolledBack" } } });
  });

  test("cli_without_capability_keeps_provider_history: transactions undone, the provider's history left, a notice says so", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await threeRuns(local, projectId);
    const r = ok<Rolled>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.undone).toHaveLength(4);
    expect(r.conversation).toBe("kept");
    expect(fake().log.rollbacks).toEqual([]);
    // Its runs are marked all the same (their work is undone): the session's own record.
    const later = runs(s.session.id).slice(1);
    expect(nodes(s.session.id).filter((x) => later.some((r2) => r2.id === x.runId) && x.kind !== "notice").every((x) => x.rolledBack === true)).toBe(true);
    expect(nodes(s.session.id).at(-1)).toMatchObject({ kind: "notice", payload: { rollback: { toRun: 1, conversation: "kept", reason: "noProviderRollback" } } });
    expect(await snapshot(local, projectId, s.root)).toEqual(s.afterRun1.root);
  });
});

describe("afterSeq is the rollback's own", () => {
  test("history.undo over RPC with afterSeq is a plain undo: exactly one unit", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    const made: string[] = [];
    await runWith(local, s.session.id, async () => {
      made.push(await edit(s.client, projectId, root, 0));
      made.push(await edit(s.client, projectId, root, 1));
    });
    const r = ok<{ undone: string[] }>(await s.client.call("history.undo", { projectId, txId: id(), author: "x", branch: root, afterSeq: 0 }));
    expect(r.undone).toEqual([made[1]]);
  });
});

/** `c`'s plain undo (or `undo --to`, `redo`) on `branch`; its result. */
const hist = async (c: NdjsonClient, op: "undo" | "redo", projectId: string, branch: string, extra: Record<string, unknown> = {}) =>
  ok<{ undone: string[] }>(await c.call(`history.${op}`, { projectId, txId: id(), author: "x", branch, ...extra }));

describe("the session's later undos and redos are reversed too", () => {
  test("a run-2 undo of a run-1 commit: rollback to run 1 redoes it (A is back)", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    let a = "";
    await runWith(local, s.session.id, async () => void (a = await edit(s.client, projectId, root, 0)));
    const afterRun1 = await snapshot(local, projectId, root);
    let u: string[] = [];
    await runWith(local, s.session.id, async () => void (u = (await hist(s.client, "undo", projectId, root)).undone));
    expect(u).toEqual([a]);
    expect(await snapshot(local, projectId, root)).not.toEqual(afterRun1);
    const r = ok<Rolled & { restored?: string[] }>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.undone).toEqual([]);
    expect(r.branches).toEqual([root]);
    expect(r.restored).toHaveLength(1);
    expect(await snapshot(local, projectId, root)).toEqual(afterRun1);
    // A's outcome is not `undone` by the rollback; the notice counts what was restored.
    expect(srv().core.store.outcomes.of(a).filter((o) => (o.details as { rollback?: unknown } | undefined)?.rollback !== undefined)).toEqual([]);
    expect(nodes(s.session.id).at(-1)).toMatchObject({ kind: "notice", payload: { rollback: { toRun: 1, undone: 0, restored: 1 } } });
  });

  test("an undo --to <checkpoint> in run 2 of run 1's units: rollback to run 1 redoes that one record", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    await runWith(local, s.session.id, async () => {
      await edit(s.client, projectId, root, 0);
      ok(await local.call("history.checkpoint", { projectId, txId: id(), author: "someone", branch: root, name: "v1" }));
      await edit(s.client, projectId, root, 1);
      await edit(s.client, projectId, root, 2);
    });
    const afterRun1 = await snapshot(local, projectId, root);
    await runWith(local, s.session.id, async () => void expect((await hist(s.client, "undo", projectId, root, { to: "v1" })).undone).toHaveLength(2));
    const before = (await heads(local, projectId)).get(root)!;
    const r = ok<Rolled & { restored?: string[] }>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.restored).toHaveLength(1);
    expect(await snapshot(local, projectId, root)).toEqual(afterRun1);
    expect((await heads(local, projectId)).get(root)).toBe(before + 1);
  });

  test("a run-2 redo of a run-1 undo is undone; undos and redos within the later runs cancel out", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    await runWith(local, s.session.id, async () => {
      await edit(s.client, projectId, root, 0);
      await hist(s.client, "undo", projectId, root);
    });
    const afterRun1 = await snapshot(local, projectId, root);
    let c = "";
    await runWith(local, s.session.id, async () => {
      await hist(s.client, "redo", projectId, root);
      // B then its undo cancel out; C clears the redo stack (B's undo is not needed back).
      await edit(s.client, projectId, root, 1);
      await hist(s.client, "undo", projectId, root);
      c = await edit(s.client, projectId, root, 2);
    });
    const r = ok<Rolled & { restored?: string[] }>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.undone).toHaveLength(2);
    expect(r.undone[0]).toBe(c);
    expect(r.restored ?? []).toEqual([]);
    expect(await snapshot(local, projectId, root)).toEqual(afterRun1);
  });

  test("an undo a later edit cleared off the redo stack cannot be redone: CONFLICT notReversible before any write", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, root, 0)));
    let u = "";
    await runWith(local, s.session.id, async () => {
      await hist(s.client, "undo", projectId, root);
      u = srv().core.store.txsOfSession(s.session.id).at(-1)!.txId;
      await edit(s.client, projectId, root, 1);
    });
    const before = await heads(local, projectId);
    const e = await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 });
    expect(code(e)).toBe("CONFLICT");
    expect(details(e)).toMatchObject({ reason: "notReversible", why: "redoCleared", branch: root, tx: u });
    expect(await heads(local, projectId)).toEqual(before);
  });
});

describe("after the undos land", () => {
  test("a redo that fails while restoring: CONFLICT with partial, redoFailed, and that branch's units recorded undone", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    const root = s.session.rootBranch!;
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, root, 0)));
    let tryId = "";
    let clip = "";
    await runWith(local, s.session.id, async () => {
      tryId = await tryBranch(s.client, projectId, root, "try-1");
      clip = ok<{ bindings: Record<string, string> }>(await commit(s.client, projectId, { ...tx(id(), 1, [insert(2)]), branch: tryId })).bindings.$c!;
    });
    let b = "";
    await runWith(local, s.session.id, async () => void (b = await edit(s.client, projectId, root, 1)));
    const head = (await heads(local, projectId)).get(tryId)!;
    ok(await commit(local, projectId, { ...tx(id(), head, [{ op: "item.delete", item: clip, mode: "ripple" }]), branch: tryId }));
    const reg = (srv().agents.runtime as unknown as { deps: { registry: RollbackRegistry } }).deps.registry;
    const history = reg.history.bind(reg);
    reg.history = (p, op, payload, m) => (op === "redo" ? Effect.fail(finError("INTERNAL", "injected redo failure")) : history(p, op, payload, m));
    try {
      const e = await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 });
      expect(code(e)).toBe("CONFLICT");
      expect(details(e)).toMatchObject({ partial: true, branch: tryId, redone: [], redoFailed: [{ branch: root, code: "INTERNAL" }], undone: [b] });
    } finally {
      reg.history = history;
    }
    expect(srv().core.store.outcomes.of(b)).toEqual([expect.objectContaining({ verdict: "undone", via: "undo", details: { rollback: { toRun: 1, by: "local:local" } } })]);
  });

  test("marking the runs or the notice failing after the undos landed: success with a warning", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await threeRuns(local, projectId);
    const marks = srv().core.store.rollbacks;
    const mark = marks.mark.bind(marks);
    let calls = 0;
    marks.mark = () => {
      calls++;
      throw new Error("injected mark failure");
    };
    try {
      const r = ok<Rolled & { warning?: string }>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
      expect(r.undone).toHaveLength(4);
      expect(calls).toBe(2);
      expect(r.warning).toContain("injected mark failure");
    } finally {
      marks.mark = mark;
    }
    expect(await snapshot(local, projectId, s.root)).toEqual(s.afterRun1.root);
  });

  test("a branch archived since is left as it was and named in the answer and the notice", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await threeRuns(local, projectId);
    ok(await local.call("branch.set", { projectId, txId: id(), author: "someone", branch: s.tryId, archived: true }));
    const r = ok<Rolled & { archived?: string[] }>(await local.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 }));
    expect(r.branches).toEqual([s.root]);
    expect(r.archived).toEqual([s.tryId]);
    expect(nodes(s.session.id).at(-1)).toMatchObject({ kind: "notice", payload: { rollback: { archived: [s.tryId] } } });
  });
});

describe("who may roll back", () => {
  test("a rollback into a protected branch needs branch.protected (5E, Task 6 review S3)", async () => {
    const { local, projectId } = await boot([PARENT, CHILD]);
    const { session: parent } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go" }));
    await until(() => get(parent.id).status === "running", "the parent runs");
    const p = await clientOf(parent.id);
    firstWork.push(async (sid) => void (await edit(await clientOf(sid), projectId, get(sid).rootBranch!, 0)));
    const c1 = ok<{ session: Session }>(await p.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })).session;
    const c1Client = await clientOf(c1.id);
    await settled(c1.id, 1);
    const atRun1 = await snapshot(local, projectId, c1.rootBranch!);
    let b = "";
    await runWith(p, c1.id, async () => void (b = await edit(c1Client, projectId, c1.rootBranch!, 1)));
    // A person protects the child's branch.
    ok(await confirmed(await srv().console(), "review.policy", { projectId, protected: ["main", c1.rootBranch!] }));
    const before = await heads(local, projectId);
    // The parent session's credential has no `branch.protected`: refused before any write.
    const refused = await p.call("agent.session.rollback", { sessionId: c1.id, toRun: 1 });
    expect(code(refused)).toBe("BRANCH_PROTECTED");
    expect(details(refused)).toMatchObject({ branch: c1.rootBranch });
    expect(await heads(local, projectId)).toEqual(before);
    expect(srv().core.store.outcomes.of(b)).toEqual([]);
    // `local` holds it (owner decision 5) and rolls the child back.
    expect(ok<Rolled>(await local.call("agent.session.rollback", { sessionId: c1.id, toRun: 1 })).undone).toEqual([b]);
    expect(await snapshot(local, projectId, c1.rootBranch!)).toEqual(atRun1);
  });

  test("parent_rolls_back_own_child_only: its direct child; SCOPE_DENIED for itself, a sibling, a grandchild and another tree's; a running child SESSION_BUSY", async () => {
    // Opened in order: the parent, child 1, child 2 (running), the grandchild, another root.
    const { local, projectId } = await boot([PARENT, CHILD, WORKING, WORKING, WORKING]);
    const { session: parent } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go" }));
    await until(() => get(parent.id).status === "running", "the parent runs");
    const p = await clientOf(parent.id);
    // Child 1's brief commits on its root (run 1) and stops at `maxTokens`: idle.
    firstWork.push(async (sid) => void (await edit(await clientOf(sid), projectId, get(sid).rootBranch!, 0)));
    const c1 = ok<{ session: Session }>(await p.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })).session;
    const c1Client = await clientOf(c1.id);
    await settled(c1.id, 1);
    expect(srv().core.store.txsOfSession(c1.id)).toHaveLength(1);
    const atRun1 = await snapshot(local, projectId, c1.rootBranch!);
    let b = "";
    await runWith(p, c1.id, async () => void (b = await edit(c1Client, projectId, c1.rootBranch!, 1)));
    const c2 = ok<{ session: Session }>(await p.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })).session;
    // The parent rolls its child back.
    const r = ok<Rolled>(await p.call("agent.session.rollback", { sessionId: c1.id, toRun: 1 }));
    expect(r.undone).toEqual([b]);
    expect(await snapshot(local, projectId, c1.rootBranch!)).toEqual(atRun1);
    expect(srv().core.store.outcomes.of(b)).toEqual([expect.objectContaining({ verdict: "undone", via: "undo", by: "agent", details: { rollback: { toRun: 1, by: `agent:${parent.id}` } } })]);
    // A running child: SESSION_BUSY.
    expect(code(await p.call("agent.session.rollback", { sessionId: c2.id, toRun: 0 }))).toBe("SESSION_BUSY");
    // Child 1's run 3 spawns a grandchild.
    let g = "";
    await runWith(p, c1.id, async () => void (g = ok<{ session: Session }>(await c1Client.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })).session.id));
    const other = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake" })).session;
    const c2Client = await clientOf(c2.id);
    for (const [who, target, what] of [
      [p, parent.id, "itself"],
      [c1Client, c1.id, "itself"],
      [c2Client, c1.id, "a sibling"],
      [c1Client, parent.id, "its parent"],
      [p, g, "a grandchild"],
      [p, other.id, "another tree's"],
      [p, "01NOSUCHSESSION00000000000", "a missing one"],
    ] as const) {
      const e = await who.call("agent.session.rollback", { sessionId: target, toRun: 0 });
      expect(code(e), what).toBe("SCOPE_DENIED");
      expect(details(e), what).toMatchObject({ reason: "notOwnChild" });
    }
  });

  test("unrelated_agent_cannot_rollback: an agent credential that is no session's rolls back nothing (SCOPE_DENIED noSession)", async () => {
    const { local, projectId } = await boot([worker()]);
    const s = await idleRoot(local, projectId);
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, s.session.rootBranch!, 0)));
    await runWith(local, s.session.id, async () => void (await edit(s.client, projectId, s.session.rootBranch!, 1)));
    const owner = await srv().console();
    const minted = ok<{ file: CredentialFile }>(
      await confirmed(owner, "auth.credential.mint", { kind: "agent", label: "hand-made", scopes: ["agent.run", "project.read", "project.write"], projects: [projectId] }),
    );
    const { client, exit } = await srv().proveAs(minted.file);
    ok(exit);
    const e = await client.call("agent.session.rollback", { sessionId: s.session.id, toRun: 1 });
    expect(code(e)).toBe("SCOPE_DENIED");
    expect(details(e)).toMatchObject({ reason: "noSession" });
    expect(srv().core.store.txsOfSession(s.session.id).flatMap((t) => srv().core.store.outcomes.of(t.txId))).toEqual([]);
  });
});

// The built-in loop -------------------------------------------------------------------------

const CATALOGUE = Catalogue.load(fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url)), {
  extra: [{ id: "acme-closed/big", window: 8192, price: { input: 1_000_000, output: 2_000_000, cacheRead: 100_000, cacheWrite: 1_250_000 } }],
});

function scripted(): ModelClient & { requests: ModelRequest[] } {
  const requests: ModelRequest[] = [];
  return {
    preset: "openrouter",
    warnings: [],
    requests,
    wireBody: (req) => req as unknown as Record<string, unknown>,
    async *stream(req): AsyncIterable<ModelStreamEvent> {
      requests.push(structuredClone(req));
      const text = `reply ${requests.length}`;
      const result: ModelResult = {
        text,
        toolCalls: [],
        reasoningText: "",
        reasoning: [],
        stopReason: "endTurn",
        usage: { ...ZERO_USAGE, input: 10, output: 5 },
        usageEstimated: false,
        capture: { request: "{}", response: [] },
        bodyHash: "blake3:00",
        latencyMs: 1,
      };
      yield { type: "textDelta", text };
      yield { type: "done", result };
    },
  };
}

/** Every text a request's messages carry. */
const texts = (req: ModelRequest) =>
  req.messages.flatMap((m) => {
    const x = m as unknown as { text?: string; content?: Array<{ text?: string }> | string };
    return [...(x.text === undefined ? [] : [x.text]), ...(Array.isArray(x.content) ? x.content.map((c) => c.text ?? "") : typeof x.content === "string" ? [x.content] : [])];
  });

describe("the built-in loop", () => {
  test("builtin_history_skips_rolled_back_runs: the next request has no messages of runs 2–3; the nodes stay, shown rolled back", async () => {
    const dir = mkdtempSync(join(tmpdir(), "agent-rollback-builtin-"));
    const clock = manualClock(Date.now());
    const control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    const store = new AgentStore(control, dir, clock, systemIds(clock));
    const client = scripted();
    const registry = new FakeRegistry();
    const outcomes = new OutcomeTracker({ store, lineage: new BranchLineage(), verdicts: { version: () => null, emit: () => false }, clock: clock.now });
    const rollbacks = new Rollbacks({
      store,
      registry: {
        branches: (p, all) => registry.branches(p, all),
        txGet: () => Effect.succeed(null),
        history: () => Effect.die(new Error("no transactions to undo")),
      },
      outcomes,
      ids: systemIds(clock),
    });
    const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client, nodes: (sid) => store.nodes(sid, 1, 1000) });
    const h = makeRuntime({ store, control }, adapter, { clock, registry, rollbacks, pricing: cataloguePricing(CATALOGUE) });
    try {
      await h.runtime.boot();
      const { session } = await Effect.runPromise(h.runtime.start({ projectId: "P1", harness: "builtin", model: "acme-closed/big", prompt: "first" }, PEOPLE, META));
      const idleAfter = async (count: number) => {
        const t0 = Date.now();
        while (store.get(session.id)!.status !== "idle" || store.runs(session.id).length !== count || store.runs(session.id).at(-1)!.status === "running") {
          if (Date.now() - t0 > 5000) throw new Error("not idle");
          await new Promise((r) => setTimeout(r, 5));
        }
      };
      await idleAfter(1);
      for (const [k, text] of [
        [2, "second"],
        [3, "third"],
      ] as const) {
        await Effect.runPromise(h.runtime.prompt(session.id, text, PEOPLE));
        await idleAfter(k);
      }
      expect(texts(client.requests.at(-1)!)).toEqual(expect.arrayContaining(["first", "reply 1", "second", "reply 2", "third"]));
      const r = await Effect.runPromise(h.runtime.rollback(session.id, 1, PEOPLE, META));
      expect(r).toMatchObject({ undone: [], branches: [], conversation: "rolledBack" });
      await Effect.runPromise(h.runtime.prompt(session.id, "fourth", PEOPLE));
      await idleAfter(4);
      const last = texts(client.requests.at(-1)!);
      expect(last).toEqual(expect.arrayContaining(["first", "reply 1", "fourth"]));
      for (const gone of ["second", "reply 2", "third", "reply 3"]) expect(last, gone).not.toContain(gone);
      // The nodes stay for display (`agent show --nodes`), marked.
      const [run1, run2, run3, run4] = store.runs(session.id);
      const all = store.nodes(session.id, 1, 1000);
      const of = (run: Run) => all.filter((x) => x.runId === run.id && x.kind !== "notice");
      expect(of(run2!).length).toBeGreaterThan(0);
      expect([...of(run2!), ...of(run3!)].every((x) => x.rolledBack === true)).toBe(true);
      expect([...of(run1!), ...of(run4!)].some((x) => x.rolledBack === true)).toBe(false);
      expect(nodeLine(of(run2!)[0]!)).toContain("(rolled back)");
    } finally {
      await h.runtime.close();
      await control.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
