// 5A Task 9: agent and MCP credentials — minted by the console, proved through the handshake,
// limited to their scopes, projects and branches, expired when idle or old, revoked through the
// one revoke path (connections, streams and writes in flight end), kept across restarts, never
// stored with their key. Units on `Credentials` with an injected clock, then a server run in
// this process (so its clock can be moved) that a console and agents talk to over the socket.
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type Credential, type CredentialFile, HUMAN_ONLY, MINT_DEFAULT_SCOPES, type Principal } from "@finisher/contracts";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { b64u, canonicalEndpoint, credentialKey, helloProof, proveMac, sameMac } from "../src/auth/crypto.ts";
import { Credentials, TOUCH_WRITE_MS } from "../src/auth/credentials.ts";
import { authDir, readKeyFile } from "../src/auth/keys.ts";
import { revokeCredential, sweepExpired } from "../src/auth/revoke.ts";
import { Tickets } from "../src/auth/tickets.ts";
import { APPROVAL_TTL_MS, keepApproval, MAX_APPROVALS, MAX_APPROVALS_PER_PRINCIPAL } from "../src/chain/chain.ts";
import { ConnectionRegistry } from "../src/connections.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, controlDbPath, openControlStore } from "../src/control/db.ts";
import { sourceOf } from "../src/diagnostics.ts";
import { childEnv } from "../src/env.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { addAsset, commit, confirmed, connectProved, type Exit, generationOf, hello, insert, NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";

const DAY = 86_400_000;
let tmp: string;
let serverData: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-credentials-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  await stopServer();
  rmSync(tmp, { recursive: true, force: true });
});

function clock(start = 1_800_000_000_000) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

const code = (e: Exit) => (e.ok ? undefined : e.error.code);

describe("Credentials (units)", () => {
  let store: ControlStore;
  let audit: Audit;
  const master = Buffer.alloc(32, 7);
  const open = (c = clock()) => {
    store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    audit = new Audit(store, c.now);
    return new Credentials(store, master, audit, c.now);
  };
  afterEach(async () => {
    await store?.close();
  });

  test("mint_defaults_and_branches_kept", () => {
    const creds = open();
    const { credential, file } = creds.mint(CONSOLE, { kind: "agent", label: "  night run  " });
    expect(credential).toMatchObject({ kind: "agent", label: "night run", scopes: MINT_DEFAULT_SCOPES, projects: "*", branches: "*", status: "live", createdBy: "console:console" });
    expect(file).toEqual({ v: 1, kind: "agent", id: credential.id, key: file.key, env: store.envId });
    expect(b64u.decode(file.key)).toEqual(credentialKey(master, store.envId, "agent", credential.id));
    const own = creds.mint(CONSOLE, { kind: "mcp", label: "adapter", scopes: ["project.read"], projects: ["01p"], branches: ["own", "br2", "own"] }).credential;
    expect(own).toMatchObject({ scopes: ["project.read"], projects: ["01p"], branches: ["own", "br2"] });
    const p = creds.lookup({ kind: "mcp", id: own.id })!.principal;
    expect(p).toEqual({ kind: "mcp", id: own.id, scopes: ["project.read"], projects: ["01p"], branches: ["own", "br2"], human: false });
    expect(sourceOf(p)).toBe("mcp");
    expect(sourceOf(creds.lookup({ kind: "agent", id: credential.id })!.principal)).toBe("fin");
    // A kind that does not match the id's is unknown.
    expect(creds.lookup({ kind: "agent", id: own.id })).toBeUndefined();
  });

  test("mint_refuses_human_only_scopes, mint_needs_a_human, bad requests", () => {
    const creds = open();
    for (const s of HUMAN_ONLY) {
      expect(() => creds.mint(CONSOLE, { kind: "agent", label: "x", scopes: ["project.read", s] })).toThrow(
        expect.objectContaining({ code: "SCOPE_DENIED", details: expect.objectContaining({ scopes: [s] }) }),
      );
    }
    for (const by of [LOCAL, { ...LOCAL, kind: "agent" as const, id: "01a" }]) {
      expect(() => creds.mint(by, { kind: "agent", label: "x" })).toThrow(expect.objectContaining({ code: "HUMAN_ONLY" }));
    }
    for (const req of [
      { label: "" },
      { label: "x".repeat(65) },
      { label: "a\u0007b" },
      { label: "x", projects: [] },
      { label: "x", projects: ["a b"] },
      { label: "x", branches: ["has space"] },
      { label: "x", idleS: 10 },
      { label: "x", absoluteS: 1.5 },
    ]) {
      expect(() => creds.mint(CONSOLE, { kind: "agent", ...req }), JSON.stringify(req)).toThrow(expect.objectContaining({ code: "INVALID_REQUEST" }));
    }
    expect(creds.list(true)).toEqual([]);
  });

  test("key_is_never_stored", async () => {
    const creds = open();
    const { file } = creds.mint(CONSOLE, { kind: "agent", label: "x" });
    await store.close();
    const key = b64u.decode(file.key)!;
    for (const name of readdirSync(serverData).filter((n) => n.startsWith("control.db"))) {
      const bytes = readFileSync(join(serverData, name));
      expect(bytes.includes(key), name).toBe(false);
      expect(bytes.includes(Buffer.from(file.key)), name).toBe(false);
    }
    store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    const columns = (store.db.prepare("PRAGMA table_info(credentials)").all() as Array<{ name: string }>).map((c) => c.name);
    expect(columns.some((c) => /key|secret/i.test(c))).toBe(false);
  });

  test("idle_and_absolute_expiry (agent 24 h idle, 7 d absolute; mcp 30 d idle, none)", () => {
    const c = clock();
    const creds = open(c);
    const agent = creds.mint(CONSOLE, { kind: "agent", label: "a" }).credential;
    const used = creds.mint(CONSOLE, { kind: "agent", label: "b" }).credential;
    const mcp = creds.mint(CONSOLE, { kind: "mcp", label: "m" }).credential;
    expect(agent).toMatchObject({ idleExpiryS: 86_400, absoluteExpiryAt: c.now() + 7 * DAY });
    expect(mcp.idleExpiryS).toBe(30 * 86_400);
    expect(mcp.absoluteExpiryAt).toBeUndefined();
    c.advance(DAY - 1000);
    creds.touch(used.id);
    expect(creds.isLive({ kind: "agent", id: agent.id })).toBe(true);
    c.advance(1000);
    // 24 h without a use.
    expect(creds.isLive({ kind: "agent", id: agent.id })).toBe(false);
    expect(creds.keyFor("agent", agent.id)).toBeUndefined();
    expect(creds.isLive({ kind: "agent", id: used.id })).toBe(true);
    // Used every 23 h, it still ends at 7 days.
    for (let t = DAY; t < 7 * DAY; t += 23 * 3_600_000) {
      c.advance(23 * 3_600_000);
      creds.touch(used.id);
    }
    expect(creds.isLive({ kind: "agent", id: used.id })).toBe(false);
    expect(creds.expired().map((r) => r.id).sort()).toEqual([agent.id, used.id].sort());
    // 30 days unused ends an mcp credential.
    expect(creds.isLive({ kind: "mcp", id: mcp.id })).toBe(true);
    c.advance(30 * DAY);
    expect(creds.isLive({ kind: "mcp", id: mcp.id })).toBe(false);
    // A used mcp credential has no absolute limit.
    const long = creds.mint(CONSOLE, { kind: "mcp", label: "long" }).credential;
    for (let i = 0; i < 20; i++) {
      c.advance(29 * DAY);
      creds.touch(long.id);
    }
    expect(creds.isLive({ kind: "mcp", id: long.id })).toBe(true);
  });

  test("touch writes lastUsedAt at most once a minute; restart keeps it", async () => {
    const c = clock();
    const creds = open(c);
    const { id } = creds.mint(CONSOLE, { kind: "agent", label: "a" }).credential;
    const stored = () => (store.db.prepare("SELECT lastUsedAt FROM credentials WHERE id = ?").get(id) as { lastUsedAt: number | null }).lastUsedAt;
    creds.touch(id);
    const first = c.now();
    expect(stored()).toBe(first);
    c.advance(TOUCH_WRITE_MS - 1);
    creds.touch(id);
    expect(stored()).toBe(first);
    expect(creds.list()[0]!.lastUsedAt).toBe(c.now());
    c.advance(1);
    creds.touch(id);
    expect(stored()).toBe(c.now());
    await store.close();
    const again = open(c);
    expect(again.list()[0]).toMatchObject({ id, lastUsedAt: c.now(), status: "live" });
  });

  test("mint_revoke_expire_are_audited, through the one revoke path", () => {
    const c = clock();
    const creds = open(c);
    const a = creds.mint(CONSOLE, { kind: "agent", label: "a" }).credential;
    const b = creds.mint(CONSOLE, { kind: "agent", label: "b" }).credential;
    const registry = new ConnectionRegistry();
    registry.useLiveness((p) => creds.isLive(p));
    const tickets = new Tickets(c.now);
    const pa = creds.lookup({ kind: "agent", id: a.id })!.principal;
    tickets.mint(pa);
    const deps = (reason: "owner" | "expired") => ({ audit, tickets, connections: registry, revoke: (r: { kind: "agent" | "mcp"; id: string }) => creds.end(r, reason) });
    expect(revokeCredential(deps("owner"), { kind: "agent", id: a.id }, { rpc: "auth.credential.revoke", reason: "owner" })).toEqual({ revoked: true, tickets: 1, connections: 0 });
    // Twice is a no-op, and not audited again.
    expect(revokeCredential(deps("owner"), { kind: "agent", id: a.id }, { rpc: "auth.credential.revoke", reason: "owner" }).revoked).toBe(false);
    c.advance(DAY);
    expect(sweepExpired(deps("expired"), () => creds.expired())).toBe(1);
    expect(sweepExpired(deps("expired"), () => creds.expired())).toBe(0);
    const rows = audit.list({}).entries.map((e) => ({ kind: e.kind, principal: `${e.principalKind}:${e.principalId}`, credentialKind: e.credentialKind, details: e.details }));
    expect(rows.reverse()).toEqual([
      { kind: "credential.mint", principal: "console:console", credentialKind: "agent", details: { credentialId: a.id, scopes: MINT_DEFAULT_SCOPES } },
      { kind: "credential.mint", principal: "console:console", credentialKind: "agent", details: { credentialId: b.id, scopes: MINT_DEFAULT_SCOPES } },
      { kind: "credential.revoke", principal: `agent:${a.id}`, credentialKind: "agent", details: { rpc: "auth.credential.revoke", reason: "owner", count: 0, credentialId: a.id } },
      { kind: "credential.expire", principal: `agent:${b.id}`, credentialKind: "agent", details: { reason: "expired", count: 0, credentialId: b.id } },
    ]);
    expect(creds.list(true).map((x) => [x.id, x.status]).sort()).toEqual([[a.id, "revoked"], [b.id, "expired"]].sort());
    expect(creds.list()).toEqual([]);
    // Nothing secret in any row.
    expect(JSON.stringify(audit.list({}).entries)).not.toMatch(/key|[A-Za-z0-9_-]{43}/);
  });

  test("credential_survives_server_restart (the store; revocation persists)", async () => {
    const creds = open();
    const live = creds.mint(CONSOLE, { kind: "agent", label: "a", branches: ["own"] }).credential;
    const gone = creds.mint(CONSOLE, { kind: "agent", label: "b" }).credential;
    creds.end({ kind: "agent", id: gone.id }, "owner");
    await store.close();
    const again = open();
    expect(again.lookup({ kind: "agent", id: live.id })?.principal.branches).toEqual(["own"]);
    expect(again.keyFor("agent", live.id)).toEqual(credentialKey(master, store.envId, "agent", live.id));
    expect(again.lookup({ kind: "agent", id: gone.id })).toBeUndefined();
  });
});

test("child_env_passes_fin_credential_path (and never one inherited)", () => {
  const parent = { PATH: "/bin", FIN_CREDENTIAL: "/elsewhere/cred.json", HOME: "/h" };
  expect(childEnv({ FIN_CREDENTIAL: "/s/agent.json" }, parent, "linux")).toEqual({ PATH: "/bin", HOME: "/h", FIN_CREDENTIAL: "/s/agent.json" });
  expect(childEnv({}, parent, "linux")).toEqual({ PATH: "/bin", HOME: "/h" });
});

test("approvals are bounded: at most MAX_APPROVALS, each for APPROVAL_TTL_MS (review of Task 5, L6)", () => {
  const approvals = new Map<string, { tag: string; principal: string; at: number }>();
  for (let i = 0; i < MAX_APPROVALS + 10; i++) keepApproval(approvals, `a${i}`, { tag: "t", principal: `agent:${i % 20}`, at: 1000 + i });
  expect(approvals.size).toBe(MAX_APPROVALS);
  expect(approvals.has("a0")).toBe(false);
  expect(approvals.has(`a${MAX_APPROVALS + 9}`)).toBe(true);
  keepApproval(approvals, "late", { tag: "t", principal: "agent:x", at: 1000 + MAX_APPROVALS + 9 + APPROVAL_TTL_MS });
  expect([...approvals.keys()]).toEqual(["late"]);
});

test("review of Tasks 8–9, L5: one principal keeps at most MAX_APPROVALS_PER_PRINCIPAL pending", () => {
  const approvals = new Map<string, { tag: string; principal: string; at: number }>();
  for (let i = 0; i < 10; i++) keepApproval(approvals, `other${i}`, { tag: "t", principal: "agent:other", at: 1000 + i });
  // A loop of one agent asking pushes out only its own oldest, never the others'.
  for (let i = 0; i < MAX_APPROVALS * 2; i++) keepApproval(approvals, `loop${i}`, { tag: "t", principal: "agent:loop", at: 2000 + i });
  const of = (who: string) => [...approvals.values()].filter((v) => v.principal === who).length;
  expect(of("agent:loop")).toBe(MAX_APPROVALS_PER_PRINCIPAL);
  expect(of("agent:other")).toBe(10);
  expect(approvals.has(`loop${MAX_APPROVALS * 2 - 1}`)).toBe(true);
  expect(approvals.has(`loop${MAX_APPROVALS * 2 - 1 - MAX_APPROVALS_PER_PRINCIPAL}`)).toBe(false);
});

// --- A server in this process, with a clock the test moves ---------------------------------

let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];
const time = clock(Date.now());

/** `sweepMs`: how often the server's expiry sweep runs (real time; a day: it never does in the test). */
async function startInProcess(sweepMs = 20): Promise<ServerInfo> {
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(
    serve({ serverData, credentials: { clock: time.now, sweepMs }, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)),
  );
  running = { fiber, info: await info };
  return running.info;
}

async function stopServer(): Promise<void> {
  if (running === undefined) return;
  await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
}

async function asConsole(): Promise<NdjsonClient> {
  const c = await connectProved({ info: running!.info, serverData }, "console");
  clients.push(c);
  return c;
}

async function mint(req: Record<string, unknown>): Promise<{ credential: Credential; file: CredentialFile }> {
  return ok(await confirmed(await asConsole(), "auth.credential.mint", { kind: "agent", label: "test", ...req }));
}

/** Connects and proves `file` as `fin` does; the `auth.prove` exit (and the hello's proof). */
async function proveAs(file: CredentialFile): Promise<{ client: NdjsonClient; exit: Exit; proved: boolean }> {
  const info = running!.info;
  const client = await NdjsonClient.connect(info.socket);
  clients.push(client);
  const key = b64u.decode(file.key)!;
  const endpoint = canonicalEndpoint(info.socket);
  const { clientNonce, exit } = await hello(client, { env: file.env, endpoint, credential: { kind: file.kind, id: file.id } });
  const answer = ok<{ envId: string; endpoint: string; serverNonce: string; proof?: string }>(exit);
  const serverNonce = b64u.decode(answer.serverNonce)!;
  const proof = answer.proof === undefined ? null : b64u.decode(answer.proof);
  const proved = proof !== null && sameMac(proof, helloProof(key, answer.envId, answer.endpoint, clientNonce, serverNonce));
  const mac = proveMac(key, answer.envId, answer.endpoint, serverNonce, clientNonce);
  return { client, exit: await client.call("auth.prove", { mac: b64u.encode(mac) }), proved };
}

async function agentClient(file: CredentialFile): Promise<NdjsonClient> {
  const { client, exit, proved } = await proveAs(file);
  expect(proved).toBe(true);
  ok(exit);
  return client;
}

async function project(c: NdjsonClient): Promise<string> {
  return ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
}

describe("credentials on a server", () => {
  test("mint_then_prove_as_agent_and_mcp: scopes exactly as minted, not human", async () => {
    await startInProcess();
    for (const kind of ["agent", "mcp"] as const) {
      const { credential, file } = await mint({ kind, scopes: ["server.read", "project.read"], projects: ["01aaaaaaaaaaaaaaaaaaaaaaaa"] });
      expect(file.env).toBe(running!.info.envId);
      const c = await agentClient(file);
      const p = ok<{ principal: Principal }>(await c.call("auth.whoami", {})).principal;
      expect(p).toEqual({ kind, id: credential.id, scopes: ["server.read", "project.read"], projects: ["01aaaaaaaaaaaaaaaaaaaaaaaa"], branches: "*", human: false });
    }
    // An unknown id, or a key for another id, gets no proof and cannot prove.
    const { file } = await mint({});
    for (const f of [{ ...file, id: "01zzzzzzzzzzzzzzzzzzzzzzzz" }, { ...file, kind: "mcp" as const }]) {
      const r = await proveAs(f);
      expect(r.proved).toBe(false);
      expect(code(r.exit)).toBe("UNAUTHENTICATED");
    }
  });

  test("mint_needs_a_human and a challenge code; list reads without one; a credential cannot mint", async () => {
    await startInProcess();
    const local = await connectProved({ info: running!.info, serverData });
    clients.push(local);
    expect(code(await local.call("auth.credential.mint", { kind: "agent", label: "x" }))).toBe("HUMAN_ONLY");
    const console = await asConsole();
    expect(code(await console.call("auth.credential.mint", { kind: "agent", label: "x" }))).toBe("CONFIRMATION_REQUIRED");
    expect(code(await console.call("auth.credential.revoke", { id: "01zzzzzzzzzzzzzzzzzzzzzzzz" }))).toBe("CONFIRMATION_REQUIRED");
    expect(ok<{ credentials: unknown[] }>(await console.call("auth.credential.list", {})).credentials).toEqual([]);
    expect(code(await confirmed(console, "auth.credential.mint", { kind: "agent", label: "x", scopes: ["auth.admin"] }))).toBe("SCOPE_DENIED");
    const { file } = await mint({ scopes: ["server.read", "project.read", "project.write"] });
    const a = await agentClient(file);
    for (const tag of ["auth.credential.mint", "auth.credential.list", "auth.credential.revoke"]) {
      expect(code(await a.call(tag, { kind: "agent", label: "x", id: file.id })), tag).toBe("HUMAN_ONLY");
    }
  });

  test("agent_scopes_limit_rpcs", async () => {
    await startInProcess();
    const local = await connectProved({ info: running!.info, serverData });
    clients.push(local);
    const projectId = await project(local);
    ok(await commit(local, projectId, tx("t0", 0, [addAsset()])));
    const { file } = await mint({ scopes: ["server.read", "project.read"] });
    const reader = await agentClient(file);
    expect(code(await reader.call("snapshot.get", { projectId, branch: "main" }))).toBeUndefined();
    const generation = await generationOf(reader, projectId);
    expect(code(await reader.call("tx.commit", { projectId, tx: tx("t1", 1, [insert(0)]), generation }))).toBe("SCOPE_DENIED");
    expect(code(await reader.call("diagnostics.status", {}))).toBe("SCOPE_DENIED");
    expect(code(await reader.call("auth.ticket", {}))).toBeUndefined();
  });

  test("branches own: a commit to main is BRANCH_NOT_ALLOWED, to a branch it made works; writes name the credential", async () => {
    await startInProcess();
    const local = await connectProved({ info: running!.info, serverData });
    clients.push(local);
    const projectId = await project(local);
    ok(await commit(local, projectId, tx("t0", 0, [addAsset()])));
    const { file } = await mint({ branches: ["own"] });
    const a = await agentClient(file);
    expect(code(await commit(a, projectId, tx("t1", 1, [insert(0)])))).toBe("BRANCH_NOT_ALLOWED");
    ok(await a.call("branch.create", { projectId, txId: "01bbbbbbbbbbbbbbbbbbbbbbbb", author: "someone", from: "main", name: "mine" }));
    const branches = ok<{ branches: Array<{ id: string; name: string; createdBy: string }> }>(await a.call("branch.list", { projectId })).branches;
    const mine = branches.find((b) => b.name === "mine");
    expect(mine?.createdBy, JSON.stringify(branches)).toBe(`agent:${file.id}`);
    expect(code(await commit(a, projectId, { ...tx("t2", 1, [insert(0)]), branch: mine!.id }))).toBeUndefined();
  });

  test("revoke_closes_live_connections: a live ops.subscribe ends with CREDENTIAL_REVOKED within 100 ms", async () => {
    await startInProcess();
    const local = await connectProved({ info: running!.info, serverData });
    clients.push(local);
    const projectId = await project(local);
    const { credential, file } = await mint({});
    const a = await agentClient(file);
    const other = await agentClient(file);
    const generation = await generationOf(a, projectId);
    const sub = a.stream("ops.subscribe", { projectId, branch: "main", fromRev: 0, generation }, () => undefined);
    await new Promise((r) => setTimeout(r, 50));
    const console = await asConsole();
    const at = performance.now();
    expect(ok(await confirmed(console, "auth.credential.revoke", { id: credential.id }))).toEqual({ revoked: true });
    const ended = await sub.done;
    expect(performance.now() - at).toBeLessThan(100);
    expect(code(ended)).toBe("CREDENTIAL_REVOKED");
    expect(code(await other.call("auth.whoami", {}))).not.toBeUndefined();
    // Never proved again, now or after a restart: the server proves itself, then refuses it by
    // name (review of Tasks 8–9, L4); revoking again changes nothing.
    const now = await proveAs(file);
    expect([now.proved, code(now.exit)]).toEqual([true, "CREDENTIAL_REVOKED"]);
    expect(ok(await confirmed(console, "auth.credential.revoke", { id: credential.id }))).toEqual({ revoked: false });
    await stopServer();
    await startInProcess();
    const r = await proveAs(file);
    expect([r.proved, code(r.exit)]).toEqual([true, "CREDENTIAL_REVOKED"]);
    // The refused connection stays unproved.
    expect(code(await r.client.call("auth.whoami", {}))).toBe("UNAUTHENTICATED");
  });

  test("credential_survives_server_restart", async () => {
    await startInProcess();
    const { credential, file } = await mint({ kind: "mcp" });
    await stopServer();
    await startInProcess();
    const c = await agentClient(file);
    expect(ok<{ principal: Principal }>(await c.call("auth.whoami", {})).principal.id).toBe(credential.id);
  });

  /** An agent with a live `ops.subscribe` stream and a second connection, both on one credential. */
  async function expiring() {
    const local = await connectProved({ info: running!.info, serverData });
    clients.push(local);
    const projectId = await project(local);
    const { file } = await mint({ idleS: 3600 });
    const a = await agentClient(file);
    const generation = await generationOf(a, projectId);
    const sub = a.stream("ops.subscribe", { projectId, branch: "main", fromRev: 0, generation }, () => undefined);
    const b = await agentClient(file);
    expect(code(await b.call("auth.whoami", {}))).toBeUndefined();
    return { sub, b };
  }

  async function expectedExpired() {
    const console = await asConsole();
    const listed = ok<{ credentials: Credential[] }>(await console.call("auth.credential.list", { all: true })).credentials;
    expect(listed.map((c) => c.status)).toEqual(["expired"]);
    const kinds = ok<{ entries: Array<{ kind: string }> }>(await console.call("auth.audit.list", { kind: "credential.expire" })).entries;
    expect(kinds).toHaveLength(1);
  }

  // The two halves run apart: with a sweep every 20 ms, the sweep can close the connection between the
  // clock's move and the request, and the request then fails with SERVER_UNAVAILABLE, not the refusal.
  test("expiry refuses requests at once, before any sweep has run", async () => {
    await startInProcess(86_400_000);
    const { b } = await expiring();
    time.advance(3_600_000);
    // Expired: refused before the sweep has run (the next request re-checks).
    expect(code(await b.call("auth.whoami", {}))).toBe("CREDENTIAL_REVOKED");
  });

  test("expiry closes live connections within the sweep", async () => {
    await startInProcess();
    const { sub } = await expiring();
    time.advance(3_600_000);
    expect(code(await sub.done)).toBe("CREDENTIAL_REVOKED");
    await expectedExpired();
  });

  test("a local key cannot be named as an agent credential, and credential.master is never readable through RPC", async () => {
    await startInProcess();
    const { file } = await mint({});
    const master = readKeyFile(join(authDir(serverData), "credential.master"));
    const forged = { ...file, key: b64u.encode(credentialKey(Buffer.alloc(32, 1), file.env, "agent", file.id)) };
    expect((await proveAs(forged)).proved).toBe(false);
    expect(b64u.decode(file.key)).toEqual(credentialKey(master, file.env, "agent", file.id));
    const console = await asConsole();
    const listed = JSON.stringify(ok(await console.call("auth.credential.list", { all: true })));
    expect(listed).not.toContain(file.key);
  });
});
