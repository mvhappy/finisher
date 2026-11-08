// The 3b/6b/7 review of 7A-2 (store, credentials, budgets, the runtime and `agent.watch`): each
// finding's regression, over Task 7's real core and the fake provider.
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { FinError, Node, Principal, Session, WatchItem } from "@finisher/contracts";
import { Cause, Deferred, Effect, Exit, Fiber, Option, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Budget, DEFAULT_BUDGET } from "../src/agent/budget.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { credentialPath, MODE_SCOPES, SessionCredentials } from "../src/agent/credentials.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { MemoryLedger } from "../src/agent/ledger.ts";
import { agentChainHooks } from "../src/agent/reach.ts";
import { REPORTED_PRICING, type ToolCall, type ToolExecutor } from "../src/agent/runtime-deps.ts";
import { approvalInReach, projectsFilter, sessionInReach } from "../src/agent/session-access.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { WatchHub } from "../src/agent/watch-hub.ts";
import { Credentials } from "../src/auth/credentials.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { makeRuntime, META, PEOPLE, planner, testCore, TEST_PRICING } from "./support/runtime-doubles.ts";

const DAY = 86_400_000;
let dir: string;
let control: ControlStore;
let clock: ReturnType<typeof manualClock>;
let store: AgentStore;
const cleanups: (() => Promise<void>)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-review-"));
  clock = manualClock(Date.now());
  control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  store = new AgentStore(control, dir, clock, systemIds(clock));
});

afterEach(async () => {
  for (const c of cleanups.splice(0)) await c();
  await control.close();
  rmSync(dir, { recursive: true, force: true });
});

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = async (e: Effect.Effect<unknown, FinError>) => Effect.runPromise(Effect.flip(e));

async function until(f: () => boolean, what = "condition", ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

function harness(adapter: ReturnType<typeof FakeAdapter>, opts: Parameters<typeof makeRuntime>[2] = {}) {
  const h = makeRuntime({ store, control }, adapter, { clock, ...opts });
  cleanups.push(() => h.runtime.close());
  return h;
}

const idleScript: FakeScript = { name: "idle", turns: [] };
const status = (id: string) => store.get(id)!.status;
/** The session's open approvals (the state machine's list, `sessions.approvals`). */
const openList = (id: string) => JSON.parse((control.db.prepare("SELECT approvals FROM sessions WHERE id = ?").get(id) as { approvals: string }).approvals) as string[];
const payload = (n: Node) => n.payload as Record<string, unknown>;
const newSession = (over: Partial<NewSession> = {}): NewSession => ({
  id: systemIds(clock).ulid(),
  projectId: "P1",
  harness: "fake",
  provider: "openrouter",
  model: "test/m",
  weights: "closed",
  role: "editor",
  mode: "draft",
  rootBranch: "br1",
  base: { branch: "main", seq: 3 },
  budget: {},
  createdBy: { principal: { kind: "local", id: "local" } },
  ...over,
});

describe("1. a revocation is never lost", () => {
  test("handle keeps the item queued until the credential's end is written", async () => {
    const audit = new Audit(control, () => clock.now());
    const auth = new Credentials(control, Buffer.alloc(32, 7), audit, () => clock.now());
    let writable = false;
    const creds = new SessionCredentials({
      store,
      credentials: auth,
      revoke: {
        audit,
        tickets: { dropWhere: () => 0 },
        connections: { closeWhere: () => 0, revoke: () => {} },
        revoke: (ref) => auth.markEnded(ref, "owner"),
        persist: () => (writable ? auth.persist() : undefined),
      },
    });
    const s = await store.createSession(newSession());
    await run(creds.mintForSession(s, "br1", PEOPLE));
    store.apply(s.id, { kind: "ready" });
    store.apply(s.id, { kind: "ended", reason: "userEnded" });
    // A busy store: ended in memory only, so the item stays owed (a restart would revive it).
    expect(creds.drain(s.id)).toBe(0);
    expect(auth.get(s.id)!.status).toBe("revoked");
    expect(store.outbox(s.id).map((o) => o.effect.kind)).toEqual(["journal", "revokeCredential", "journal"]);
    writable = true;
    expect(creds.drain(s.id)).toBe(1);
    expect(auth.unsaved()).toBe(0);
    expect(store.outbox(s.id).map((o) => o.effect.kind)).toEqual(["journal", "journal"]);
  });

  test("reconcile ends live session credentials of ended or missing sessions only", async () => {
    const h = harness(FakeAdapter([idleScript, idleScript, idleScript]));
    await h.runtime.boot();
    const ended = (await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META))).session;
    const gone = (await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META))).session;
    const live = (await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META))).session;
    const minted = h.core.auth.mint({ ...PEOPLE, kind: "console", id: "console", human: true }, { kind: "agent", label: "my script", projects: ["P1"] });
    store.apply(ended.id, { kind: "ended", reason: "userEnded" });
    await store.delete(gone.id);
    for (const item of store.outbox()) store.outboxDone(item.id);
    expect(h.core.credentials.reconcile()).toBe(2);
    expect([ended.id, gone.id, live.id, minted.credential.id].map((id) => h.core.auth.get(id)!.status)).toEqual(["revoked", "revoked", "live", "live"]);
    expect(h.core.credentials.reconcile()).toBe(0);
  });
});

describe("2. approvals expire", () => {
  async function interruptedWithApproval(expiresIn: number) {
    const s = await store.createSession(newSession());
    store.apply(s.id, { kind: "ready" });
    store.apply(s.id, { kind: "interrupted", reason: "BUDGET_EXCEEDED" });
    const { approval } = store.openApproval({ sessionId: s.id, kind: "budget", subject: {}, argsHash: "budget:money", expiresAt: clock.now() + expiresIn });
    expect(openList(s.id)).toEqual([approval.id]);
    return { s, approval };
  }

  test("an answer after expiresAt is APPROVAL_EXPIRED and the approval leaves the open list", async () => {
    const { s, approval } = await interruptedWithApproval(1000);
    clock.advance(1000);
    expect(() => store.answerApproval(approval.id, "allow", "console:console")).toThrow(expect.objectContaining({ code: "APPROVAL_EXPIRED" }));
    expect(store.approval(approval.id)!.status).toBe("expired");
    expect(status(s.id)).toBe("interrupted");
    expect(openList(s.id)).toEqual([]);
    expect(() => store.answerApproval(approval.id, "allow", "console:console")).toThrow(expect.objectContaining({ code: "INVALID_REQUEST" }));
  });

  test("the hourly sweep expires overdue approvals", async () => {
    const h = harness(FakeAdapter([idleScript]));
    await h.runtime.boot();
    const overdue = await interruptedWithApproval(DAY);
    const fresh = await interruptedWithApproval(3 * DAY);
    clock.advance(2 * DAY);
    await h.runtime.sweep();
    expect(store.approval(overdue.approval.id)!.status).toBe("expired");
    expect(openList(overdue.s.id)).toEqual([]);
    expect(store.approval(fresh.approval.id)!.status).toBe("open");
    expect(openList(fresh.s.id)).toEqual([fresh.approval.id]);
    expect((await failure(h.runtime.approve(overdue.approval.id, "allow", { moneyMicros: 1 }, PEOPLE))).code).toBe("INVALID_REQUEST");
  });
});

describe("3. budgets fail closed", () => {
  test("a non-local model without a price is refused at start; a local one starts at zero", async () => {
    const h = harness(FakeAdapter([idleScript]), { pricing: REPORTED_PRICING });
    await h.runtime.boot();
    const e = await failure(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    expect(e).toMatchObject({ code: "PRICING_UNKNOWN", details: { provider: "openrouter", model: "test/closed-explicit" } });
    expect(store.list().sessions).toEqual([]);
    expect(h.registry.created).toEqual([]);
    const local = harness(FakeAdapter([idleScript]), { pricing: REPORTED_PRICING, planner: planner({ provider: "local" }) });
    await local.runtime.boot();
    expect((await run(local.runtime.start({ projectId: "P1" }, PEOPLE, META))).session.provider).toBe("local");
  });

  test("concurrent calls hold their projected tokens, so they cannot overshoot together", () => {
    const ledger = new MemoryLedger();
    const b = new Budget(ledger, "s", { ...DEFAULT_BUDGET, tokens: 1000 }, manualClock());
    const u = { input: 300, output: 300, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
    const first = b.canCall(u);
    expect(first).toMatchObject({ ok: true, reservation: { tokens: 600 } });
    expect(b.canCall(u)).toMatchObject({ ok: false, kind: "tokens", used: 600, limit: 1000 });
    b.charge({ ...u, output: 100 }, 0, { reservation: first.ok ? first.reservation : null });
    expect(ledger.read("s", "tokens")).toMatchObject({ used: 400, reserved: 0 });
    expect(b.canCall(u)).toMatchObject({ ok: true });
  });

  test("a non-local call that reports no cost is charged its projection", async () => {
    const usage = { type: "usage", input: 10, output: 10, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
    const adapter = FakeAdapter([{ turns: [[{ emit: [{ type: "toolStarted", providerId: "c1", tool: "fin", input: { argv: ["view"] } }] }, { awaitTool: "c1" }, { emit: [usage] }]] }]);
    const unpriced = { ...TEST_PRICING, projected: () => 700, price: () => ({ micros: 0, basis: "unknown" as const }) };
    const h = harness(adapter, { pricing: unpriced });
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1, "the tool running");
    expect(adapter.log.opened[0]!.spec.gate!.beforeCall({ input: 10, output: 10, cacheRead: 0, cacheWrite: 0, reasoning: 0 })).toEqual({ ok: true });
    h.tool.release();
    await until(() => store.ledger().read(session.id, "money").used > 0, "the usage charged");
    expect(store.ledger().read(session.id, "money")).toMatchObject({ used: 700, reserved: 0 });
    expect(payload(store.nodes(session.id).find((n) => n.kind === "usage")!)).toMatchObject({ moneyMicros: 700 });
  });
});

describe("4. a lagging watcher is ended, never dropped from silently", () => {
  test("WATCH_LAGGING once a watcher holds bufferSize live items unread", async () => {
    const hub = new WatchHub();
    const session = { id: "S", status: "running" } as unknown as Session;
    const nodes: Node[] = [1, 2].map((seq) => ({ id: `n${seq}`, sessionId: "S", runId: "r", seq, kind: "notice", payload: {}, at: 0 }) as unknown as Node);
    const source = { get: () => session, nodes: (_: string, from = 1) => nodes.filter((n) => n.seq >= from), lastSeq: () => nodes.length };
    const got: WatchItem[] = [];
    const exit = await Effect.runPromise(
      Effect.gen(function* () {
        const gate = yield* Deferred.make<void>();
        const fiber = yield* Effect.forkChild(
          Effect.exit(Stream.runForEach(hub.watch(source, "S", 1, 3), (i) => Effect.andThen(Deferred.await(gate), Effect.sync(() => void got.push(i))))),
        );
        // The watcher is stuck on its first item: the live items pile up.
        yield* Effect.sleep("20 millis");
        for (let seq = 3; seq <= 10; seq++) {
          const node = { ...nodes[0]!, id: `n${seq}`, seq };
          nodes.push(node);
          hub.publish("S", { type: "nodeAdded", node });
        }
        yield* Deferred.succeed(gate, undefined);
        return yield* Fiber.join(fiber);
      }),
    );
    expect(Exit.isFailure(exit)).toBe(true);
    const err = Exit.isFailure(exit) ? Option.getOrUndefined(Cause.findErrorOption(exit.cause)) : undefined;
    expect(err).toMatchObject({ code: "WATCH_LAGGING", details: { sessionId: "S", bufferSize: 3 } });
    const seqs = got.flatMap((i) => (i.type === "nodeAdded" ? [i.node.seq] : []));
    // Every item delivered is in order and none was skipped; the client resumes after the last.
    expect(seqs).toEqual(seqs.map((_, k) => k + 1));
    expect(seqs.length).toBeLessThan(10);
    expect((err as FinError).details).toMatchObject({ lastSeq: expect.any(Number) });
    expect(((err as FinError).details as { lastSeq: number }).lastSeq).toBeGreaterThanOrEqual(seqs.at(-1)!);
  });
});

describe("5. session RPCs check the project reach", () => {
  test("sessionInReach and approvalInReach answer NOT_FOUND outside the principal's projects", async () => {
    const s = await store.createSession(newSession({ projectId: "B" }));
    store.apply(s.id, { kind: "ready" });
    store.apply(s.id, { kind: "interrupted", reason: "BUDGET_EXCEEDED" });
    const { approval } = store.openApproval({ sessionId: s.id, kind: "budget", subject: {}, argsHash: "h", expiresAt: clock.now() + DAY });
    const only = (projects: Principal["projects"]): Principal => ({ kind: "device", id: "d", scopes: [], projects, branches: "*", human: true });
    expect((await run(sessionInReach(store, only("*"), s.id))).id).toBe(s.id);
    expect((await run(sessionInReach(store, only(["A", "B"]), s.id))).id).toBe(s.id);
    expect(await failure(sessionInReach(store, only(["A"]), s.id))).toMatchObject({ code: "SESSION_NOT_FOUND" });
    expect((await failure(sessionInReach(store, only(["A"]), s.id))).message).toBe((await failure(sessionInReach(store, only("*"), "nope"))).message);
    expect((await run(approvalInReach(store, only(["B"]), approval.id))).session.id).toBe(s.id);
    expect(await failure(approvalInReach(store, only(["A"]), approval.id))).toMatchObject({ code: "APPROVAL_NOT_FOUND" });
    expect(projectsFilter(only("*"))).toBeUndefined();
    expect(store.list({ projects: projectsFilter(only(["A"]))! }).sessions).toEqual([]);
  });
});

describe("6. reservations do not survive a crash", () => {
  test("recoverAfterCrash returns every reservation before the outbox drains", () => {
    const ledger = store.ledger();
    ledger.add("S", "money", { used: 5, reserved: 900 });
    ledger.add("S", "tokens", { reserved: 400 });
    ledger.addDaily("2026-11-08", { used: 5, reserved: 900 });
    store.recoverAfterCrash();
    expect(ledger.read("S", "money")).toMatchObject({ used: 5, reserved: 0 });
    expect(ledger.read("S", "tokens")).toMatchObject({ reserved: 0 });
    expect(ledger.daily("2026-11-08")).toEqual({ used: 5, reserved: 0 });
  });
});

describe("7. a budget stop has a deadline", () => {
  test("a tool that never answers is aborted after stopGraceMs and the session stops", async () => {
    const signals: AbortSignal[] = [];
    const stuck: ToolExecutor = { run: (_c: ToolCall, signal: AbortSignal) => (signals.push(signal), new Promise(() => {})) };
    const script: FakeScript = {
      name: "stuck",
      turns: [[{ emit: [{ type: "toolStarted", providerId: "c1", tool: "fin", input: { argv: ["view"] } }, { type: "usage", input: 5000, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 }] }, { awaitTool: "c1" }]],
    };
    const h = harness(FakeAdapter([script]), { tools: { fake: stuck }, stopGraceMs: 50 });
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => signals.length === 1 && store.nodes(session.id).some((n) => n.kind === "usage"), "the call and its usage");
    expect(status(session.id)).toBe("running");
    await until(() => status(session.id) === "interrupted", "the stop after the grace period", 2000);
    expect(store.get(session.id)!.statusReason).toBe("BUDGET_EXCEEDED");
    expect(signals[0]!.aborted).toBe(true);
    const result = store.nodes(session.id).find((n) => n.kind === "toolResult")!;
    expect(payload(result)).toMatchObject({ ok: false, code: "SESSION_INTERRUPTED", synthetic: true });
    expect(store.approvals(session.id)).toMatchObject([{ kind: "budget", status: "open" }]);
  });
});

describe("8, 9, 11. minting a session's credential", () => {
  test("a session that reached idle is never minted again; one that did not is, past a stale temp file", async () => {
    const core = testCore(store, control, clock);
    const ready = await store.createSession(newSession());
    await run(core.credentials.mintForSession(ready, "br1", PEOPLE));
    store.apply(ready.id, { kind: "ready" });
    store.apply(ready.id, { kind: "interrupted", reason: "serverRestart" });
    core.auth.end({ kind: "agent", id: ready.id }, "expired");
    // 181 days later the ended row is pruned when the credentials load again.
    clock.advance(181 * DAY);
    const later = testCore(store, control, clock);
    expect(later.auth.get(ready.id)).toBeUndefined();
    expect(await failure(later.credentials.refreshForSession(store.get(ready.id)!))).toMatchObject({ code: "CREDENTIAL_EXPIRED" });
    expect(later.auth.get(ready.id)).toBeUndefined();
    // A crash between the record and the mint: the session never got to idle.
    const unminted = await store.createSession(newSession());
    store.apply(unminted.id, { kind: "interrupted", reason: "serverRestart" });
    const path = credentialPath(store.root, unminted.id);
    writeFileSync(`${path}.${unminted.id}.tmp`, "left by a crash", { mode: 0o600 });
    await run(later.credentials.refreshForSession(store.get(unminted.id)!));
    expect(later.auth.get(unminted.id)!.status).toBe("live");
    expect(readdirSync(dirname(path)).filter((f) => f.startsWith("credential.json."))).toEqual([`credential.json.${unminted.id}.tmp`]);
  });

  test("a non-local starter's scopes bound the session's", async () => {
    const core = testCore(store, control, clock);
    const device: Principal = { kind: "device", id: "phone", scopes: ["server.read", "project.read", "project.write"], projects: ["P1"], branches: "*", human: true };
    const s = await store.createSession(newSession());
    await run(core.credentials.mintForSession(s, "br1", device));
    expect([...core.auth.get(s.id)!.scopes].sort()).toEqual(["project.read", "project.write", "server.read"]);
    const t = await store.createSession(newSession());
    await run(core.credentials.mintForSession(t, "br1", PEOPLE));
    expect(core.auth.get(t.id)!.scopes).toEqual([...MODE_SCOPES.draft]);
  });
});

describe("10, 12, 13", () => {
  test("the chain hook refuses sessions what changes state outside their branches", async () => {
    const hooks = agentChainHooks({
      reach: { covers: (p: Principal) => p.kind === "agent" } as never,
      paths: {} as never,
      sessions: store,
    });
    const agent: Principal = { kind: "agent", id: "S", scopes: [], projects: ["P1"], branches: "*", human: false };
    const resolved = {} as never;
    for (const tag of ["project.acknowledgeSalvage", "diagnostics.project", "diagnostics.consent", "diagnostics.purge", "project.open"]) {
      expect(await failure(hooks.authorize!(tag as never, agent, resolved) as Effect.Effect<void, FinError>), tag).toMatchObject({ code: "SCOPE_DENIED", details: { reason: "agentSession" } });
      await run(hooks.authorize!(tag as never, { ...agent, kind: "local", id: "local" }, resolved) as Effect.Effect<void, FinError>);
    }
    await run(hooks.authorize!("diagnostics.status" as never, agent, resolved) as Effect.Effect<void, FinError>);
  });

  test("session_txs outlive their session and go with the project or an explicit purge", async () => {
    const a = await store.createSession(newSession({ projectId: "P1" }));
    const b = await store.createSession(newSession({ projectId: "P2" }));
    store.recordTx({ txId: "T1", sessionId: a.id, callId: null, attribution: "none", at: 1 });
    store.recordTx({ txId: "T2", sessionId: b.id, callId: null, attribution: "none", at: 2 });
    store.recordTx({ txId: "T3", sessionId: b.id, callId: null, attribution: "none", at: 3 });
    await store.delete(a.id);
    store.apply(b.id, { kind: "ended", reason: "userEnded" });
    clock.advance(365 * DAY);
    await store.sweep(clock.now());
    expect(store.get(b.id)).toBeUndefined();
    expect([...store.sessionOfTx(["T1", "T2", "T3"]).keys()].sort()).toEqual(["T1", "T2", "T3"]);
    expect(await store.purgeTxs({ projectId: "P2" })).toBe(2);
    expect(await store.purgeTxs({ sessionId: a.id })).toBe(1);
    expect(store.sessionOfTx(["T1", "T2", "T3"]).size).toBe(0);
  });

  test("a start answers the unsandboxed warning (owner decision F2)", async () => {
    const h = harness(FakeAdapter([idleScript]));
    await h.runtime.boot();
    const started = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    expect(started.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent" }) })]);
    expect(started.session.status).toBe("idle");
  });
});
