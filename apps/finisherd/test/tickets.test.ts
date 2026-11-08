// 5A Task 6: one-time tickets, the upgrade check (path, version, Host, Origin, ticket) and
// `auth.revokeSelf`, as units.
import { Effect } from "effect";
import { describe, expect, test } from "vitest";
import { revokeSelf } from "../src/auth/handshake.ts";
import { MAX_LIVE_TICKETS, Tickets } from "../src/auth/tickets.ts";
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

  test("a minting flood keeps at most MAX_LIVE_TICKETS", () => {
    const t = new Tickets();
    const first = t.mint(LOCAL).ticket;
    for (let i = 0; i < MAX_LIVE_TICKETS + 10; i++) t.mint(LOCAL);
    expect(t.size).toBe(MAX_LIVE_TICKETS);
    expect(t.take(first)).toBeUndefined();
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
    expect(r).toEqual({ ok: true, principal: LOCAL, protocol: "finisher.v2" });
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

  test("revoke_self_needs_no_id: an agent credential revokes itself, never another", async () => {
    const { rows, audit: a } = audit();
    const revoked: string[] = [];
    const tickets = new Tickets();
    const self = agent("01selfselfselfselfselfself");
    const other = agent("01otherotherotherotherothe");
    const mine = tickets.mint(self).ticket;
    const theirs = tickets.mint(other).ticket;
    let closed = 0;
    const out = await Effect.runPromise(
      revokeSelf({ audit: a, revoke: (ref) => (revoked.push(`${ref.kind}:${ref.id}`), true) }, self, () => closed++, (p) => tickets.dropWhere(p)),
    );
    expect(out).toEqual({});
    expect(revoked).toEqual(["agent:01selfselfselfselfselfself"]);
    expect(closed).toBe(1);
    expect(tickets.take(mine)).toBeUndefined();
    expect(tickets.take(theirs)).toEqual(other);
    expect(rows).toEqual([
      {
        kind: "credential.revoke",
        principal: { kind: "agent", id: self.id },
        credentialKind: "agent",
        outcome: "ok",
        details: { rpc: "auth.revokeSelf", reason: "self", credentialId: self.id },
      },
    ]);
  });

  test("local and console have nothing of their own to revoke", async () => {
    const { audit: a } = audit();
    for (const p of [LOCAL, CONSOLE]) {
      const e = await Effect.runPromise(Effect.flip(revokeSelf({ audit: a, revoke: () => true }, p, () => undefined, () => undefined)));
      expect(e.code).toBe("INVALID_REQUEST");
      expect(e.fix).not.toMatch(/`|\bfin(isherd)? [a-z]/);
    }
  });
});
