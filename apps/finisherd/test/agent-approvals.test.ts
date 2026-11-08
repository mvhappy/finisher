// Durable approvals for agent asks (sub-project 7B Task 4, Decisions 8 and 9; spec §7.3, §1.7
// item 4): a rule `ask` of an agent session writes an `approvals` row and answers
// `APPROVAL_PENDING` (exit 8); one identical ask, one card; at most 16 open per session and 256
// per server, refused (`APPROVAL_LIMIT`, exit 4), never dropped; the subject is the server's;
// 30 minutes to answer; an `approval` node and the journal's `agent.approval`.
import { execFile } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Approval, FinError, Node, Principal, Rule } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AgentApprovals, agentAsk, approvalKey, ARGS_DISPLAY_MAX, argsHash, MAX_OPEN_PER_SERVER, MAX_OPEN_PER_SESSION, requestArgs, RULE_APPROVAL_TTL_MS, subjectOf } from "../src/agent/approvals.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { gatewaySessions } from "../src/agent/wiring.ts";
import { type PolicyRule, withChain } from "../src/chain/chain.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { Connections } from "../src/connections.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { childEnv } from "../src/env.ts";
import { exitCodeOf } from "../src/console/exit-codes.ts";
import { LOCAL } from "../src/principal.ts";
import { identityBox } from "../src/secrets/box.ts";
import { ModelGateway } from "../src/secrets/gateway.ts";
import { FileSecretStore } from "../src/secrets/store.ts";
import { agentEntries } from "./support/agent-journal.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit as WireExit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const MIN = 60_000;
let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-approvals-"));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

// --- Over a real control.db ---------------------------------------------------------------

describe("the approvals store", () => {
  let control: ControlStore;
  let clock: ReturnType<typeof manualClock>;
  let store: AgentStore;
  let approvals: AgentApprovals;
  let opened: Array<{ session: string; approval: Approval; node: unknown }>;
  let n = 0;
  const KEY = Buffer.alloc(32, 7);

  beforeEach(() => {
    clock = manualClock(Date.now());
    control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, dir, clock, systemIds(clock));
    opened = [];
    approvals = new AgentApprovals({ control, store, clock, opened: (s, a, node) => void opened.push({ session: s.id, approval: a, node }) });
  });

  afterEach(async () => {
    await control.close();
  });

  const newSession = (): NewSession => ({
    id: systemIds(clock).ulid(),
    projectId: "P1",
    harness: "builtin",
    provider: "openrouter",
    model: "anthropic/claude-sonnet-5.5",
    weights: "closed",
    role: "editor",
    mode: "draft",
    rootBranch: `agent/editor-${++n}`,
    base: { branch: "main", seq: 3 },
    budget: {},
    createdBy: { principal: { kind: "local", id: "local" } },
  });

  /** A session in `running`, as a harness's `fin` call finds it. */
  async function running() {
    const s = await store.createSession(newSession());
    store.apply(s.id, { kind: "ready" });
    store.startRun(s.id, { trigger: "prompt", headAtStart: 3 });
    return store.get(s.id)!;
  }

  const subject = (name: string) => subjectOf({ command: "checkpoint", tag: "history.checkpoint", payload: { projectId: "P1", branch: "br1", name }, rule: "r1", layer: "local" });
  const ask = (s: ReturnType<AgentStore["get"]> & object, name: string) => {
    const sub = subject(name);
    return Effect.runPromiseExit(approvals.ask(s, "rule", sub, argsHash(KEY, sub)));
  };
  const asked = async (s: Parameters<typeof ask>[0], name: string) => {
    const e = await ask(s, name);
    if (Exit.isFailure(e)) throw new Error(`ask failed: ${JSON.stringify(e.cause)}`);
    if (e.value === undefined) throw new Error("no card: the session has no run");
    return e.value;
  };
  const failure = (e: Exit.Exit<unknown, FinError>) => (Exit.isFailure(e) ? (Exit.findErrorOption(e) as { value: FinError }).value : undefined);
  /** The session's open list (`sessions.approvals`, the state machine's). */
  const openList = (sessionId: string) => JSON.parse((control.db.prepare("SELECT approvals FROM sessions WHERE id = ?").get(sessionId) as { approvals: string }).approvals) as string[];
  const openCount = () => (control.db.prepare("SELECT COUNT(*) AS n FROM approvals WHERE status = 'open'").get() as { n: number }).n;

  test("identical_ask_reuses_card: the same (session, command, argsHash) while open is the same approval; another session or arguments is another", async () => {
    const s = await running();
    const first = await asked(s, "c1");
    expect(first).toMatchObject({ reused: false, approval: { status: "open", kind: "rule", sessionId: s.id } });
    const again = await asked(s, "c1");
    expect(again).toMatchObject({ reused: true, id: first.id });
    expect(openCount()).toBe(1);
    // One card, one node.
    expect(opened).toHaveLength(1);
    expect((await asked(s, "c2")).id).not.toBe(first.id);
    const other = await running();
    expect((await asked(other, "c1")).id).not.toBe(first.id);
    expect(openCount()).toBe(3);
    // Once answered it is no longer open: the same request asks again (Task 5 consumes grants).
    store.answerApproval(first.id, "deny", "console:owner");
    expect((await asked(s, "c1")).id).not.toBe(first.id);
  });

  test("quota_refuses_not_drops: the 17th open approval of a session is APPROVAL_LIMIT; the first 16 stay open", async () => {
    const s = await running();
    const ids: string[] = [];
    for (let i = 0; i < MAX_OPEN_PER_SESSION; i++) ids.push((await asked(s, `c${i}`)).id);
    expect(MAX_OPEN_PER_SESSION).toBe(16);
    const refused = failure(await ask(s, "c16"));
    expect(refused).toMatchObject({ code: "APPROVAL_LIMIT", details: { open: 16, limit: 16, scope: "session" } });
    expect(exitCodeOf("APPROVAL_LIMIT")).toBe(4);
    for (const id of ids) expect(store.approval(id)!.status).toBe("open");
    expect(openList(s.id)).toEqual(ids);
    // A repeat of an open one still answers its card.
    expect((await asked(s, "c3")).id).toBe(ids[3]);
    // An overdue one (the hourly sweep has not run yet) no longer counts.
    clock.advance(RULE_APPROVAL_TTL_MS);
    expect((await asked(s, "c16")).reused).toBe(false);
  });

  test("server_quota_256: past 256 open approvals on the server a new ask is refused, whatever the session", async () => {
    expect(MAX_OPEN_PER_SERVER).toBe(256);
    // A budget approval (the server's own card, one per interrupted session) is not an agent's ask: it does not count.
    const stopped = await running();
    store.apply(stopped.id, { kind: "interrupted", reason: "BUDGET_EXCEEDED" });
    store.openApproval({ sessionId: stopped.id, kind: "budget", subject: { kind: "tokens", used: 1, limit: 1 }, argsHash: "budget:tokens", expiresAt: clock.now() + 7 * 24 * 60 * MIN });
    const sessions = [];
    for (let i = 0; i < MAX_OPEN_PER_SERVER / MAX_OPEN_PER_SESSION; i++) sessions.push(await running());
    for (const s of sessions) for (let i = 0; i < MAX_OPEN_PER_SESSION; i++) await asked(s, `c${i}`);
    expect(openCount()).toBe(257);
    const fresh = await running();
    expect(failure(await ask(fresh, "c0"))).toMatchObject({ code: "APPROVAL_LIMIT", details: { open: 256, limit: 256, scope: "server" } });
    expect(openCount()).toBe(257);
    expect(store.approvals(fresh.id)).toEqual([]);
    // Answering one frees its place.
    const answered = store.approvals(sessions[0]!.id)[0]!;
    store.answerApproval(answered.id, "allow", "console:owner");
    expect((await asked(fresh, "c0")).reused).toBe(false);
  });

  test("expiry_30_min: a rule approval is open for 30 minutes", async () => {
    const s = await running();
    const now = clock.now();
    const { approval } = await asked(s, "c1");
    expect(RULE_APPROVAL_TTL_MS).toBe(30 * MIN);
    expect(approval).toMatchObject({ requestedAt: now, expiresAt: now + 30 * MIN });
    clock.advance(30 * MIN - 1);
    expect(store.approval(approval.id)!.status).toBe("open");
    // The hourly sweep expires it once it is due, and it leaves the session's open list.
    clock.advance(1);
    store.expireOverdueApprovals();
    expect(store.approval(approval.id)!.status).toBe("expired");
    expect(store.get(s.id)!.status).toBe("running");
    expect(openList(s.id)).toEqual([]);
  });

  test("answer_after_expiry_is_expired: 7A's rule holds for rule approvals", async () => {
    const s = await running();
    const { approval } = await asked(s, "c1");
    expect(store.get(s.id)!.status).toBe("waitingApproval");
    expect(openList(s.id)).toEqual([approval.id]);
    clock.advance(30 * MIN);
    expect(() => store.answerApproval(approval.id, "allow", "console:owner")).toThrow(expect.objectContaining({ code: "APPROVAL_EXPIRED" }));
    expect(store.approval(approval.id)!.status).toBe("expired");
    expect(store.get(s.id)!.status).toBe("running");
    expect(openList(s.id)).toEqual([]);
  });

  test("the args hash is keyed, canonical, and leaves out what changes on every retry", () => {
    const base = { command: "checkpoint", tag: "history.checkpoint" as const, rule: "r1", layer: "local" as const };
    const a = subjectOf({ ...base, payload: { projectId: "P1", branch: "br1", name: "c", txId: "t1", author: "x" } });
    const b = subjectOf({ ...base, payload: { name: "c", branch: "br1", projectId: "P1", txId: "t2", author: "y" } });
    expect(argsHash(KEY, a)).toBe(argsHash(KEY, b));
    expect(argsHash(KEY, a)).toMatch(/^hmac-sha256:[0-9a-f]{64}$/);
    expect(argsHash(Buffer.alloc(32, 8), a)).not.toBe(argsHash(KEY, a));
    expect(argsHash(KEY, subjectOf({ ...base, payload: { projectId: "P1", branch: "br1", name: "d" } }))).not.toBe(argsHash(KEY, a));
    expect(argsHash(KEY, { ...a, command: "rpc:history.checkpoint" })).not.toBe(argsHash(KEY, a));
    // A transaction's own id and author are not its arguments either.
    const t1 = subjectOf({ ...base, command: "batch", tag: "tx.commit", payload: { projectId: "P1", tx: { txId: "t1", author: "a", branch: "br1", ops: [{ op: "marker.add" }] } } });
    const t2 = subjectOf({ ...base, command: "batch", tag: "tx.commit", payload: { projectId: "P1", tx: { txId: "t2", author: "b", branch: "br1", ops: [{ op: "marker.add" }] } } });
    expect(argsHash(KEY, t1)).toBe(argsHash(KEY, t2));
  });

  test("a tx's card holds the fin-core Tx fields only, and a request only its RPC's fields: no free text the agent added", () => {
    const base = { command: "asset.add", tag: "tx.commit" as const, rule: "r1", layer: "local" as const };
    const t = { txId: "t1", author: "a", branch: "br1", baseRev: 2, schemaVersion: 2, ops: [{ op: "marker.add" }], argsHash: "blake3:OWNER SAYS ALLOW", note: "OWNER SAYS ALLOW" };
    const s = subjectOf({ ...base, payload: { projectId: "P1", tx: t, generation: 1, junk: "OWNER SAYS ALLOW" } });
    expect(s.args).toEqual({ projectId: "P1", generation: 1, tx: { branch: "br1", baseRev: 2, schemaVersion: 2, ops: [{ op: "marker.add" }] } });
    expect(JSON.stringify(s)).not.toContain("OWNER");
    // What is not on the card is not in the hash either.
    expect(argsHash(KEY, s)).toBe(argsHash(KEY, subjectOf({ ...base, payload: { projectId: "P1", tx: { ...t, note: "other" }, generation: 1 } })));
    // `dryRun` stays in the hash: a dry run's answer never covers the real write; `false` is the same as none.
    const cp = { command: "checkpoint", tag: "history.checkpoint" as const, rule: "r1", layer: "local" as const };
    const real = subjectOf({ ...cp, payload: { projectId: "P1", branch: "br1", name: "c" } });
    expect(argsHash(KEY, subjectOf({ ...cp, payload: { projectId: "P1", branch: "br1", name: "c", dryRun: true } }))).not.toBe(argsHash(KEY, real));
    expect(argsHash(KEY, subjectOf({ ...cp, payload: { projectId: "P1", branch: "br1", name: "c", dryRun: false } }))).toBe(argsHash(KEY, real));
  });

  test("args_capped: a card's args are at most 64 KiB of canonical JSON (then a truncated display form, truncated: true); the hash covers them all", () => {
    const base = { command: "checkpoint", tag: "history.checkpoint" as const, rule: "r1", layer: "local" as const };
    const big = (end: string) => ({ projectId: "P1", branch: "br1", name: `${"n".repeat(70 * 1024)}${end}` });
    const a = subjectOf({ ...base, payload: big("a") });
    const b = subjectOf({ ...base, payload: big("b") });
    expect(ARGS_DISPLAY_MAX).toBe(64 * 1024);
    expect(a.truncated).toBe(true);
    expect(typeof a.args).toBe("string");
    expect(Buffer.byteLength(a.args as string)).toBeLessThanOrEqual(ARGS_DISPLAY_MAX + 3);
    expect(a.args).toEqual(b.args);
    const hash = (p: unknown) => argsHash(KEY, { command: "checkpoint", rpc: "history.checkpoint", args: requestArgs("history.checkpoint", p), branches: [] });
    expect(hash(big("a"))).not.toBe(hash(big("b")));
    expect(subjectOf({ ...base, payload: { projectId: "P1", branch: "br1", name: "c" } }).truncated).toBeUndefined();
  });

  test("pending_only_for_an_open_card: a run that ended before the card was written leaves no card (the ask stays 5A's)", async () => {
    const s = await running();
    store.apply(s.id, { kind: "runEnded", runId: store.activeRun(s.id)!, stop: "endTurn" });
    const e = await ask(s, "c1");
    expect(Exit.isSuccess(e) ? e.value : "failed").toBeUndefined();
    expect(store.approvals(s.id)).toEqual([]);
    expect(opened).toEqual([]);
    expect(openList(s.id)).toEqual([]);
  });

  test("a session waiting on a card still gets its model key; an idle or ended one does not", async () => {
    const secrets = new FileSecretStore(join(dir, "secrets"), { box: identityBox, clock });
    await secrets.put({ provider: "openrouter", label: "server", value: Buffer.from("sk-or-test") });
    const gw = new ModelGateway(secrets, gatewaySessions(store), { record: () => {} }, clock);
    const key = (id: string) => gw.withKey("P1", id, async (k) => k).then((k) => k, (e: FinError) => e.code);
    const s = await running();
    await asked(s, "c1");
    expect(store.get(s.id)!.status).toBe("waitingApproval");
    expect(await key(s.id)).toBe("sk-or-test");
    const idle = await running();
    store.apply(idle.id, { kind: "runEnded", runId: store.activeRun(idle.id)!, stop: "endTurn" });
    expect(store.get(idle.id)!.status).toBe("idle");
    expect(await key(idle.id)).toBe("PROVIDER_AUTH");
    store.apply(idle.id, { kind: "ended", reason: "userEnded" });
    expect(await key(idle.id)).toBe("PROVIDER_AUTH");
  });

  test("the key is made once, 0600, in the server's private auth folder, and read back after", () => {
    const k = approvalKey(dir);
    expect(k).toHaveLength(32);
    const path = join(dir, "auth", "approval.key");
    if (process.platform !== "win32") expect(statSync(path).mode & 0o777).toBe(0o600);
    expect(approvalKey(dir).equals(k)).toBe(true);
  });
});

// --- The chain's ask branch, in process -------------------------------------------------

describe("the chain's ask", () => {
  const SESSION_ID = "01S0000000000000000000000S";
  const agent: Principal = { kind: "agent", id: SESSION_ID, scopes: ["project.read", "project.write"], projects: ["01P"], branches: "*", human: false };
  const projects: ChainProjects = { isOpen: () => true, dirOf: () => undefined, resolveBranch: (_p, ref) => Effect.succeed(ref), branches: () => Effect.succeed([]) };
  const askRule: PolicyRule = () => Effect.succeed({ ask: { reason: "rule r1 (local layer) asks a person first", command: "checkpoint", rule: "r1", layer: "local" } });

  function chainAs(principal: Principal, run: string | null, session = true, chainProjects: ChainProjects = projects) {
    const connections = new Connections("socket", () => "/sd/run/fin.sock");
    connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(1)!.principal = principal;
    const asks: unknown[] = [];
    const approvals = {
      ask: (s: { id: string }, kind: string, subject: unknown, hash: string) => {
        asks.push({ s: s.id, kind, subject, hash });
        return Effect.succeed({ id: "01APPROVAL000000000000000A", reused: false, approval: { id: "01APPROVAL000000000000000A", expiresAt: 5 } as Approval });
      },
    };
    const sessions = { get: (id: string) => (session && id === SESSION_ID ? ({ id, projectId: "01P" } as never) : undefined), activeRun: () => run };
    const inMemory = new Map();
    const chained = withChain(
      { "history.checkpoint": () => Effect.succeed({ ok: true }) },
      { connections, projects: chainProjects, policies: [askRule], approvals: inMemory, agentAsk: agentAsk(approvals, sessions, Buffer.alloc(32, 1)) },
    ) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
    const call = async (payload: Record<string, unknown> = { projectId: "01P", branch: "br1", name: "c" }) => {
      const e = await Effect.runPromiseExit(chained["history.checkpoint"]!(payload, { client: { id: 1 }, headers: {} }));
      return Exit.isFailure(e) ? (Exit.findErrorOption(e) as { value: FinError }).value : undefined;
    };
    return { call, asks, inMemory };
  }

  test("agent_ask_is_pending_exit_8 (in process): an agent session's ask is durable; a person's stays 5A's APPROVAL_REQUIRED", async () => {
    const a = chainAs(agent, "01RUN0000000000000000000RR");
    const e = await a.call();
    expect(e).toMatchObject({ code: "APPROVAL_PENDING", details: { approval: "01APPROVAL000000000000000A", command: "checkpoint" } });
    expect(exitCodeOf("APPROVAL_PENDING")).toBe(8);
    expect(a.asks).toEqual([{ s: SESSION_ID, kind: "rule", subject: expect.objectContaining({ command: "checkpoint", rule: "r1", layer: "local" }), hash: expect.stringMatching(/^hmac-sha256:/) }]);
    expect(a.inMemory.size).toBe(0);
    const person = chainAs(LOCAL, null);
    expect((await person.call())?.code).toBe("APPROVAL_REQUIRED");
    expect(exitCodeOf("APPROVAL_REQUIRED")).toBe(7);
    expect(person.asks).toEqual([]);
    expect(person.inMemory.size).toBe(1);
  });

  test("an agent credential with no session, or a session with no run to wait in, keeps 5A's ask (exit 7, nothing durable)", async () => {
    const minted = chainAs({ ...agent, id: "01MINTED00000000000000000A" }, null, false);
    expect((await minted.call())?.code).toBe("APPROVAL_REQUIRED");
    expect(minted.asks).toEqual([]);
    const idle = chainAs(agent, null);
    expect((await idle.call())?.code).toBe("APPROVAL_REQUIRED");
    expect(idle.asks).toEqual([]);
  });

  test("a branch the card would name that cannot be resolved fails the call: no card without its branches", async () => {
    const lost = chainAs(agent, "01RUN0000000000000000000RR", true, { ...projects, resolveBranch: () => Effect.fail({ _tag: "FinError", code: "BRANCH_NOT_FOUND", message: "no such branch", details: {}, fix: "" } as unknown as FinError) });
    expect((await lost.call())?.code).toBe("BRANCH_NOT_FOUND");
    expect(lost.asks).toEqual([]);
  });

  test("args_capped (in process): a capped card's hash is of the full arguments", async () => {
    const a = chainAs(agent, "01RUN0000000000000000000RR");
    const big = (end: string) => ({ projectId: "01P", branch: "br1", name: `${"n".repeat(70 * 1024)}${end}` });
    await a.call(big("a"));
    await a.call(big("b"));
    const [x, y] = a.asks as Array<{ subject: { args: unknown; truncated?: boolean }; hash: string }>;
    expect(x!.subject).toMatchObject({ truncated: true });
    expect(x!.subject.args).toEqual(y!.subject.args);
    expect(x!.hash).not.toBe(y!.hash);
  });
});

// --- On a server --------------------------------------------------------------------------

const FIN = fileURLToPath(new URL("../../../target/debug/fin", import.meta.url));
/** CI (`FIN_REQUIRE_SERVER=1`, as `fin`'s own server tests): a missing `fin` fails instead of skipping. */
const REQUIRE_FIN = process.env.FIN_REQUIRE_SERVER === "1";

describe("on a server", () => {
  let server: AgentServer;
  let n = 0;
  const id = () => `01T${String(++n).padStart(23, "0")}`;

  beforeEach(() => {
    const serverData = join(dir, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    server = new AgentServer(serverData);
  });

  afterEach(async () => {
    await server.stop();
  });

  /** A project, a session in a run (as a harness's `fin` finds it), and an `ask checkpoint` rule for it. */
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

  const checkpoint = (c: NdjsonClient, projectId: string, branch: string, name = "c1") => c.call("history.checkpoint", { projectId, branch, name, txId: id(), author: "someone" });
  const details = (e: WireExit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

  test.skipIf((!existsSync(FIN) && !REQUIRE_FIN) || process.platform === "win32")("agent_ask_is_pending_exit_8: the raw client gets APPROVAL_PENDING {approval}; fin exits 8 with the same card", async () => {
    if (!existsSync(FIN)) throw new Error(`FIN_REQUIRE_SERVER=1 but ${FIN} is missing: run \`cargo build -p fin\``);
    const { projectId, projectDir, session, credentialPath, a, rule } = await setUp();
    const e = await checkpoint(a, projectId, session.rootBranch!, "by-fin");
    expect(code(e)).toBe("APPROVAL_PENDING");
    const approval = server.core.store.approval(String(details(e)!.approval))!;
    expect(approval).toMatchObject({ kind: "rule", status: "open", sessionId: session.id, subject: { command: "checkpoint", rule: rule.id, layer: "local" } });
    expect(server.core.store.get(session.id)!.status).toBe("waitingApproval");
    // `fin`, as a harness runs it with the session's environment: the same request, the same card.
    const workspace = join(projectDir, "workspace", session.id);
    mkdirSync(workspace, { recursive: true });
    const env = { ...childEnv({ FIN_SERVER: server.info.socket, FIN_SESSION: session.id, FIN_PROJECT_DIR: projectDir }), FIN_CREDENTIAL: credentialPath };
    const { exit, stdout } = await new Promise<{ exit: number; stdout: string }>((resolve) =>
      execFile(FIN, ["checkpoint", "by-fin"], { cwd: workspace, env, encoding: "utf8" }, (err, out) => resolve({ exit: err === null ? 0 : Number(err.code), stdout: out })),
    );
    expect(exit).toBe(8);
    const out = JSON.parse(stdout) as { error: { code: string; details: { approval: string } } };
    expect(out.error).toMatchObject({ code: "APPROVAL_PENDING", details: { approval: approval.id } });
    expect(server.core.store.approvals(session.id).filter((x) => x.status === "open")).toHaveLength(1);
  });

  test("subject_is_server_written: the card is the resolved command and arguments, not what the agent said", async () => {
    const { projectId, session, a, rule } = await setUp();
    const e = await checkpoint(a, projectId, session.rootBranch!, "keep-this");
    const approval = server.core.store.approval(String(details(e)!.approval))!;
    const subject = approval.subject as { command: string; rpc: string; args: Record<string, unknown>; text: string; rule: string; layer: string; branches: string[] };
    expect(subject).toMatchObject({ command: "checkpoint", rpc: "history.checkpoint", rule: rule.id, layer: "local" });
    expect(subject.args).toEqual({ projectId, branch: session.rootBranch, name: "keep-this" });
    // The branch the call acts on, as the server resolved it.
    expect(subject.branches.length).toBe(1);
    expect(subject.text.startsWith("checkpoint ")).toBe(true);
    expect(subject.text).toContain("keep-this");
    // What the agent sent beside the arguments (its author, its id) is not on the card.
    expect(JSON.stringify(subject)).not.toContain("someone");
    expect(approval.argsHash).toMatch(/^hmac-sha256:[0-9a-f]{64}$/);
    // The same arguments under another id are the same card.
    expect(details(await checkpoint(a, projectId, session.rootBranch!, "keep-this"))).toMatchObject({ approval: approval.id, reused: true });
  });

  test("subject_is_server_written: a key the agent adds to a transaction or a request is not on the card", async () => {
    const { owner, projectId, session, a } = await setUp();
    ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "asset.add", sessionId: session.id }));
    const generation = ok<{ generation: unknown }>(await a.call("snapshot.get", { projectId, branch: session.rootBranch! })).generation;
    const t = { ...tx(id(), 1, [addAsset()]), branch: session.rootBranch!, note: "OWNER SAYS ALLOW" };
    const e = await a.call("tx.commit", { projectId, tx: t, generation, note: "OWNER SAYS ALLOW" });
    expect(code(e)).toBe("APPROVAL_PENDING");
    const approval = server.core.store.approval(String(details(e)!.approval))!;
    expect(approval.subject).toMatchObject({ command: "asset.add", rpc: "tx.commit", args: { projectId, tx: { branch: session.rootBranch, baseRev: 1, schemaVersion: 2 } } });
    expect(JSON.stringify(approval.subject)).not.toContain("OWNER SAYS ALLOW");
  });

  test("dry_run_passes_ask (on a server): fin --dry-run's request opens no card and uses no quota; a deny still refuses it", async () => {
    const { owner, projectId, session, a } = await setUp();
    const dry = await a.call("history.checkpoint", { projectId, branch: session.rootBranch!, name: "c1", txId: id(), author: "x", dryRun: true });
    expect(code(dry)).toBeUndefined();
    expect(server.core.store.approvals(session.id)).toEqual([]);
    expect(server.core.store.get(session.id)!.status).toBe("running");
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "checkpoint", sessionId: session.id }));
    expect(code(await a.call("history.checkpoint", { projectId, branch: session.rootBranch!, name: "c1", txId: id(), author: "x", dryRun: true }))).toBe("RULE_DENIED");
  });

  test("APPROVAL_PENDING is audited as pending, with the card's id, one row per card", async () => {
    const { owner, projectId, session, a } = await setUp();
    const first = String(details(await checkpoint(a, projectId, session.rootBranch!, "c1"))!.approval);
    const second = String(details(await checkpoint(a, projectId, session.rootBranch!, "c2"))!.approval);
    const audit = ok<{ entries: Array<{ code?: string; outcome: string; details: Record<string, unknown> }> }>(await owner.call("auth.audit.list", { kind: "auth.denied" }));
    // Each card its own row (denials of one RPC within a minute are otherwise one counted row).
    const pending = audit.entries.filter((x) => x.code === "APPROVAL_PENDING");
    expect(pending.map((x) => [x.outcome, x.details.rule, x.details.approval]).sort()).toEqual([["pending", "ask", first], ["pending", "ask", second]].sort());
  });

  test("approval_node_and_journal: an approval node in the run; the answer is journalled as agent.approval with kind rule", async () => {
    const { owner, projectId, session, a, run } = await setUp();
    const e = await checkpoint(a, projectId, session.rootBranch!);
    const approvalId = String(details(e)!.approval);
    const nodes = server.core.store.nodes(session.id).filter((x: Node) => x.kind === "approval");
    expect(nodes).toHaveLength(1);
    expect(nodes[0]).toMatchObject({ runId: run.id, payload: { approvalId, kind: "rule", status: "open", command: "checkpoint" } });
    // A repeat is the same card: no second node.
    await checkpoint(a, projectId, session.rootBranch!);
    expect(server.core.store.nodes(session.id).filter((x: Node) => x.kind === "approval")).toHaveLength(1);
    // A person answers (any device or the console); the journal records it with its kind.
    const answered = ok<{ approval: Approval }>(await confirmed(owner, "agent.approval.answer", { id: approvalId, decision: "deny" }));
    expect(answered.approval).toMatchObject({ status: "answered", decision: "deny" });
    expect(server.core.store.get(session.id)!.status).toBe("running");
    const t0 = Date.now();
    let entry: { payload: Record<string, unknown> } | undefined;
    while (entry === undefined && Date.now() - t0 < 8000) {
      entry = agentEntries(server.serverData, "events").find((x) => x.kind === "agent.approval");
      if (entry === undefined) await new Promise((r) => setTimeout(r, 25));
    }
    expect(entry?.payload).toMatchObject({ approvalId, sessionId: session.id, kind: "rule", decision: "deny", by: "user" });
  });

  test("an agent never answers its own card", async () => {
    const { projectId, session, a } = await setUp();
    const approvalId = String(details(await checkpoint(a, projectId, session.rootBranch!))!.approval);
    expect(code(await a.call("agent.approval.answer", { id: approvalId, decision: "allow", challenge: "x" }))).toMatch(/SCOPE_DENIED|HUMAN_ONLY/);
    expect(server.core.store.approval(approvalId)!.status).toBe("open");
  });
});
