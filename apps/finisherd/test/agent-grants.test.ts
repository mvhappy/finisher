// One-time grants, waits and answers (sub-project 7B Task 5, Decisions 10–12; spec §7.3): an
// `allow` answer lets exactly one identical request of its session through, consumed in
// `control.db` before the handler and given back when the handler fails without committing; a
// crash between the two loses the grant and never doubles a write; `agent.approval.wait` waits at
// most 90 s (exit 0 allow, 7 deny or expiry, 8 still pending); `remember` adds a visible,
// revocable rule; no agent answers or remembers.
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Approval, FinError, ListedRule, Principal, Rule, Session } from "@finisher/contracts";
import { Effect, Exit, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AgentApprovals, agentAsk, grantPort, idemOf, MAX_WAITS_PER_SESSION, WAIT_POLL_MS } from "../src/agent/approvals.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { remember } from "../src/agent/remember.ts";
import { RuleStore } from "../src/agent/rule-store.ts";
import { commandRules, type RuleSource } from "../src/agent/rules.ts";
import { hashOf, NO_FILE } from "../src/agent/trust.ts";
import { Audit } from "../src/control/audit.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { withChain } from "../src/chain/chain.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { Connections } from "../src/connections.ts";
import { exitCodeOf } from "../src/console/exit-codes.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { finError } from "../src/errors.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit as WireExit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-grants-"));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** A refusal's details. */
const det = (e: FinError | undefined) => (e?.details ?? {}) as Record<string, unknown>;
const details = (e: WireExit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

// --- The chain with a real control.db -----------------------------------------------------

describe("grants in the chain", () => {
  let control: ControlStore;
  let clock: ReturnType<typeof manualClock>;
  let store: AgentStore;
  let approvals: AgentApprovals;
  const KEY = Buffer.alloc(32, 9);
  const projects: ChainProjects = { isOpen: () => true, dirOf: () => undefined, resolveBranch: (_p, ref) => Effect.succeed(ref), branches: () => Effect.succeed([]) };
  const rule: Rule = { id: "r1", effect: "ask", command: "checkpoint", layer: "local", source: "test", createdAt: 0 };
  const rules: RuleSource = { layers: () => ({ rules: [rule], projects: new Map() }) };

  beforeEach(() => {
    clock = manualClock(Date.now());
    control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, dir, clock, systemIds(clock));
    approvals = new AgentApprovals({ control, store, clock, approve: (id, decision, by) => Effect.sync(() => ({ approval: store.answerApproval(id, decision, `${by.kind}:${by.id}`).approval })) });
  });

  afterEach(async () => {
    await control.close();
  });

  const newSession = (): NewSession => ({
    id: systemIds(clock).ulid(),
    projectId: "01P",
    harness: "builtin",
    provider: "openrouter",
    model: "anthropic/claude-sonnet-5.5",
    weights: "closed",
    role: "editor",
    mode: "draft",
    rootBranch: `agent/editor-${systemIds(clock).ulid()}`,
    base: { branch: "main", seq: 3 },
    budget: {},
    createdBy: { principal: { kind: "local", id: "local" } },
  });

  async function running(): Promise<Session> {
    const s = await store.createSession(newSession());
    store.apply(s.id, { kind: "ready" });
    store.startRun(s.id, { trigger: "prompt", headAtStart: 3 });
    return store.get(s.id)!;
  }

  /** `history.checkpoint` behind the chain as session `s`; `handler` is the engine's answer. */
  function chainAs(s: Session, handler: (p: unknown) => Effect.Effect<unknown, FinError>) {
    const connections = new Connections("socket", () => "/sd/run/fin.sock");
    connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
    const principal: Principal = { kind: "agent", id: s.id, scopes: ["project.read", "project.write"], projects: ["01P"], branches: "*", human: false };
    connections.get(1)!.principal = principal;
    const chained = withChain({ "history.checkpoint": handler }, {
      connections,
      projects,
      policies: [commandRules(rules, store, { grants: grantPort(approvals, store, KEY) })],
      agentAsk: agentAsk(approvals, store, KEY),
    }) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
    return async (name: string, txId = systemIds(clock).ulid()) => {
      const e = await Effect.runPromiseExit(chained["history.checkpoint"]!({ projectId: "01P", branch: "br1", name, txId }, { client: { id: 1 }, headers: { "fin-actor-call": "01CALL0000000000000000000C" } }));
      return Exit.isFailure(e) ? (Exit.findErrorOption(e) as { value: FinError }).value : undefined;
    };
  }

  const PERSON: Principal = { kind: "console", id: "owner", scopes: [], projects: "*", branches: "*", human: true };
  const allow = (id: string) => Effect.runPromise(approvals.answer(id, "allow", PERSON));
  const row = (id: string) => control.db.prepare("SELECT consumedAt, grantIdem FROM approvals WHERE id = ?").get(id) as { consumedAt: number | null; grantIdem: string | null };

  test("failed_handler_restores_grant: a typed failure gives the grant back; the retry goes through once", async () => {
    const s = await running();
    let fail: FinError | null = finError("CONFLICT", "the base moved", {});
    let ran = 0;
    const call = chainAs(s, () => {
      ran++;
      return fail === null ? Effect.succeed({ ok: true }) : Effect.fail(fail);
    });
    const pending = await call("c1");
    expect(pending?.code).toBe("APPROVAL_PENDING");
    const id = String(det(pending).approval);
    await allow(id);
    // The grant is consumed before the handler; the handler fails: it is back, unconsumed.
    expect((await call("c1", "01TX00000000000000000000T1"))?.code).toBe("CONFLICT");
    expect(ran).toBe(1);
    expect(row(id)).toEqual({ consumedAt: null, grantIdem: null });
    fail = null;
    expect(await call("c1", "01TX00000000000000000000T2")).toBeUndefined();
    expect(row(id)).toMatchObject({ grantIdem: "01TX00000000000000000000T2" });
    expect(row(id).consumedAt).not.toBeNull();
    // Used once: the same request asks again.
    expect((await call("c1"))?.code).toBe("APPROVAL_PENDING");
    expect(ran).toBe(2);
  });

  test("a failure that may have committed (INTERNAL, a closed engine) keeps the grant consumed", async () => {
    const s = await running();
    let next: FinError = finError("INTERNAL", "panicked", {});
    const call = chainAs(s, () => Effect.fail(next));
    const id = String(det(await call("c1")).approval);
    await allow(id);
    expect((await call("c1"))?.code).toBe("INTERNAL");
    expect(row(id).consumedAt).not.toBeNull();
    expect((await call("c1"))?.code).toBe("APPROVAL_PENDING");
    const id2 = String(det(await call("c1")).approval);
    await allow(id2);
    next = finError("ENGINE_CLOSED", "closed", { reason: "panicked" });
    expect((await call("c1"))?.code).toBe("ENGINE_CLOSED");
    expect(row(id2).consumedAt).not.toBeNull();
  });

  test("the idempotency key recorded is the write's txId, else the call's FIN_CALL", async () => {
    const s = await running();
    const call = chainAs(s, () => Effect.succeed({ ok: true }));
    const id = String(det(await call("c1")).approval);
    await allow(id);
    expect(await call("c1", "01TXIDEM000000000000000001")).toBeUndefined();
    expect(row(id).grantIdem).toBe("01TXIDEM000000000000000001");
    // Directly through the port: no txId, the FIN_CALL header.
    const hash = (store.approval(id)!.argsHash);
    const id2 = String(det(await call("c1")).approval);
    await allow(id2);
    expect(await Effect.runPromise(approvals.consumeGrant(s.id, "checkpoint", hash, "01CALL0000000000000000000C"))).toBe(true);
    expect(row(id2).grantIdem).toBe("01CALL0000000000000000000C");
    await Effect.runPromise(approvals.restoreGrant(s.id, "01CALL0000000000000000000C"));
    expect(row(id2)).toEqual({ consumedAt: null, grantIdem: null });
  });

  test("a grant expires with its approval", async () => {
    const s = await running();
    const call = chainAs(s, () => Effect.succeed({ ok: true }));
    const first = await call("c1");
    const id = String(det(first).approval);
    await allow(id);
    clock.advance(Number(det(first).expiresAt) - clock.now());
    expect((await call("c1"))?.code).toBe("APPROVAL_PENDING");
    expect(row(id).consumedAt).toBeNull();
  });

  test("wait_caps_at_90s (in process): a wait answers pending at its deadline by the injected clock, at most 90 s", async () => {
    const s = await running();
    const call = chainAs(s, () => Effect.succeed({ ok: true }));
    const id = String(det(await call("c1")).approval);
    const t0 = clock.now();
    const waiting = Effect.runPromise(approvals.wait(id, 10 * 60_000));
    // The wait's own clock is the injected one: past 90 s it is pending, though 10 min were asked.
    await new Promise((r) => setTimeout(r, WAIT_POLL_MS / 5));
    clock.advance(90_000);
    expect(await waiting).toBe("pending");
    expect(clock.now() - t0).toBe(90_000);
    // An answer wakes a wait at once.
    const woken = Effect.runPromise(approvals.wait(id, 90_000));
    await allow(id);
    expect(await woken).toBe("allow");
    // A deny, and an expired card.
    const id2 = String(det(await call("c2")).approval);
    await Effect.runPromise(approvals.answer(id2, "deny", PERSON));
    expect(await Effect.runPromise(approvals.wait(id2, 1000))).toBe("deny");
    const third = await call("c3");
    clock.advance(Number(det(third).expiresAt) - clock.now());
    expect(await Effect.runPromise(approvals.wait(String(det(third).approval), 1000))).toBe("expired");
  });

  test("idemOf prefers the payload's idemKey (jobs.submit, --idem-key) over txId and FIN_CALL", () => {
    const principal = { kind: "agent", id: "S", scopes: [], projects: "*", branches: "*", human: false } as Principal;
    const headers = { "fin-actor-call": "01CALL0000000000000000000C" };
    expect(idemOf({ payload: { idemKey: "k1", txId: "t1" }, principal, headers })).toBe("k1");
    expect(idemOf({ payload: { idemKey: "k1" }, principal, headers })).toBe("k1");
    expect(idemOf({ payload: { txId: "t1" }, principal, headers })).toBe("t1");
    expect(idemOf({ payload: {}, principal, headers })).toBe("01CALL0000000000000000000C");
  });

  test("an interrupted wait stops polling; at most MAX_WAITS_PER_SESSION waits of one session at a time", async () => {
    const s = await running();
    const call = chainAs(s, () => Effect.succeed({ ok: true }));
    const id = String(det(await call("c1")).approval);
    let reads = 0;
    const counted = new Proxy(store, { get: (t, k) => (k === "approval" ? (x: string) => (reads++, t.approval(x)) : Reflect.get(t, k)) });
    const watched = new AgentApprovals({ control, store: counted, clock });
    // Interruption (a client that went away: the RPC fiber is interrupted) ends the wait at once.
    const fiber = Effect.runFork(watched.wait(id, 90_000));
    await new Promise((r) => setTimeout(r, WAIT_POLL_MS / 5));
    expect(watched.waiting).toBe(1);
    await Effect.runPromise(Fiber.interrupt(fiber));
    await new Promise((r) => setTimeout(r, 0));
    expect(watched.waiting).toBe(0);
    const after = reads;
    await new Promise((r) => setTimeout(r, WAIT_POLL_MS * 2));
    expect(reads).toBe(after);
    // The cap: past MAX_WAITS_PER_SESSION concurrent waits, APPROVAL_LIMIT; the others keep waiting.
    const fibers = Array.from({ length: MAX_WAITS_PER_SESSION }, () => Effect.runFork(watched.wait(id, 90_000)));
    await new Promise((r) => setTimeout(r, 0));
    const over = await Effect.runPromiseExit(watched.wait(id, 90_000));
    expect(Exit.isFailure(over) && (Exit.findErrorOption(over) as { value: FinError }).value).toMatchObject({ code: "APPROVAL_LIMIT", details: { waits: MAX_WAITS_PER_SESSION, limit: MAX_WAITS_PER_SESSION, scope: "session" } });
    expect(watched.waiting).toBe(MAX_WAITS_PER_SESSION);
    for (const f of fibers) await Effect.runPromise(Fiber.interrupt(f));
    expect(watched.waiting).toBe(0);
  });
});

// --- remember: project, in process ---------------------------------------------------------

describe("remember into finisher.rules.json", () => {
  let control: ControlStore;
  let ruleStore: RuleStore;
  let projectDir: string;
  const clock = manualClock(1_794_571_200_000);
  const PERSON: Principal = { kind: "console", id: "owner", scopes: [], projects: "*", branches: "*", human: true };
  const session = { id: "S1", projectId: "P1" } as Session;

  beforeEach(() => {
    control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    ruleStore = new RuleStore(control.db);
    projectDir = join(dir, "P");
    mkdirSync(projectDir);
  });

  afterEach(async () => {
    await control.close();
  });

  const deps = () => ({ ruleStore, sessions: { get: () => session }, dirOf: () => projectDir, audit: new Audit(control, () => clock.now()), clock });
  const card = (id: string, command: string) => ({ id, sessionId: "S1", kind: "rule", status: "open", subject: { command }, argsHash: "h", requestedAt: 0, expiresAt: 1 }) as unknown as Approval;
  const file = () => join(projectDir, "finisher.rules.json");
  const ids = () => (JSON.parse(readFileSync(file(), "utf8")) as { rules: Array<{ id: string }> }).rules.map((r) => r.id);

  test("a failed answer's undo takes out only its own rule: A writes, B writes, A's approve fails, B's rule stays (trust follows)", () => {
    writeFileSync(file(), `${JSON.stringify({ rules: [{ id: "base", effect: "deny", command: "render" }] })}\n`);
    ruleStore.trust("P1", hashOf(readFileSync(file())), "console:owner", 5);
    const a = remember(deps(), card("A", "checkpoint"), "deny", "project", PERSON);
    const b = remember(deps(), card("B", "export"), "deny", "project", PERSON);
    expect(ids()).toEqual(["base", a.rule.id, b.rule.id]);
    // B refreshed the trust to the file holding both: it still covers what remains without A.
    expect(ruleStore.trustOf("P1")!.rulesHash).toBe(hashOf(readFileSync(file())));
    a.undo();
    expect(ids()).toEqual(["base", b.rule.id]);
    expect(ruleStore.trustOf("P1")).toMatchObject({ rulesHash: hashOf(readFileSync(file())), by: "console:owner" });
    expect(readdirSync(projectDir).filter((x) => x.endsWith(".tmp"))).toEqual([]);
  });

  test("an undo with nothing in between restores the file and the trust exactly; edited by hand since, the trust goes back to what it covered", () => {
    const original = `${JSON.stringify({ rules: [], note: "kept" })}\n`;
    writeFileSync(file(), original);
    ruleStore.trust("P1", hashOf(Buffer.from(original)), "console:owner", 5);
    remember(deps(), card("A", "checkpoint"), "deny", "project", PERSON).undo();
    expect(readFileSync(file(), "utf8")).toBe(original);
    expect(ruleStore.trustOf("P1")).toMatchObject({ rulesHash: hashOf(Buffer.from(original)), trustedAt: 5, by: "console:owner" });
    // Changed by hand after the answer wrote: the rule leaves, the hand edit stays, and the trust is
    // what it was before (it never covered the hand edit).
    const c = remember(deps(), card("C", "checkpoint"), "deny", "project", PERSON);
    const edited = JSON.parse(readFileSync(file(), "utf8")) as { rules: unknown[] };
    writeFileSync(file(), JSON.stringify({ ...edited, rules: [...edited.rules, { id: "hand", effect: "allow", command: "render" }] }));
    c.undo();
    expect(ids()).toEqual(["hand"]);
    expect(ruleStore.trustOf("P1")!.rulesHash).toBe(hashOf(Buffer.from(original)));
    // No file before, none after.
    rmSync(file());
    ruleStore.trust("P1", NO_FILE, "console:owner", 6);
    remember(deps(), card("D", "checkpoint"), "deny", "project", PERSON).undo();
    expect(existsSync(file())).toBe(false);
    expect(ruleStore.trustOf("P1")!.rulesHash).toBe(NO_FILE);
  });

  test("a symlinked or directory finisher.rules.json (any non-regular file) is refused, untouched; a replace keeps the file's mode", () => {
    const target = join(dir, "elsewhere.json");
    writeFileSync(target, JSON.stringify({ rules: [] }));
    symlinkSync(target, file());
    expect(() => remember(deps(), card("A", "checkpoint"), "deny", "project", PERSON)).toThrow(expect.objectContaining({ code: "INVALID_REQUEST", details: expect.objectContaining({ reason: "notRegular" }) }));
    expect(readFileSync(target, "utf8")).toBe(JSON.stringify({ rules: [] }));
    rmSync(file());
    mkdirSync(file());
    expect(() => remember(deps(), card("A", "checkpoint"), "deny", "project", PERSON)).toThrow(expect.objectContaining({ details: expect.objectContaining({ reason: "notRegular" }) }));
    rmSync(file(), { recursive: true });
    writeFileSync(file(), JSON.stringify({ rules: [] }));
    chmodSync(file(), 0o600);
    remember(deps(), card("A", "checkpoint"), "deny", "project", PERSON);
    expect(statSync(file()).mode & 0o777).toBe(0o600);
  });
});

// --- On a server --------------------------------------------------------------------------

describe("on a server", () => {
  let server: AgentServer;
  let n = 0;
  const id = () => `01T${String(++n).padStart(23, "0")}`;
  let serverData: string;
  /** The crash point of `grant_survives_no_crash_double`. */
  let crash: { at: (() => void) | null; hang: boolean } = { at: null, hang: false };

  beforeEach(() => {
    serverData = join(dir, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    crash = { at: null, hang: false };
    server = new AgentServer(serverData, {
      afterGrantConsumed: () => {
        crash.at?.();
        return crash.hang ? new Promise<void>(() => {}) : Promise.resolve();
      },
    });
  });

  afterEach(async () => {
    await server.stop();
  });

  /** A project, a session in a run, and an `ask checkpoint` rule for it. */
  async function setUp() {
    await server.start();
    const local = await server.local();
    const projectDir = join(dir, "P");
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
    const opened = await open(server.core, local, projectId);
    const owner = await server.console();
    const { rule } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "checkpoint", sessionId: opened.session.id }));
    const { run } = server.core.store.startRun(opened.session.id, { trigger: "prompt" });
    const a = await server.agent(opened.credentialPath);
    return { local, owner, projectId, projectDir, session: opened.session, credentialPath: opened.credentialPath, a, rule, run };
  }

  const checkpoint = (c: NdjsonClient, projectId: string, branch: string, name = "c1", txId = id()) => c.call("history.checkpoint", { projectId, branch, name, txId, author: "x" });
  const answer = (owner: NdjsonClient, approvalId: string, decision: "allow" | "deny", remember?: string) =>
    confirmed(owner, "agent.approval.answer", { id: approvalId, decision, ...(remember === undefined ? {} : { remember }) });
  const head = async (c: NdjsonClient, projectId: string, branch: string) =>
    ok<{ branches: Array<{ id: string; head: number }> }>(await c.call("branch.list", { projectId, all: true })).branches.find((b) => b.id === branch)!.head;

  test("grant_is_once_and_exact: one identical request goes through; a second, other arguments or another session ask again", async () => {
    const { local, owner, projectId, session, a } = await setUp();
    const root = session.rootBranch!;
    const first = await checkpoint(a, projectId, root, "c1");
    const approvalId = String(details(first)!.approval);
    ok(await answer(owner, approvalId, "allow"));
    // Other arguments are another card, and do not use the grant.
    expect(code(await checkpoint(a, projectId, root, "c2"))).toBe("APPROVAL_PENDING");
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeUndefined();
    // Another session never inherits it (its own rule, the same command and arguments).
    const other = await open(server.core, local, projectId);
    ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "checkpoint", sessionId: other.session.id }));
    server.core.store.startRun(other.session.id, { trigger: "prompt" });
    const b = await server.agent(other.credentialPath);
    expect(code(await checkpoint(b, projectId, other.session.rootBranch!, "c1"))).toBe("APPROVAL_PENDING");
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeUndefined();
    // The identical request goes through once, under a new id: the grant is the card's arguments.
    const before = await head(local, projectId, root);
    const txId = id();
    const done = await checkpoint(a, projectId, root, "c1", txId);
    expect(code(done)).toBeUndefined();
    expect(await head(local, projectId, root)).toBe(before + 1);
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeTypeOf("number");
    // The second identical request asks again, with a new card.
    const again = await checkpoint(a, projectId, root, "c1");
    expect(code(again)).toBe("APPROVAL_PENDING");
    expect(details(again)!.approval).not.toBe(approvalId);
    expect(await head(local, projectId, root)).toBe(before + 1);
  });

  test("grant_survives_no_crash_double: a crash between consume and commit loses the grant; the retry asks again, one commit in all", async () => {
    const { owner, projectId, session, a, credentialPath } = await setUp();
    const root = session.rootBranch!;
    const approvalId = String(details(await checkpoint(a, projectId, root, "c1"))!.approval);
    ok(await answer(owner, approvalId, "allow"));
    const local = await server.local();
    const before = await head(local, projectId, root);
    // The server dies after the grant was consumed, before the handler ran.
    let consumed!: () => void;
    const atCrash = new Promise<void>((r) => (consumed = r));
    crash = { at: () => consumed(), hang: true };
    const crashedTx = id();
    void checkpoint(a, projectId, root, "c1", crashedTx).catch(() => undefined);
    await atCrash;
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeTypeOf("number");
    await server.stop();
    crash = { at: null, hang: false };
    // Restarted on the same data: the write is absent; the grant stays used.
    server = new AgentServer(serverData);
    await server.start();
    const local2 = await server.local();
    expect(await head(local2, projectId, root)).toBe(before);
    expect(ok(await local2.call("tx.get", { projectId, txId: crashedTx }))).toBeNull();
    expect(server.core.store.approval(approvalId)).toMatchObject({ status: "answered", decision: "allow" });
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeTypeOf("number");
    // The session (interrupted by the restart) runs again; the agent's retry asks again.
    server.core.store.apply(session.id, { kind: "resumed" });
    server.core.store.startRun(session.id, { trigger: "prompt" });
    const a2 = await server.agent(credentialPath);
    const retry = await checkpoint(a2, projectId, root, "c1", crashedTx);
    expect(code(retry)).toBe("APPROVAL_PENDING");
    const second = String(details(retry)!.approval);
    expect(second).not.toBe(approvalId);
    const owner2 = await server.console();
    ok(await answer(owner2, second, "allow"));
    expect(code(await checkpoint(a2, projectId, root, "c1", crashedTx))).toBeUndefined();
    expect(await head(local2, projectId, root)).toBe(before + 1);
    void owner;
  });

  test("wait_timeout_is_pending_exit_8: the RPC answers allow, APPROVAL_DENIED on deny or expiry, APPROVAL_PENDING on timeout (repeatable)", async () => {
    const { owner, projectId, session, a } = await setUp();
    const root = session.rootBranch!;
    const id1 = String(details(await checkpoint(a, projectId, root, "c1"))!.approval);
    for (let i = 0; i < 2; i++) {
      const timedOut = await a.call("agent.approval.wait", { id: id1, timeoutMs: 20 });
      expect(code(timedOut)).toBe("APPROVAL_PENDING");
      expect(details(timedOut)).toMatchObject({ approval: id1, command: "checkpoint" });
    }
    expect(exitCodeOf("APPROVAL_PENDING")).toBe(8);
    // An answer while waiting ends the wait at once.
    const waiting = a.call("agent.approval.wait", { id: id1, timeoutMs: 90_000 });
    await new Promise((r) => setTimeout(r, 50));
    ok(await answer(owner, id1, "allow"));
    expect(ok<{ approval: Approval }>(await waiting).approval).toMatchObject({ id: id1, status: "answered", decision: "allow" });
    // Deny.
    const id2 = String(details(await checkpoint(a, projectId, root, "c2"))!.approval);
    ok(await answer(owner, id2, "deny"));
    const denied = await a.call("agent.approval.wait", { id: id2, timeoutMs: 0 });
    expect(code(denied)).toBe("APPROVAL_DENIED");
    expect(details(denied)).toMatchObject({ approval: id2, decision: "deny" });
    expect(exitCodeOf("APPROVAL_DENIED")).toBe(7);
    // Expiry, by the server's (injected) clock.
    const id3 = String(details(await checkpoint(a, projectId, root, "c3"))!.approval);
    server.time.advance(31 * 60_000);
    expect(details(await a.call("agent.approval.wait", { id: id3, timeoutMs: 0 }))).toMatchObject({ approval: id3, decision: "expired" });
    // Past 90 s is refused by the schema.
    expect(code(await a.call("agent.approval.wait", { id: id3, timeoutMs: 90_001 }))).toBe("INVALID_REQUEST");
  });

  test("a session waits on and lists its own cards only; other agent reads stay people's", async () => {
    const { local, owner, projectId, session, a } = await setUp();
    const mine = String(details(await checkpoint(a, projectId, session.rootBranch!, "c1"))!.approval);
    const other = await open(server.core, local, projectId);
    ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "checkpoint", sessionId: other.session.id }));
    server.core.store.startRun(other.session.id, { trigger: "prompt" });
    const b = await server.agent(other.credentialPath);
    const theirs = String(details(await checkpoint(b, projectId, other.session.rootBranch!, "c1"))!.approval);
    expect(code(await a.call("agent.approval.wait", { id: theirs, timeoutMs: 0 }))).toBe("APPROVAL_NOT_FOUND");
    const listed = ok<{ approvals: Approval[] }>(await a.call("agent.approval.list", { sessionId: other.session.id }));
    expect(listed.approvals.map((x) => x.id)).toEqual([mine]);
    expect(ok<{ approvals: Approval[] }>(await a.call("agent.approval.list", {})).approvals.map((x) => x.id)).toEqual([mine]);
    for (const [tag, payload] of [["agent.session.get", { sessionId: other.session.id }], ["agent.rules.list", {}]] as const) {
      expect(code(await a.call(tag, payload)), tag).toBe("SCOPE_DENIED");
    }
    // A session lists its own children only (7B Task 6): none here.
    expect(ok<{ sessions: unknown[] }>(await a.call("agent.session.list", {})).sessions).toEqual([]);
    // A person waits on any card in reach.
    expect(code(await local.call("agent.approval.wait", { id: theirs, timeoutMs: 0 }))).toBe("APPROVAL_PENDING");
  });

  test("agent_cannot_answer_or_remember: every agent answer is refused, with or without remember", async () => {
    const { projectId, session, a, credentialPath } = await setUp();
    const approvalId = String(details(await checkpoint(a, projectId, session.rootBranch!))!.approval);
    for (const remember of [undefined, "session", "project", "projectLocal"]) {
      const e = await a.call("agent.approval.answer", { id: approvalId, decision: "allow", challenge: "x", ...(remember === undefined ? {} : { remember }) });
      expect(code(e), String(remember)).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    }
    // Another agent credential (a second proof of the same file) is refused the same.
    const again = await server.agent(credentialPath);
    expect(code(await again.call("agent.approval.answer", { id: approvalId, decision: "deny", challenge: "x" }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    expect(server.core.store.approval(approvalId)!.status).toBe("open");
    expect(code(await a.call("agent.rules.add", { effect: "allow", command: "checkpoint", challenge: "x" }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    expect(server.agents.ruleStore.all().filter((r) => r.effect === "allow")).toEqual([]);
  });

  test("remember on a budget approval is INVALID_REQUEST", async () => {
    const { owner, session } = await setUp();
    const { approval } = server.core.store.openApproval({ sessionId: session.id, kind: "budget", subject: { kind: "tokens", used: 1, limit: 1 }, argsHash: "budget:tokens", expiresAt: server.time.now() + 60_000 });
    const e = await answer(owner, approval.id, "allow", "session");
    expect(code(e)).toBe("INVALID_REQUEST");
    expect(details(e)).toMatchObject({ field: "remember" });
    expect(server.core.store.approval(approval.id)!.status).toBe("open");
  });

  describe("remember_adds_a_rule", () => {
    const listRules = async (owner: NdjsonClient, q: Record<string, unknown>) => ok<{ rules: ListedRule[]; projectRules?: string; rulesHash?: string }>(await owner.call("agent.rules.list", q));
    const audits = async (owner: NdjsonClient) =>
      ok<{ entries: Array<{ kind: string; details: Record<string, unknown> }> }>(await owner.call("auth.audit.list", { kind: "agent.rule.add" })).entries;

    test("session: a local rule for the session; projectLocal: for the project; listed with source, removable, audited", async () => {
      const { owner, projectId, session, a } = await setUp();
      const root = session.rootBranch!;
      const id1 = String(details(await checkpoint(a, projectId, root, "c1"))!.approval);
      const r1 = ok<{ approval: Approval; rule: ListedRule; notice?: string }>(await answer(owner, id1, "deny", "session"));
      expect(r1.rule).toMatchObject({ effect: "deny", command: "checkpoint", layer: "local", source: `approval:${id1}`, sessionId: session.id, projectId });
      expect(r1.approval).toMatchObject({ status: "answered", decision: "deny" });
      // The remembered deny applies: the same command is now refused, with any arguments.
      expect(code(await checkpoint(a, projectId, root, "other"))).toBe("RULE_DENIED");
      const listed = await listRules(owner, { sessionId: session.id });
      expect(listed.rules.find((r) => r.id === r1.rule.id)).toMatchObject({ layer: "local", source: `approval:${id1}`, applies: true });
      ok(await confirmed(owner, "agent.rules.remove", { id: r1.rule.id }));
      expect(code(await checkpoint(a, projectId, root, "other"))).toBe("APPROVAL_PENDING");
      // projectLocal: the project's local rule (no session), never in the project folder.
      const id2 = String(details(await checkpoint(a, projectId, root, "c2"))!.approval);
      const r2 = ok<{ rule: ListedRule; notice?: string }>(await answer(owner, id2, "allow", "projectLocal"));
      expect(r2.rule).toMatchObject({ effect: "allow", command: "checkpoint", layer: "local", projectId });
      expect(r2.rule.sessionId).toBeUndefined();
      // A remembered allow lifts the ask that asked (owner decision 2026-11-08): no notice.
      expect(r2.notice).toBeUndefined();
      const entries = await audits(owner);
      expect(entries.filter((e) => e.details.from !== undefined).map((e) => [e.details.from, e.details.layer]).sort()).toEqual([[id1, "local"], [id2, "local"]].sort());
      // The same request, and any other of the command, goes through (the lift, not the grant).
      expect(code(await checkpoint(a, projectId, root, "c2"))).toBeUndefined();
      expect(code(await checkpoint(a, projectId, root, "c3"))).toBeUndefined();
      expect(server.core.store.approval(id2)!.consumedAt).toBeUndefined();
    });

    test("project: the shared finisher.rules.json, atomically; trust refreshed only if trusted with the previous hash", async () => {
      const { owner, projectId, projectDir, session, a } = await setUp();
      const root = session.rootBranch!;
      const file = join(projectDir, "finisher.rules.json");
      // Untrusted: the rule is written, does not apply, and the answer's grant still does.
      const id1 = String(details(await checkpoint(a, projectId, root, "c1"))!.approval);
      const r1 = ok<{ rule: ListedRule; notice?: string }>(await answer(owner, id1, "deny", "project"));
      expect(r1.rule).toMatchObject({ effect: "deny", command: "checkpoint", layer: "project", applies: false });
      expect(r1.notice).toContain("does not apply until a person trusts");
      expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ rules: [{ id: r1.rule.id, effect: "deny", command: "checkpoint" }] });
      expect(existsSync(file) && readdirTmp(projectDir)).toEqual([]);
      expect(code(await checkpoint(a, projectId, root, "other"))).toBe("APPROVAL_PENDING");
      expect((await listRules(owner, { projectId })).projectRules).toBe("untrusted");
      // Trusted with the file's hash: the next remember keeps it trusted (the new hash).
      const { rulesHash } = await listRules(owner, { projectId });
      ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rulesHash }));
      expect(code(await checkpoint(a, projectId, root, "other"))).toBe("RULE_DENIED");
      writeFileSync(file, JSON.stringify({ rules: [], note: "kept" }));
      const now = await listRules(owner, { projectId });
      ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: now.rulesHash }));
      const id2 = String(details(await checkpoint(a, projectId, root, "c2"))!.approval);
      const r2 = ok<{ rule: ListedRule; notice?: string }>(await answer(owner, id2, "deny", "project"));
      expect(r2.rule).toMatchObject({ layer: "project", applies: true });
      expect(r2.notice).toBeUndefined();
      expect(JSON.parse(readFileSync(file, "utf8"))).toMatchObject({ note: "kept", rules: [{ id: r2.rule.id, effect: "deny", command: "checkpoint" }] });
      const after = await listRules(owner, { projectId });
      expect(after.projectRules).toBe("trusted");
      expect(after.rules.find((r) => r.id === r2.rule.id)).toMatchObject({ layer: "project", source: "finisher.rules.json", applies: true });
      expect(code(await checkpoint(a, projectId, root, "other"))).toBe("RULE_DENIED");
      // Changed since it was trusted (by hand): a remember writes, does not re-trust.
      writeFileSync(file, JSON.stringify({ rules: [] }));
      const id3 = String(details(await checkpoint(a, projectId, root, "c3"))!.approval);
      const r3 = ok<{ rule: ListedRule; notice?: string }>(await answer(owner, id3, "allow", "project"));
      expect(r3.rule.applies).toBe(false);
      expect((await listRules(owner, { projectId })).projectRules).toBe("changed");
      const entries = await audits(owner);
      expect(entries.filter((e) => e.details.layer === "project").map((e) => e.details.from).sort()).toEqual([id1, id2, id3].sort());
    });

    test("a project file that is not a rules file is not touched; the card stays open", async () => {
      const { owner, projectId, projectDir, session, a } = await setUp();
      const file = join(projectDir, "finisher.rules.json");
      writeFileSync(file, "not json");
      const id1 = String(details(await checkpoint(a, projectId, session.rootBranch!, "c1"))!.approval);
      expect(code(await answer(owner, id1, "deny", "project"))).toBe("INVALID_REQUEST");
      expect(readFileSync(file, "utf8")).toBe("not json");
      expect(server.core.store.approval(id1)!.status).toBe("open");
    });
  });
});

/** Temporary files left beside the rules file (an atomic replace leaves none). */
function readdirTmp(d: string): string[] {
  return readdirSync(d).filter((x) => x.endsWith(".tmp"));
}
