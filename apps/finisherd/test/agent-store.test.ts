// The agent session store (plan 7A Task 3b; Decisions 1, 3, 4).
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { measureAgentStoreBudget } from "../scripts/agent-store-budget.ts";
import { writeBlob } from "../src/agent/blobs.ts";
import { Budget, DEFAULT_BUDGET } from "../src/agent/budget.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { MemoryLedger } from "../src/agent/ledger.ts";
import { AgentStore, type NewSession, SERVER_RESTART } from "../src/agent/store.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { makeDiagnostics } from "../src/diagnostics.ts";

const DAY = 86_400_000;
let dir: string;
let control: ControlStore;
let clock: ReturnType<typeof manualClock>;
let store: AgentStore;
let n = 0;

const open = () => {
  control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  store = new AgentStore(control, dir, clock, systemIds(clock));
};

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-store-"));
  clock = manualClock(Date.now());
  open();
});

afterEach(async () => {
  await control.close();
  rmSync(dir, { recursive: true, force: true });
});

const sid = () => systemIds(clock).ulid();

function session(over: Partial<NewSession> = {}): NewSession {
  n++;
  return {
    id: sid(),
    projectId: "P1",
    harness: "builtin",
    provider: "openrouter",
    model: "anthropic/claude-sonnet-5.5",
    weights: "closed",
    role: "editor",
    mode: "draft",
    rootBranch: `agent/editor-${n}`,
    base: { branch: "main", seq: 3 },
    budget: {},
    createdBy: { principal: { kind: "local", id: "local" } },
    ...over,
  };
}

/** A session in `running` with one run. */
async function running(over: Partial<NewSession> = {}) {
  const s = await store.createSession(session(over));
  store.apply(s.id, { kind: "ready" });
  const { run } = store.startRun(s.id, { trigger: "prompt", headAtStart: 3 });
  return { s, run };
}

const count = (table: string, where = "1 = 1", ...args: string[]): number =>
  (control.db.prepare(`SELECT COUNT(*) AS n FROM ${table} WHERE ${where}`).get(...args) as { n: number }).n;

describe("agent store", () => {
  test("agent_chain_is_registered", () => {
    expect(control.db.prepare("SELECT value FROM meta WHERE key = 'formatVersion.agent'").get()).toEqual({ value: "2" });
    for (const t of ["sessions", "runs", "nodes", "session_txs", "session_state", "approvals", "budgets", "budget_days", "session_outbox"]) {
      expect(count(t)).toBe(0);
    }
  });

  test("sessions_follow_the_state_machine", async () => {
    const s = await store.createSession(session());
    expect(s).toMatchObject({ status: "starting", credentialId: s.id, rootBranch: s.rootBranch, base: { branch: "main", seq: 3 } });
    expect(existsSync(join(dir, "agents", s.id, "provider"))).toBe(true);
    expect(store.apply(s.id, { kind: "ready" }).session.status).toBe("idle");
    const { run, session: r } = store.startRun(s.id, { trigger: "prompt", headAtStart: 7 });
    expect(r.status).toBe("running");
    expect(run).toMatchObject({ seq: 1, trigger: "prompt", status: "running", headAtStart: 7 });
    expect(() => store.startRun(s.id)).toThrow(expect.objectContaining({ code: "SESSION_BUSY" }));
    const usage = { input: 10, output: 5, cacheRead: 1, cacheWrite: 0, reasoning: 2, moneyMicros: 30, costBasis: "reported" as const };
    store.addUsage(run.id, usage);
    expect(store.addUsage(run.id, usage).session.usage).toMatchObject({ input: 20, moneyMicros: 60, costBasis: "reported" });
    store.apply(s.id, { kind: "runEnded", runId: run.id, stop: "endTurn" });
    expect(store.run(run.id)).toMatchObject({ status: "completed", stopReason: "endTurn", usage: { input: 20 } });
    expect(store.startRun(s.id).run.seq).toBe(2);
    const ended = store.apply(s.id, { kind: "ended", reason: "user" });
    expect(ended.session).toMatchObject({ status: "ended", statusReason: "user", endedAt: clock.now() });
    expect(ended.effects.map((e) => e.kind)).toEqual(["closeOpenToolCalls", "closeRun", "stopProvider", "revokeCredential", "journal"]);
    expect(store.outbox(s.id).map((o) => o.effect)).toEqual([
      { kind: "journal", phase: "started", reason: null },
      { kind: "revokeCredential" },
      { kind: "journal", phase: "ended", reason: "user" },
    ]);
    expect(() => store.apply(s.id, { kind: "resumed" })).toThrow(expect.objectContaining({ code: "SESSION_ENDED" }));
    expect(() => store.apply("nope", { kind: "ready" })).toThrow(expect.objectContaining({ code: "SESSION_NOT_FOUND" }));
  });

  test("plan_sessions_have_no_root_branch", async () => {
    await expect(store.createSession(session({ mode: "plan" }))).rejects.toMatchObject({ code: "INVALID_REQUEST" });
    const { rootBranch: _, ...rest } = session();
    await expect(store.createSession({ ...rest, mode: "draft" })).rejects.toMatchObject({ code: "INVALID_REQUEST" });
    const plan = await store.createSession({ ...rest, mode: "plan" });
    expect(plan).not.toHaveProperty("rootBranch");
    expect(store.state(plan.id)).toEqual({ defaultBranch: "main", tries: [] });
  });

  test("node_blob_threshold", async () => {
    const { s, run } = await running();
    // `JSON.stringify` of a string of k ASCII characters is k + 2 bytes.
    const inline = await store.appendNode({ sessionId: s.id, runId: run.id, kind: "assistantMessage", payload: "a".repeat(4094) });
    const blob = await store.appendNode({ sessionId: s.id, runId: run.id, kind: "assistantMessage", payload: "b".repeat(4095) });
    expect(inline.node.payload).toBe("a".repeat(4094));
    expect(inline.node).not.toHaveProperty("blobRef");
    expect(blob.node).not.toHaveProperty("payload");
    expect(blob.node.blobRef).toMatch(/^[0-9a-f]{64}$/);
    expect(existsSync(join(dir, "agents", s.id, "blobs", blob.node.blobRef!))).toBe(true);
    expect(await store.payload(blob.node)).toBe("b".repeat(4095));
    // A crash between the blob and its row leaves an orphan blob, never a row naming nothing.
    const orphan = await writeBlob(store.root, s.id, Buffer.from(JSON.stringify("c".repeat(5000))));
    expect(await store.sweepOrphans()).toBe(0); // within the grace: it may be an append in flight
    clock.advance(2 * 60 * 60_000);
    expect(await store.sweepOrphans()).toBe(1);
    expect(readdirSync(join(dir, "agents", s.id, "blobs"))).toEqual([blob.node.blobRef]);
    expect(orphan).not.toBe(blob.node.blobRef);
    // A row that cannot be written takes its blob with it.
    await expect(store.appendNode({ sessionId: s.id, runId: "nope", kind: "notice", payload: "d".repeat(5000) })).rejects.toMatchObject({
      code: "INVALID_REQUEST",
    });
    expect(readdirSync(join(dir, "agents", s.id, "blobs"))).toEqual([blob.node.blobRef]);
    for (const r of control.db.prepare("SELECT sessionId, blobRef FROM nodes WHERE blobRef IS NOT NULL").all() as Array<{ sessionId: string; blobRef: string }>) {
      expect(existsSync(join(dir, "agents", r.sessionId, "blobs", r.blobRef))).toBe(true);
    }
  });

  test("nodes_keep_call_order", async () => {
    const { s, run } = await running();
    const big = store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "C1", payload: "x".repeat(200_000) });
    const small = store.appendNode({ sessionId: s.id, runId: run.id, kind: "notice", payload: "after" });
    const [a, b] = await Promise.all([big, small]);
    expect([a.node.seq, b.node.seq]).toEqual([1, 2]);
    expect(store.nodes(s.id).map((x) => x.kind)).toEqual(["toolCall", "notice"]);
    expect(store.nodes(s.id, 2).map((x) => x.seq)).toEqual([2]);
    await expect(store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", payload: {} })).rejects.toMatchObject({ code: "INVALID_REQUEST" });
    await expect(store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", callId: "C9", payload: {} })).rejects.toMatchObject({
      code: "INVALID_REQUEST",
    });
    const res = await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", callId: "C1", payload: { ok: true } });
    expect(res.inserted).toBe(true);
    const again = await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", callId: "C1", payload: { ok: false } });
    expect(again).toEqual({ node: res.node, inserted: false });
  });

  test("recover_marks_interrupted_and_closes_open_calls", async () => {
    const { s, run } = await running();
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "C-open", providerId: "toolu_1", payload: { argv: ["status"] } });
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "C-done", payload: {} });
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", callId: "C-done", payload: { ok: true } });
    const waiting = await running();
    const { approval } = store.openApproval({ sessionId: waiting.s.id, kind: "budget", subject: { kind: "money" }, argsHash: "h", expiresAt: clock.now() + 90_000 });
    expect(store.get(waiting.s.id)!.status).toBe("waitingApproval");
    const starting = await store.createSession(session());
    const idle = await store.createSession(session());
    store.apply(idle.id, { kind: "ready" });
    for (const o of store.outbox()) store.outboxDone(o.id);
    // The crash: the process goes, the next start opens the same control.db.
    await control.close();
    clock.advance(1000);
    open();
    const r = store.recoverAfterCrash();
    expect(r).toEqual({ interrupted: [s.id, waiting.s.id, starting.id].sort(), closedCalls: 1 });
    expect(store.get(s.id)).toMatchObject({ status: "interrupted", statusReason: SERVER_RESTART });
    expect(store.get(idle.id)!.status).toBe("idle");
    expect(store.run(run.id)).toMatchObject({ status: "interrupted", stopReason: SERVER_RESTART });
    expect(store.approvals(waiting.s.id)).toEqual([expect.objectContaining({ id: approval.id, status: "expired" })]);
    const results = store.nodes(s.id).filter((x) => x.kind === "toolResult");
    expect(results).toHaveLength(2);
    expect(results[1]).toMatchObject({ callId: "C-open", providerId: "toolu_1", payload: { ok: false, code: "SESSION_INTERRUPTED", synthetic: true } });
    // Each session's entries in order: its synthetic results before its journal phase.
    expect(store.outbox(s.id).map((o) => o.effect)).toEqual([
      { kind: "toolResult", nodeId: results[1]!.id, callId: "C-open", runId: run.id },
      { kind: "journal", phase: "interrupted", reason: SERVER_RESTART },
    ]);
    for (const id of [waiting.s.id, starting.id]) {
      expect(store.outbox(id).map((o) => o.effect)).toEqual([{ kind: "journal", phase: "interrupted", reason: SERVER_RESTART }]);
    }
    // Exactly once: a second recovery (another restart before anything ran) finds nothing.
    await control.close();
    open();
    expect(store.recoverAfterCrash()).toEqual({ interrupted: [], closedCalls: 0 });
    expect(count("nodes", "kind = 'toolResult'")).toBe(2);
    expect(store.outbox()).toHaveLength(4); // still owed until done
    // A late real result is not written over the synthetic one; no call starts in a closed run.
    expect((await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolResult", callId: "C-open", payload: { ok: true } })).inserted).toBe(false);
    await expect(store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "C-late", payload: {} })).rejects.toMatchObject({
      code: "SESSION_INTERRUPTED",
    });
    // Resume after recovery: a fresh run, the credential refresh owed.
    expect(store.apply(s.id, { kind: "resumed" }).session.status).toBe("idle");
    expect(store.outbox(s.id).at(-1)!.effect).toEqual({ kind: "journal", phase: "resumed", reason: null });
  });

  test("recover_after_sigkill", async () => {
    const script = join(dir, "crash.mjs");
    const src = (p: string) => JSON.stringify(pathToFileURL(join(import.meta.dirname, "..", "src", p)).href);
    writeFileSync(
      script,
      `const { openControlStore } = await import(${src("control/db.ts")});
       const { AgentStore } = await import(${src("agent/store.ts")});
       const { systemIds, systemClock } = await import(${src("agent/clock.ts")});
       const control = openControlStore(process.argv[2], { serverId: "sid", checkpoint: false });
       const store = new AgentStore(control, process.argv[2], systemClock, systemIds());
       const s = await store.createSession(JSON.parse(process.argv[3]));
       store.apply(s.id, { kind: "ready" });
       const { run } = store.startRun(s.id);
       await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "K1", payload: { big: "z".repeat(9000) } });
       process.kill(process.pid, "SIGKILL");`,
    );
    await control.close();
    const crashDir = join(dir, "crash");
    mkdirSync(crashDir);
    const child = spawnSync(process.execPath, [script, crashDir, JSON.stringify(session())], { encoding: "utf8", env: process.env });
    if (process.platform !== "win32") expect(child.signal, child.stderr).toBe("SIGKILL");
    control = openControlStore(crashDir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, crashDir, clock, systemIds(clock));
    const r = store.recoverAfterCrash();
    expect(r.interrupted).toHaveLength(1);
    expect(r.closedCalls).toBe(1);
    const [id] = r.interrupted;
    expect(store.nodes(id!).map((x) => [x.kind, x.callId])).toEqual([
      ["toolCall", "K1"],
      ["toolResult", "K1"],
    ]);
    expect(await store.payload(store.nodes(id!)[0]!)).toEqual({ big: "z".repeat(9000) });
  });

  test("session_txs_records_every_agent_commit", async () => {
    const a = await store.createSession(session());
    const b = await store.createSession(session());
    store.recordTx({ txId: "T1", sessionId: a.id, callId: "C1", attribution: "exact", at: 10 });
    store.recordTx({ txId: "T2", sessionId: a.id, callId: null, attribution: "none", at: 20 });
    store.recordTx({ txId: "T3", sessionId: b.id, callId: "C3", attribution: "inferred", at: 30 });
    store.recordTx({ txId: "T1", sessionId: b.id, callId: null, attribution: "none", at: 40 }); // a retried after stage: kept as first written
    const m = store.sessionOfTx(["T1", "T2", "T3", "T-human"]);
    expect([...m.keys()].sort()).toEqual(["T1", "T2", "T3"]);
    expect(m.get("T1")).toEqual({ txId: "T1", sessionId: a.id, callId: "C1", attribution: "exact", at: 10 });
    expect(m.get("T2")).toMatchObject({ sessionId: a.id, callId: null, attribution: "none" });
    expect(store.txsOfSession(a.id).map((t) => t.txId)).toEqual(["T1", "T2"]);
    expect(store.txsOfSession(a.id, 15).map((t) => t.txId)).toEqual(["T2"]);
    expect(store.txsOfSession(b.id).map((t) => t.txId)).toEqual(["T3"]);
    expect(store.txsOfCall("C1").map((t) => t.txId)).toEqual(["T1"]);
    expect(store.setOutcome("T2", "undone")).toBe(true);
    expect(store.setOutcome("T-human", "undone")).toBe(false);
    expect(store.sessionOfTx(["T2"]).get("T2")!.outcome).toBe("undone");
    expect(store.sessionOfTx([]).size).toBe(0);
    expect(() => store.recordTx({ txId: "T9", sessionId: a.id, callId: null, attribution: "guess" as never, at: 1 })).toThrow(
      expect.objectContaining({ code: "INVALID_REQUEST" }),
    );
  });

  test("session_state_is_per_session", async () => {
    const a = await store.createSession(session());
    const b = await store.createSession(session());
    expect(store.state(a.id)).toEqual({ defaultBranch: a.rootBranch, tries: [] });
    store.recordTry(a.id, `${a.rootBranch}/try-1`);
    store.setDefaultBranch(a.id, `${a.rootBranch}/try-1`);
    store.recordTry(a.id, `${a.rootBranch}/try-2`);
    store.setDefaultBranch(a.id, `${a.rootBranch}/try-2`);
    expect(store.state(b.id)).toEqual({ defaultBranch: b.rootBranch, tries: [] });
    expect(() => store.setDefaultBranch(b.id, `${a.rootBranch}/try-1`)).toThrow(expect.objectContaining({ code: "BRANCH_NOT_ALLOWED" }));
    expect(store.state(a.id)).toEqual({
      defaultBranch: `${a.rootBranch}/try-2`,
      tries: [
        { branch: `${a.rootBranch}/try-1`, previousDefault: a.rootBranch },
        { branch: `${a.rootBranch}/try-2`, previousDefault: `${a.rootBranch}/try-1` },
      ],
    });
    // Clearing the inner try restores its default; clearing the outer one hands its default on.
    expect(store.clearTry(a.id, `${a.rootBranch}/try-2`).defaultBranch).toBe(`${a.rootBranch}/try-1`);
    store.recordTry(a.id, `${a.rootBranch}/try-3`);
    store.setDefaultBranch(a.id, `${a.rootBranch}/try-3`);
    expect(store.clearTry(a.id, `${a.rootBranch}/try-1`)).toEqual({
      defaultBranch: `${a.rootBranch}/try-3`,
      tries: [{ branch: `${a.rootBranch}/try-3`, previousDefault: a.rootBranch }],
    });
    expect(store.clearTry(a.id, `${a.rootBranch}/try-3`)).toEqual({ defaultBranch: a.rootBranch, tries: [] });
    expect(store.setDefaultBranch(a.id, a.rootBranch!).defaultBranch).toBe(a.rootBranch);
    expect(store.state(b.id)).toEqual({ defaultBranch: b.rootBranch, tries: [] });
    store.apply(b.id, { kind: "ended", reason: "user" });
    expect(() => store.recordTry(b.id, "x")).toThrow(expect.objectContaining({ code: "SESSION_ENDED" }));
  });

  test("approvals_open_answer_and_consume", async () => {
    const { s } = await running();
    const { approval, session: w } = store.openApproval({ sessionId: s.id, kind: "budget", subject: { kind: "money", add: 5 }, argsHash: "h", expiresAt: clock.now() + DAY });
    expect(w.status).toBe("waitingApproval");
    expect(approval).toMatchObject({ status: "open", subject: { kind: "money", add: 5 } });
    const answered = store.answerApproval(approval.id, "allow", "console:console");
    expect(answered.session.status).toBe("running");
    expect(answered.approval).toMatchObject({ status: "answered", decision: "allow", by: "console:console", answeredAt: clock.now() });
    expect(() => store.answerApproval(approval.id, "deny", "x")).toThrow(expect.objectContaining({ code: "INVALID_REQUEST" }));
    expect(store.consumeApproval(approval.id)).toBe(true);
    expect(store.consumeApproval(approval.id)).toBe(false);
    // An approval nothing can wait on (an idle session) is expired at once.
    const idle = await store.createSession(session());
    store.apply(idle.id, { kind: "ready" });
    expect(store.openApproval({ sessionId: idle.id, kind: "rule", subject: {}, argsHash: "h", expiresAt: 1 }).approval.status).toBe("expired");
  });

  test("ledger_matches_the_memory_ledger", async () => {
    const s = await store.createSession(session());
    const db = store.ledger();
    const mem = new MemoryLedger();
    for (const l of [db, mem]) {
      l.setLimit(s.id, "tokens", 100);
      l.add(s.id, "tokens", { used: 40, reserved: 30 });
      l.add(s.id, "tokens", { reserved: -50 });
      l.add(s.id, "money", { used: 7 });
      l.addDaily("2026-11-08", { used: 5, reserved: 2 });
      l.addDaily("2026-11-08", { reserved: -9 });
    }
    for (const k of ["tokens", "money", "wallMs"] as const) expect(db.read(s.id, k)).toEqual(mem.read(s.id, k));
    expect(db.daily("2026-11-08")).toEqual(mem.daily("2026-11-08"));
    expect(db.daily("2026-11-09")).toEqual({ used: 0, reserved: 0 });
    // `Budget` over either ledger reads the same.
    const t = await store.createSession(session());
    const limits = { ...DEFAULT_BUDGET, tokens: 1000, moneyMicros: 5000 };
    const readings = [db, mem].map((l) => {
      const b = new Budget(l, t.id, limits, clock);
      b.startRun();
      const use = { input: 600, output: 250, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
      const allowed = b.canCall(use);
      return { allowed, crossed: b.charge(use, 4500), readings: b.readings() };
    });
    expect(readings[0]).toEqual(readings[1]);
    expect(readings[0]!.crossed.notices.map((r) => r.kind).sort()).toEqual(["money", "tokens"]);
  });

  test("retention_sweeps_ended_sessions_and_blobs", async () => {
    const old = await running();
    await store.appendNode({ sessionId: old.s.id, runId: old.run.id, kind: "assistantMessage", payload: "o".repeat(9000) });
    store.recordTx({ txId: "T-old", sessionId: old.s.id, callId: null, attribution: "none", at: 1 });
    store.ledger().add(old.s.id, "tokens", { used: 1 });
    store.apply(old.s.id, { kind: "ended", reason: "user" });
    clock.advance(60 * DAY);
    const recent = await running();
    store.apply(recent.s.id, { kind: "ended", reason: "user" });
    const live = await running();
    await store.appendNode({ sessionId: live.s.id, runId: live.run.id, kind: "assistantMessage", payload: "l".repeat(9000) });
    // A directory no row names (a crash during a create or a delete), old enough.
    mkdirSync(join(dir, "agents", "01ORPHAN"));
    const past = (clock.now() - 2 * 60 * 60_000) / 1000;
    utimesSync(join(dir, "agents", "01ORPHAN"), past, past);
    clock.advance(31 * DAY);
    expect(await store.sweep(clock.now())).toBe(1);
    expect(store.get(old.s.id)).toBeUndefined();
    for (const t of ["runs", "nodes", "session_state", "budgets", "approvals"]) expect(count(t, "sessionId = ?", old.s.id)).toBe(0);
    expect(existsSync(join(dir, "agents", old.s.id))).toBe(false);
    expect(existsSync(join(dir, "agents", "01ORPHAN"))).toBe(false);
    expect(store.get(recent.s.id)).toBeDefined();
    expect(store.nodes(live.s.id)).toHaveLength(1);
    expect(await store.payload(store.nodes(live.s.id)[0]!)).toBe("l".repeat(9000));
    expect(await store.sweep(clock.now() + 30 * DAY)).toBe(0);
    expect(await store.sweep(clock.now() + 60 * DAY)).toBe(1); // `recent` ended 91 days before
  });

  test("delete_removes_rows_and_dir", async () => {
    const { s, run } = await running();
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "toolCall", callId: "D1", payload: "q".repeat(9000) });
    store.recordTx({ txId: "TD", sessionId: s.id, callId: "D1", attribution: "exact", at: 1 });
    store.openApproval({ sessionId: s.id, kind: "rule", subject: {}, argsHash: "h", expiresAt: 1 });
    const other = await running();
    const { effects } = await store.delete(s.id);
    expect(effects.map((e) => e.kind)).toEqual(["closeOpenToolCalls", "closeRun", "expireApprovals", "stopProvider", "revokeCredential", "journal"]);
    expect(store.get(s.id)).toBeUndefined();
    for (const t of ["runs", "nodes", "session_state", "budgets", "approvals"]) expect(count(t, "sessionId = ?", s.id)).toBe(0);
    // Owner decision: `session_txs` outlive the session (late merges are still attributed).
    expect(store.txsOfSession(s.id).map((t) => t.txId)).toEqual(["TD"]);
    expect(existsSync(join(dir, "agents", s.id))).toBe(false);
    // The revocation and the journal entry outlive the rows until they are done.
    expect(store.outbox(s.id).map((o) => o.effect.kind)).toEqual(["journal", "toolResult", "revokeCredential", "journal"]);
    expect(store.get(other.s.id)!.status).toBe("running");
    await expect(store.delete(s.id)).rejects.toMatchObject({ code: "SESSION_NOT_FOUND" });
  });

  test("diagnostics_purge_and_export_leave_nodes_alone", async () => {
    const { s, run } = await running();
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "userMessage", payload: { text: "cut the intro" } });
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "assistantMessage", payload: "m".repeat(9000) });
    const before = ["sessions", "runs", "nodes", "session_txs", "session_state"].map((t) => count(t));
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const d = yield* makeDiagnostics(dir);
          yield* d.export({ filter: {} }, false);
          yield* d.export({ filter: {}, asUpload: "diagnostics" }, true);
          yield* d.purge("all");
        }),
      ),
    );
    await Effect.runPromise(new Audit(control, () => clock.now()).purge({ all: true }));
    expect(["sessions", "runs", "nodes", "session_txs", "session_state"].map((t) => count(t))).toEqual(before);
    expect(await store.payload(store.nodes(s.id)[1]!)).toBe("m".repeat(9000));
  });

  test("statement_budget", { timeout: 120_000 }, async () => {
    // Load-sensitive (TASKS.md pattern): strict p99 ≤ 5 ms only with FIN_STRICT_TIMING=1;
    // otherwise every median ≤ 1 ms and every p99 ≤ 50 ms. The full 10k/1M run is the script's.
    const strict = process.env.FIN_STRICT_TIMING === "1";
    const r = await measureAgentStoreBudget(mkdtempSync(join(dir, "budget-")), 2000, 200_000);
    console.log(`agent store p99 (ms): ${JSON.stringify(r.p99Ms)}; p50 ${JSON.stringify(r.p50Ms)}`);
    for (const [k, v] of Object.entries(r.p50Ms)) expect(v, `${k} median`).toBeLessThanOrEqual(1);
    for (const [k, v] of Object.entries(r.p99Ms)) expect(v, k).toBeLessThanOrEqual(strict ? 5 : 50);
  });
});
