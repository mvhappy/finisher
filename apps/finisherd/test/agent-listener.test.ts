// The Linux agent listener (plan 7C Task 6, spec §8.3): loopback TCP with the local socket's
// framing and handshake, the canonical endpoint `tcp:127.0.0.1:<port>` in both MACs, `agent` and
// `mcp` principals only (others refused after the proof and audited `listenerKind`), a handshake
// timeout and a connection cap. Units on `Connections`' listener rules, then a server run in
// this process.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Credential, CredentialFile } from "@finisher/contracts";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AGENT_LISTENER_KINDS, type AgentListener, agentConnections, agentEndpoint, makeAgentListener, rateLimited } from "../src/agent/listener.ts";
import { b64u, canonicalEndpoint, helloProof, proveMac, sameMac } from "../src/auth/crypto.ts";
import type { Link } from "../src/connections.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { ANONYMOUS, LOCAL } from "../src/principal.ts";
import { confirmed, connectProved, type Exit, hello, keyOf, NdjsonClient, ok, type ServerInfo } from "./support/ndjson-client.ts";

const code = (e: Exit) => (e.ok ? undefined : e.error.code);

describe("listener rules (units)", () => {
  const link = () => {
    const l = { closed: 0, close: () => void l.closed++, closeAfterReply: () => {}, terminate: () => {} };
    return l as Link & { closed: number };
  };
  const timers = () => {
    const pending: Array<{ f: () => void; cleared: boolean }> = [];
    return {
      pending,
      set: (f: () => void) => {
        const h = { f, cleared: false };
        pending.push(h);
        return h;
      },
      clear: (h: unknown) => void ((h as { cleared: boolean }).cleared = true),
      fire: () => {
        for (const h of pending.splice(0)) if (!h.cleared) h.f();
      },
    };
  };

  test("admits_agent_and_mcp_only", () => {
    const c = agentConnections(() => "tcp:127.0.0.1:1");
    expect([...AGENT_LISTENER_KINDS].sort()).toEqual(["agent", "mcp"]);
    expect(c.transport).toBe("agentTcp");
    for (const kind of ["agent", "mcp"]) expect(c.admits({ ...LOCAL, kind } as typeof LOCAL)).toBe(true);
    for (const p of [LOCAL, { ...LOCAL, kind: "console" }, { ...LOCAL, kind: "device" }, ANONYMOUS]) expect(c.admits(p as typeof LOCAL)).toBe(false);
  });

  test("unproved_connection_is_cut_after_the_timeout", () => {
    const t = timers();
    const c = agentConnections(() => "e", { timers: t, handshakeTimeoutMs: 50 });
    const silent = link();
    const proved = link();
    const gone = link();
    c.attach(1, silent);
    c.attach(2, proved).principal = { ...LOCAL, kind: "agent", id: "a1" } as typeof LOCAL;
    c.attach(3, gone);
    c.close(3);
    t.fire();
    expect([silent.closed, proved.closed, gone.closed]).toEqual([1, 0, 0]);
    expect(t.pending.length).toBe(0);
  });

  test("unproved_cap_keeps_slots_for_proved_agents", () => {
    const refused: string[] = [];
    const c = agentConnections(() => "e", { maxConnections: 4, maxUnproved: 2, timers: timers(), onRefused: (cap) => refused.push(cap) });
    const agent = { ...LOCAL, kind: "agent", id: "a1" } as typeof LOCAL;
    const ls = [link(), link(), link()];
    ls.forEach((l, i) => c.attach(i + 1, l));
    // The third unproved one is refused; a connection that arrives proved is not.
    expect(ls.map((l) => l.closed)).toEqual([0, 0, 1]);
    c.get(1)!.principal = agent;
    const later = link();
    c.attach(5, later);
    expect(later.closed).toBe(0);
    expect(refused).toEqual(["unproved"]);
  });

  test("refusals_are_logged_at_most_once_per_period", () => {
    let t = 0;
    const seen: number[] = [];
    const f = rateLimited(1000, (n) => seen.push(n), () => t);
    f();
    f();
    f();
    t = 1500;
    f();
    expect(seen).toEqual([1, 3]);
  });

  test("connection_cap_holds", () => {
    const c = agentConnections(() => "e", { maxConnections: 2, timers: timers() });
    const links = [link(), link(), link()];
    links.forEach((l, i) => c.attach(i + 1, l));
    expect(links.map((l) => l.closed)).toEqual([0, 0, 1]);
    expect(c.has(3)).toBe(false);
    c.close(1);
    const next = link();
    c.attach(4, next);
    expect(next.closed).toBe(0);
  });

  test("ensure_starts_once_and_retries_a_failure", async () => {
    let n = 0;
    const l = makeAgentListener(async () => {
      n++;
      if (n === 1) throw new Error("boom");
      return "tcp:127.0.0.1:9";
    });
    expect(l.current()).toBeUndefined();
    await expect(l.ensure()).rejects.toThrow("boom");
    const [a, b] = await Promise.all([l.ensure(), l.ensure()]);
    expect([a, b, n, l.current()]).toEqual(["tcp:127.0.0.1:9", "tcp:127.0.0.1:9", 2, "tcp:127.0.0.1:9"]);
    expect(agentEndpoint(4100)).toBe("tcp:127.0.0.1:4100");
  });
});

describe("the agent listener on a server", () => {
  let tmp: string;
  let serverData: string;
  let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo; listener: AgentListener } | undefined;
  const clients: NdjsonClient[] = [];

  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-listener-"));
    serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
  });
  afterEach(async () => {
    for (const c of clients.splice(0)) await c.close();
    if (running !== undefined) await Effect.runPromise(Fiber.interrupt(running.fiber));
    running = undefined;
    rmSync(tmp, { recursive: true, force: true });
  });

  async function start(rules: { handshakeTimeoutMs?: number; maxConnections?: number } = {}) {
    let ready!: (i: ServerInfo) => void;
    let listener!: AgentListener;
    const info = new Promise<ServerInfo>((r) => (ready = r));
    const fiber = Effect.runFork(
      serve({ serverData, agentListener: rules, onAgentListener: (l) => (listener = l), onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)),
    );
    running = { fiber, info: await info, listener };
    return running;
  }

  const port = (endpoint: string) => Number(/^tcp:127\.0\.0\.1:(\d+)$/.exec(endpoint)![1]);
  const tcp = async (endpoint: string) => {
    const c = await NdjsonClient.connect({ host: "127.0.0.1", port: port(endpoint) });
    clients.push(c);
    return c;
  };
  const console_ = async () => {
    const c = await connectProved({ info: running!.info, serverData }, "console");
    clients.push(c);
    return c;
  };
  const mint = async (kind: "agent" | "mcp") =>
    ok<{ credential: Credential; file: CredentialFile }>(await confirmed(await console_(), "auth.credential.mint", { kind, label: "t" }));

  /** The handshake as `fin` does it, binding `boundTo` as the endpoint. */
  async function proveOver(c: NdjsonClient, key: Buffer, credential: unknown, env: string, boundTo: string) {
    const { clientNonce, exit } = await hello(c, { env, endpoint: boundTo, credential });
    const a = ok<{ envId: string; endpoint: string; serverNonce: string; proof?: string }>(exit);
    const sn = b64u.decode(a.serverNonce)!;
    const proved = a.proof !== undefined && sameMac(b64u.decode(a.proof)!, helloProof(key, a.envId, a.endpoint, clientNonce, sn));
    const prove = await c.call("auth.prove", { mac: b64u.encode(proveMac(key, a.envId, boundTo, sn, clientNonce)) });
    return { answer: a, proved, prove };
  }

  test("starts_on_first_need_binds_loopback_and_lists_itself", async () => {
    const r = await start();
    expect(r.listener.current()).toBeUndefined();
    const status = async () => ok<{ listeners: Array<{ transport: string; endpoint: string }> }>(await (await console_()).call("server.status", {}));
    expect((await status()).listeners.map((l) => l.transport)).toEqual(["socket", "ws"]);
    const endpoint = await r.listener.ensure();
    expect(endpoint).toMatch(/^tcp:127\.0\.0\.1:\d+$/);
    expect(await r.listener.ensure()).toBe(endpoint);
    expect((await status()).listeners).toContainEqual({ transport: "agentTcp", endpoint });
  });

  test("agent_and_mcp_prove_with_the_tcp_endpoint_in_both_macs", async () => {
    const r = await start();
    const endpoint = await r.listener.ensure();
    for (const kind of ["agent", "mcp"] as const) {
      const { credential, file } = await mint(kind);
      const key = b64u.decode(file.key)!;
      const c = await tcp(endpoint);
      const { answer, proved, prove } = await proveOver(c, key, { kind, id: credential.id }, file.env, endpoint);
      expect(answer.endpoint).toBe(endpoint);
      expect(proved).toBe(true);
      expect(ok<{ principal: { kind: string } }>(prove).principal.kind).toBe(kind);
      expect(ok<{ principal: { kind: string; id: string } }>(await c.call("auth.whoami", {})).principal).toMatchObject({ kind, id: credential.id });
      // A proof bound to another endpoint (the socket's) gets no server proof and fails.
      const other = await tcp(endpoint);
      const wrong = await proveOver(other, key, { kind, id: credential.id }, file.env, canonicalEndpoint(r.info.socket));
      expect(wrong.proved).toBe(false);
      expect(code(wrong.prove)).toBe("UNAUTHENTICATED");
    }
  });

  test("local_and_console_are_refused_after_the_proof_and_audited", async () => {
    const r = await start();
    const endpoint = await r.listener.ensure();
    for (const name of ["local", "console"] as const) {
      const c = await tcp(endpoint);
      const { proved, prove } = await proveOver(c, keyOf(serverData, name), { kind: name }, r.info.envId, endpoint);
      // The server proved itself (the key is right), then refused the kind.
      expect(proved).toBe(true);
      expect(prove.ok ? undefined : prove.error).toMatchObject({ code: "UNAUTHENTICATED", details: { reason: "listenerKind" } });
      // And the connection is closed after the answer.
      await c.closed();
    }
    const audit = ok<{ entries: Array<{ kind: string; details: Record<string, unknown> }> }>(await (await console_()).call("auth.audit.list", {}));
    const refused = audit.entries.filter((e) => e.kind === "auth.fail" && e.details.reason === "listenerKind");
    expect(refused.map((e) => [(e as { credentialKind?: string }).credentialKind, e.details.transport]).sort()).toEqual([
      ["console", "agentTcp"],
      ["local", "agentTcp"],
    ]);
  });

  test("a_silent_client_is_cut_after_the_timeout", async () => {
    const r = await start({ handshakeTimeoutMs: 200 });
    const silent = await tcp(await r.listener.ensure());
    const t0 = Date.now();
    await silent.closed();
    expect(Date.now() - t0).toBeLessThan(5_000);
  });

  test("the_cap_holds", async () => {
    // A long timeout: a refused connection closes at once, not by its timer.
    const r = await start({ handshakeTimeoutMs: 60_000, maxConnections: 2 });
    const endpoint = await r.listener.ensure();
    const a = await tcp(endpoint);
    const b = await tcp(endpoint);
    const over = await tcp(endpoint);
    const t0 = Date.now();
    await over.closed();
    expect(Date.now() - t0).toBeLessThan(10_000);
    for (const c of [a, b]) expect(code(await c.call("server.hello", {}))).toBeUndefined();
  });
});
