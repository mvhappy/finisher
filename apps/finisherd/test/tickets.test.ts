// 5A Task 6: one-time tickets, the upgrade check (path, version, Host, Origin, ticket) and
// `auth.revokeSelf`, as units.
import { Effect } from "effect";
import { describe, expect, test } from "vitest";
import { revokeSelf } from "../src/auth/handshake.ts";
import { revokeCredential } from "../src/auth/revoke.ts";
import { ConnectionRegistry, Connections, type Link } from "../src/connections.ts";
import { LOCAL_MINT_RATE, MAX_LIVE_TICKETS, MAX_TICKETS_LOCAL, MAX_TICKETS_PER_PRINCIPAL, MINT_RATE, Tickets } from "../src/auth/tickets.ts";
import type { Audit } from "../src/control/audit.ts";
import { CONSOLE, LOCAL, type Principal } from "../src/principal.ts";
import { makeUpgradeCheck, type UpgradeResult } from "../src/upgrade.ts";

const PORT = 41234;
const agent = (id: string): Principal => ({ kind: "agent", id, scopes: ["server.read"], projects: "*", branches: "*", human: false });

function clock(start = 1_000_000) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

describe("tickets", () => {
  test("ticket_is_one_time", () => {
    const t = new Tickets();
    const { ticket } = t.mint(LOCAL);
    expect(ticket).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(t.take(ticket)).toEqual(LOCAL);
    expect(t.take(ticket)).toBeUndefined();
  });

  test("ticket_expires", () => {
    const c = clock();
    const t = new Tickets(c.now);
    const { ticket, expiresAt } = t.mint(LOCAL);
    expect(expiresAt).toBe(c.now() + 30_000);
    c.advance(30_000);
    expect(t.take(ticket)).toBeUndefined();
    // Swept, and an expired ticket is used up by the attempt too.
    const second = t.mint(LOCAL, 5).ticket;
    c.advance(6_000);
    t.sweep();
    expect(t.size).toBe(0);
    expect(t.take(second)).toBeUndefined();
  });

  test("ticket_ttl_capped_at_60", () => {
    const c = clock();
    const t = new Tickets(c.now);
    expect(t.mint(LOCAL, 3600).expiresAt).toBe(c.now() + 60_000);
    expect(t.mint(LOCAL, 0).expiresAt).toBe(c.now() + 1_000);
    expect(t.mint(LOCAL, 12.7).expiresAt).toBe(c.now() + 12_000);
  });

  test("malformed tickets are not looked up and do not use up a real one", () => {
    const t = new Tickets();
    const { ticket } = t.mint(LOCAL);
    expect(t.take(`${ticket}x`)).toBeUndefined();
    expect(t.take("")).toBeUndefined();
    expect(t.take(ticket)).toEqual(LOCAL);
  });

  test("a principal's flood evicts only its own tickets (review of Task 6, finding 4)", () => {
    const c = clock();
    const t = new Tickets(c.now);
    // The UI's ticket, minted first.
    const ui = t.mint(CONSOLE).ticket;
    const flooder = agent("01floodfloodfloodfloodfloo");
    const firsts: string[] = [];
    for (let i = 0; i < 40; i++) {
      c.advance(1000 / MINT_RATE.perSecond); // within the mint rate
      const minted = t.mint(flooder).ticket;
      if (i < 2) firsts.push(minted);
    }
    // At most 8 of its own, its oldest dropped; the UI's ticket untouched.
    expect(t.size).toBe(MAX_TICKETS_PER_PRINCIPAL + 1);
    for (const f of firsts) expect(t.take(f)).toBeUndefined();
    expect(t.take(ui)).toEqual(CONSOLE);
  });

  test("minting is rate limited per principal", () => {
    const c = clock();
    const t = new Tickets(c.now);
    const a = agent("01aaaaaaaaaaaaaaaaaaaaaaaa");
    for (let i = 0; i < MINT_RATE.burst; i++) t.mint(a);
    expect(() => t.mint(a)).toThrow(expect.objectContaining({ code: "TICKET_LIMIT", details: { reason: "rateLimited" } }));
    // Another principal is not slowed down; the first refills at its rate.
    expect(t.mint(LOCAL).ticket).toMatch(/^[A-Za-z0-9_-]{43}$/);
    c.advance(1000 / MINT_RATE.perSecond);
    expect(t.mint(a).ticket).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  test("past the global cap a mint is refused, nobody's ticket dropped", () => {
    const c = clock();
    const t = new Tickets(c.now);
    const kept: string[] = [];
    for (let i = 0; t.size < MAX_LIVE_TICKETS; i++) kept.push(t.mint(agent(`01${String(i).padStart(24, "0")}`)).ticket);
    expect(() => t.mint(LOCAL)).toThrow(expect.objectContaining({ code: "TICKET_LIMIT", details: { reason: "full" } }));
    expect(t.size).toBe(MAX_LIVE_TICKETS);
    expect(t.take(kept[0]!)).toBeDefined();
    // Expired ones make room again.
    c.advance(60_000);
    expect(t.mint(LOCAL).ticket).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  test("local tickets are limited per connection, under a cap for local as a whole (review of Task 7, M1)", () => {
    const c = clock();
    const t = new Tickets(c.now);
    // One looping client spends its own burst; another `fin` is not slowed down.
    for (let i = 0; i < MINT_RATE.burst; i++) t.mint(LOCAL, 30, "socket:1");
    expect(() => t.mint(LOCAL, 30, "socket:1")).toThrow(expect.objectContaining({ code: "TICKET_LIMIT", details: { reason: "rateLimited" } }));
    expect(t.mint(LOCAL, 30, "socket:2").ticket).toMatch(/^[A-Za-z0-9_-]{43}$/);
    // Its own 8 live tickets at most: the newer drop its own oldest, never another connection's.
    expect(t.size).toBe(MAX_TICKETS_PER_PRINCIPAL + 1);
    const other = t.mint(LOCAL, 30, "socket:2").ticket;
    for (let i = 0; i < 4; i++) {
      c.advance(1000 / MINT_RATE.perSecond);
      t.mint(LOCAL, 30, "socket:1");
    }
    expect(t.take(other)).toEqual(LOCAL);
    // `local` as a whole: at most MAX_TICKETS_LOCAL live, then refused (no eviction).
    const t2 = new Tickets(c.now);
    let minted = 0;
    let refused: unknown;
    for (let conn = 0; conn < 20 && refused === undefined; conn++) {
      for (let i = 0; i < MAX_TICKETS_PER_PRINCIPAL; i++) {
        c.advance(1000 / LOCAL_MINT_RATE.perSecond);
        try {
          t2.mint(LOCAL, 60, `socket:${conn}`);
          minted++;
        } catch (e) {
          refused = e;
          break;
        }
      }
    }
    expect(minted).toBe(MAX_TICKETS_LOCAL);
    expect(refused).toMatchObject({ code: "TICKET_LIMIT", details: { reason: "full" } });
    // And a whole-`local` mint rate above the per-connection one.
    const t3 = new Tickets(c.now);
    let n = 0;
    for (let conn = 0; conn < 10; conn++) {
      for (let i = 0; i < MINT_RATE.burst; i++) {
        try {
          t3.mint(LOCAL, 1, `ws:${conn}`);
          n++;
        } catch {
          break;
        }
      }
    }
    expect(n).toBe(LOCAL_MINT_RATE.burst);
  });

  test("dropWhere drops one principal's tickets", () => {
    const t = new Tickets();
    const a = t.mint(agent("01aaaaaaaaaaaaaaaaaaaaaaaa")).ticket;
    const l = t.mint(LOCAL).ticket;
    expect(t.dropWhere((p) => p.kind === "agent")).toBe(1);
    expect(t.take(a)).toBeUndefined();
    expect(t.take(l)).toEqual(LOCAL);
  });
});

describe("upgrade check", () => {
  const setup = (origins: string[] = []) => {
    const tickets = new Tickets();
    const check = makeUpgradeCheck({ port: () => PORT, tickets, origins: () => origins });
    const req = (headers: Record<string, string>, url = "/rpc") => check({ url, headers: { host: `127.0.0.1:${PORT}`, ...lower(headers) } });
    return { tickets, check, req };
  };
  const lower = (h: Record<string, string>) => Object.fromEntries(Object.entries(h).map(([k, v]) => [k.toLowerCase(), v]));
  const proto = (ticket?: string, version = "finisher.v2") => ({ "sec-websocket-protocol": [version, ...(ticket ? [`finisher.ticket.${ticket}`] : [])].join(", ") });
  const failed = (r: UpgradeResult) => (r.ok ? undefined : { status: r.status, reason: r.reason });

  test("ticket_in_subprotocol_header", () => {
    const { tickets, req } = setup();
    const r = req(proto(tickets.mint(LOCAL).ticket));
    expect(r).toEqual({ ok: true, principal: LOCAL, owner: "local:", protocol: "finisher.v2" });
  });

  test("ticket_not_accepted_in_query", () => {
    const { tickets, req } = setup();
    const { ticket } = tickets.mint(LOCAL);
    expect(failed(req(proto(), `/rpc?ticket=${ticket}`))).toEqual({ status: 401, reason: "noTicket" });
    expect(tickets.take(ticket)).toEqual(LOCAL);
  });

  test("ws_without_ticket_is_401; two tickets or an unknown one too", () => {
    const { tickets, req } = setup();
    expect(failed(req(proto()))).toEqual({ status: 401, reason: "noTicket" });
    const a = tickets.mint(LOCAL).ticket;
    const b = tickets.mint(LOCAL).ticket;
    expect(failed(req({ "sec-websocket-protocol": `finisher.v2, finisher.ticket.${a}, finisher.ticket.${b}` }))).toEqual({ status: 401, reason: "noTicket" });
    expect(failed(req(proto("A".repeat(43))))).toEqual({ status: 401, reason: "badTicket" });
  });

  test("ws_wrong_version_is_426_with_range", () => {
    const { tickets, req } = setup();
    const { ticket } = tickets.mint(LOCAL);
    expect(req(proto(ticket, "finisher.v1"))).toEqual({ ok: false, status: 426, reason: "badVersion", body: { protocol: { min: 2, max: 2 } } });
    expect(failed(req({}))).toEqual({ status: 426, reason: "badVersion" });
    // The ticket was not consumed.
    expect(tickets.take(ticket)).toEqual(LOCAL);
  });

  test("the path is /rpc", () => {
    const { tickets, req } = setup();
    expect(failed(req(proto(tickets.mint(LOCAL).ticket), "/"))).toEqual({ status: 404, reason: "badPath" });
  });

  test("ws_bad_host_is_403", () => {
    const { tickets, req } = setup();
    for (const host of ["evil.example:41234", `127.0.0.1:${PORT + 1}`, "127.0.0.1", `attacker.localhost:${PORT}`, `0.0.0.0:${PORT}`]) {
      expect(failed(req({ ...proto(tickets.mint(LOCAL).ticket), host })), host).toEqual({ status: 403, reason: "badHost" });
    }
    for (const host of [`127.0.0.1:${PORT}`, `localhost:${PORT}`, `LOCALHOST:${PORT}`, `[::1]:${PORT}`]) {
      expect(req({ ...proto(tickets.mint(LOCAL).ticket), host }).ok, host).toBe(true);
    }
  });

  test("ws_origin_matrix", () => {
    const { tickets, req } = setup(["https://app.example", "not an origin"]);
    const cases: Array<[string | undefined, boolean]> = [
      [undefined, true],
      [`http://127.0.0.1:${PORT}`, true],
      [`http://localhost:${PORT}`, true],
      [`http://[::1]:${PORT}`, true],
      ["https://app.example", true],
      ["https://app.example:443", true],
      ["https://evil.example", false],
      [`http://127.0.0.1:${PORT + 1}`, false],
      [`https://127.0.0.1:${PORT}`, false],
      ["null", false],
      ["file://", false],
      ["garbage", false],
    ];
    for (const [origin, allowed] of cases) {
      const headers = { ...proto(tickets.mint(LOCAL).ticket), ...(origin === undefined ? {} : { origin }) };
      expect(req(headers).ok, String(origin)).toBe(allowed);
      if (!allowed) expect(failed(req(headers))).toEqual({ status: 403, reason: "badOrigin" });
    }
  });

  test("host_and_origin_checked_before_ticket_is_consumed", () => {
    const { tickets, req } = setup();
    const { ticket } = tickets.mint(LOCAL);
    expect(failed(req({ ...proto(ticket), host: "evil.example" }))?.status).toBe(403);
    expect(failed(req({ ...proto(ticket), origin: "https://evil.example" }))?.status).toBe(403);
    expect(req(proto(ticket))).toMatchObject({ ok: true, principal: LOCAL });
    expect(failed(req(proto(ticket)))).toEqual({ status: 401, reason: "badTicket" });
  });

  test("a console ticket is a console principal", () => {
    const { tickets, req } = setup();
    expect(req(proto(tickets.mint(CONSOLE).ticket))).toMatchObject({ ok: true, principal: CONSOLE });
  });
});

describe("auth.revokeSelf", () => {
  const audit = () => {
    const rows: unknown[] = [];
    return { rows, audit: { append: (e: unknown) => void rows.push(e) } as unknown as Audit };
  };

  /** Two transports; `links` records what happened to each connection. */
  const transports = () => {
    const links = new Map<string, string[]>();
    const registry = new ConnectionRegistry();
    const socket = registry.add(new Connections("socket", () => "/sd/run/fin.sock"));
    const ws = registry.add(new Connections("ws", () => "ws://127.0.0.1:1"));
    const open = (c: Connections, clientId: number, p: Principal) => {
      const seen: string[] = [];
      links.set(`${c.transport}:${clientId}`, seen);
      const link: Link = {
        close: () => void seen.push("close"),
        closeAfterReply: () => void seen.push("closeAfterReply"),
        terminate: (e) => {
          seen.push(`terminate:${e.code}`);
          c.close(clientId);
        },
      };
      c.attach(clientId, link, p);
    };
    return { links, registry, socket, ws, open };
  };

  test("revoke_self_needs_no_id: an agent credential revokes itself, never another", async () => {
    const { rows, audit: a } = audit();
    const revoked: string[] = [];
    const tickets = new Tickets();
    const self = agent("01selfselfselfselfselfself");
    const other = agent("01otherotherotherotherothe");
    const mine = tickets.mint(self).ticket;
    const theirs = tickets.mint(other).ticket;
    const { links, registry, socket, open } = transports();
    open(socket, 1, self);
    let closed = 0;
    const out = await Effect.runPromise(
      revokeSelf(
        { audit: a, tickets, connections: registry, revoke: (ref) => (revoked.push(`${ref.kind}:${ref.id}`), true) },
        self,
        { transport: "socket", clientId: 1, closeAfterReply: () => closed++ },
      ),
    );
    expect(out).toEqual({});
    expect(revoked).toEqual(["agent:01selfselfselfselfselfself"]);
    // The caller gets its answer, then its connection closes.
    expect(closed).toBe(1);
    expect(links.get("socket:1")).toEqual([]);
    expect(tickets.take(mine)).toBeUndefined();
    expect(tickets.take(theirs)).toEqual(other);
    expect(rows).toEqual([
      {
        kind: "credential.revoke",
        principal: { kind: "agent", id: self.id },
        credentialKind: "agent",
        outcome: "ok",
        details: { rpc: "auth.revokeSelf", reason: "self", count: 0, credentialId: self.id },
      },
    ]);
  });

  test("revocation_reaches_tickets_and_every_live_connection (review of Task 6, finding 3)", async () => {
    const { audit: a } = audit();
    const tickets = new Tickets();
    const self = agent("01selfselfselfselfselfself");
    const other = agent("01otherotherotherotherothe");
    for (let i = 0; i < 3; i++) tickets.mint(self);
    const theirs = tickets.mint(other).ticket;
    const { links, registry, socket, ws, open } = transports();
    // The caller, another socket connection of the same credential (a live stream there), one
    // of its WebSockets, and someone else's connections.
    open(socket, 1, self);
    open(socket, 2, self);
    open(ws, 1, self);
    open(socket, 3, other);
    open(ws, 2, LOCAL);
    await Effect.runPromise(
      revokeSelf({ audit: a, tickets, connections: registry, revoke: () => true }, self, { transport: "socket", clientId: 1, closeAfterReply: () => undefined }),
    );
    expect(links.get("socket:2")).toEqual(["terminate:CREDENTIAL_REVOKED"]);
    expect(links.get("ws:1")).toEqual(["terminate:CREDENTIAL_REVOKED"]);
    expect([links.get("socket:1"), links.get("socket:3"), links.get("ws:2")]).toEqual([[], [], []]);
    expect(tickets.size).toBe(1);
    expect(tickets.take(theirs)).toEqual(other);
    // The same path for the owner's revoke (Task 9): every connection, no exception.
    open(socket, 4, other);
    const report = revokeCredential({ audit: a, tickets, connections: registry }, { kind: "agent", id: other.id }, { rpc: "auth.credential.revoke", reason: "owner" });
    expect(report).toEqual({ revoked: false, tickets: 0, connections: 2 });
    expect(links.get("socket:3")).toEqual(["terminate:CREDENTIAL_REVOKED"]);
    expect(links.get("socket:4")).toEqual(["terminate:CREDENTIAL_REVOKED"]);
  });

  test("a revoked credential cannot come back through a ticket taken before, nor run another request (review of Task 7, M3)", async () => {
    const { audit: a } = audit();
    const tickets = new Tickets();
    const self = agent("01selfselfselfselfselfself");
    const { links, registry, socket, ws, open } = transports();
    open(socket, 1, self);
    expect(socket.principalOf(1)).toEqual(self);
    revokeCredential({ audit: a, tickets, connections: registry }, { kind: "agent", id: self.id }, { rpc: "auth.credential.revoke", reason: "owner" });
    // A WebSocket whose ticket was taken before the revocation registers after it: closed at
    // once, and anonymous meanwhile.
    open(ws, 7, self);
    expect(links.get("ws:7")).toEqual(["close"]);
    expect(ws.principalOf(7).kind).toBe("anonymous");
    // A connection the sweep missed runs nothing more: every request re-checks.
    const missed = new Connections("socket", () => "/sd/run/fin.sock");
    registry.add(missed);
    missed.attach(9, { close: () => undefined, closeAfterReply: () => undefined, terminate: () => undefined }, LOCAL);
    const c = missed.get(9)!;
    c.principal = self;
    expect(missed.principalOf(9).kind).toBe("anonymous");
    expect(missed.revoked(9)).toBe(true);
    // Other credentials and `local` are untouched.
    open(socket, 2, agent("01otherotherotherotherothe"));
    open(socket, 3, LOCAL);
    expect(socket.principalOf(2).kind).toBe("agent");
    expect(socket.principalOf(3)).toEqual(LOCAL);
  });

  test("without a credential store (5A) a revocation is still audited", async () => {
    const { rows, audit: a } = audit();
    const { registry } = transports();
    const self = agent("01selfselfselfselfselfself");
    await Effect.runPromise(
      revokeSelf({ audit: a, tickets: new Tickets(), connections: registry }, self, { transport: "socket", clientId: 1, closeAfterReply: () => undefined }),
    );
    expect(rows).toEqual([expect.objectContaining({ kind: "credential.revoke", details: expect.objectContaining({ rpc: "auth.revokeSelf", reason: "self" }) })]);
    // A store that says it was not live: nothing to audit.
    const { rows: none, audit: b } = audit();
    revokeCredential({ audit: b, tickets: new Tickets(), connections: registry, revoke: () => false }, { kind: "agent", id: self.id }, { rpc: "auth.credential.revoke", reason: "owner" });
    expect(none).toEqual([]);
  });

  test("local and console have nothing of their own to revoke", async () => {
    const { audit: a } = audit();
    for (const p of [LOCAL, CONSOLE]) {
      const deps = { audit: a, tickets: new Tickets(), connections: new ConnectionRegistry(), revoke: () => true };
      const e = await Effect.runPromise(Effect.flip(revokeSelf(deps, p, { transport: "socket", clientId: 1, closeAfterReply: () => undefined })));
      expect(e.code).toBe("INVALID_REQUEST");
      expect(e.fix).not.toMatch(/`|\bfin(isherd)? [a-z]/);
    }
  });
});
