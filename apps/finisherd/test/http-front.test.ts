// 5B Task 6: the HTTP front (Decision 11) and its rate buckets (Decision 13), on loopback.
import { createServer, request, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { Audit, AUDIT_ADDRESSES_PER_MINUTE } from "../src/control/audit.ts";
import { openControlStore } from "../src/control/db.ts";
import { finError } from "../src/errors.ts";
import { type FrontRefusal, MAX_BODY_BYTES, makeFront, type Route } from "../src/http/front.ts";
import { Buckets } from "../src/http/limits.ts";
import { type ListenerPolicy, loopbackPolicy } from "../src/http/policy.ts";

const servers: Server[] = [];
afterEach(async () => {
  for (const s of servers.splice(0)) await new Promise((r) => s.close(r));
});

interface Got {
  status: number;
  headers: Record<string, string | string[] | undefined>;
  body: string;
}

async function serveFront(routes: Record<string, Route>, policy: (port: () => number) => ListenerPolicy, onRefused?: (r: FrontRefusal) => void) {
  let port = 0;
  const server = createServer(makeFront(routes, policy(() => port), { current: () => "nonce-now" }, { ...(onRefused === undefined ? {} : { onRefused }), remoteOf: () => "127.0.0.9" }));
  servers.push(server);
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  port = (server.address() as AddressInfo).port;
  const call = (method: string, path: string, headers: Record<string, string> = {}, body?: string | Buffer) =>
    new Promise<Got>((resolve, reject) => {
      const req = request({ host: "127.0.0.1", port, method, path, headers: { host: `127.0.0.1:${port}`, ...headers } }, (res) => {
        let text = "";
        res.setEncoding("utf8");
        res.on("data", (d: string) => (text += d));
        res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: text }));
      });
      req.on("error", reject);
      if (body !== undefined) req.write(body);
      req.end();
    });
  return { port, call };
}

const routes: Record<string, Route> = {
  "GET /.well-known/finisher": async () => ({ status: 200, body: { envId: "01envenvenvenvenvenvenvenv", protocol: { min: 2, max: 2 }, spki: "x" } }),
  "POST /pair/redeem": async (_req, body) => ({ status: 200, body: { echo: body } }),
  "POST /auth/token": async () => {
    throw finError("DPOP_NONCE", "a fresh nonce is needed", {});
  },
  "POST /auth/ws-ticket": async () => {
    throw new Error("a secret detail of what failed");
  },
};

const loop = (port: () => number) => loopbackPolicy(port, () => ["https://app.example.test"]);
const json = { "content-type": "application/json" };

describe("the HTTP front", () => {
  test("host_checked_first", async () => {
    const refused: FrontRefusal[] = [];
    const f = await serveFront(routes, loop, (r) => refused.push(r));
    // A rebinding page: another Host, whatever the path or origin.
    for (const [method, path] of [["GET", "/.well-known/finisher"], ["POST", "/pair/redeem"], ["GET", "/nowhere"], ["OPTIONS", "/pair/redeem"]] as const) {
      const got = await f.call(method, path, { host: "evil.example.test", origin: "https://evil.example.test", ...json }, "{}");
      expect(got.status, `${method} ${path}`).toBe(403);
      expect(JSON.parse(got.body).error.code).toBe("HOST_REFUSED");
      expect(got.headers["access-control-allow-origin"]).toBeUndefined();
    }
    // The three loopback names with the port pass.
    for (const host of [`127.0.0.1:${f.port}`, `localhost:${f.port}`, `[::1]:${f.port}`]) {
      expect((await f.call("GET", "/.well-known/finisher", { host })).status, host).toBe(200);
    }
    // Another port is another host.
    expect((await f.call("GET", "/.well-known/finisher", { host: `127.0.0.1:${f.port + 1}` })).status).toBe(403);
    // Refusals of pairing paths are reported with the remote address; the well-known path's are not.
    expect(refused.map((r) => [r.code, r.credentialKind, r.remote])).toEqual([
      ["HOST_REFUSED", "pairing", "127.0.0.9"],
      ["HOST_REFUSED", "pairing", "127.0.0.9"],
    ]);
  });

  test("origin_matrix_per_listener", async () => {
    const loopback = await serveFront(routes, loop);
    const remote = await serveFront(routes, () => ({
      name: "remote",
      hosts: () => ["studio.example.test:7311"],
      origins: () => ["https://studio.example.test:7311"],
      maxUnauthenticated: 16,
      perAddress: 4,
    }));
    const lo = (origin?: string) => loopback.call("GET", "/.well-known/finisher", origin === undefined ? {} : { origin });
    const re = (origin?: string) =>
      remote.call("GET", "/.well-known/finisher", { host: "studio.example.test:7311", ...(origin === undefined ? {} : { origin }) });
    expect((await lo()).status).toBe(200);
    expect((await lo(`http://127.0.0.1:${loopback.port}`)).status).toBe(200);
    expect((await lo(`http://localhost:${loopback.port}`)).status).toBe(200);
    expect((await lo("https://app.example.test")).status).toBe(200);
    expect((await lo("https://elsewhere.example.test")).status).toBe(403);
    expect((await lo("null")).status).toBe(403);
    // The remote listener: its own routes' origins only; loopback origins are not its own.
    expect((await re()).status).toBe(200);
    expect((await re("https://studio.example.test:7311")).status).toBe(200);
    expect((await re(`http://127.0.0.1:${loopback.port}`)).status).toBe(403);
    expect(JSON.parse((await re("https://app.example.test")).body).error.code).toBe("ORIGIN_REFUSED");
    // Its Host list is not the loopback names.
    expect((await remote.call("GET", "/.well-known/finisher")).status).toBe(403);
  });

  test("cors_only_for_allowed_origins", async () => {
    const f = await serveFront(routes, loop);
    const pre = await f.call("OPTIONS", "/pair/redeem", { origin: "https://app.example.test", "access-control-request-method": "POST" });
    expect(pre.status).toBe(204);
    expect(pre.headers).toMatchObject({
      "access-control-allow-origin": "https://app.example.test",
      "access-control-allow-methods": "POST",
      "access-control-allow-headers": "authorization, dpop, content-type",
      "access-control-max-age": "600",
    });
    const answer = await f.call("POST", "/pair/redeem", { origin: "https://app.example.test", ...json }, "{}");
    expect(answer.headers["access-control-allow-origin"]).toBe("https://app.example.test");
    expect(answer.headers["access-control-expose-headers"]).toBe("DPoP-Nonce");
    // Without an origin, or with a refused one, no CORS header at all; never credentials.
    const plain = await f.call("POST", "/pair/redeem", json, "{}");
    expect(plain.headers["access-control-allow-origin"]).toBeUndefined();
    expect((await f.call("OPTIONS", "/pair/redeem", { origin: "https://elsewhere.example.test" })).headers["access-control-allow-origin"]).toBeUndefined();
    for (const g of [pre, answer, plain]) expect(g.headers["access-control-allow-credentials"]).toBeUndefined();
    // A preflight of a path the front does not serve is 404.
    expect((await f.call("OPTIONS", "/nowhere", { origin: "https://app.example.test" })).status).toBe(404);
  });

  test("body_cap_and_content_type", async () => {
    const refused: FrontRefusal[] = [];
    const f = await serveFront(routes, loop, (r) => refused.push(r));
    expect(JSON.parse((await f.call("POST", "/pair/redeem", json, '{"a":1}')).body)).toEqual({ echo: { a: 1 } });
    expect(JSON.parse((await f.call("POST", "/pair/redeem", { "content-type": "application/json; charset=utf-8" }, "[1]")).body)).toEqual({ echo: [1] });
    expect((await f.call("POST", "/pair/redeem", { "content-type": "text/plain" }, "{}")).status).toBe(415);
    expect((await f.call("POST", "/pair/redeem", {}, "{}")).status).toBe(415);
    expect((await f.call("POST", "/pair/redeem", json, "not json")).status).toBe(400);
    const big = await f.call("POST", "/pair/redeem", json, Buffer.alloc(MAX_BODY_BYTES + 1, 0x20));
    expect(big.status).toBe(413);
    expect(JSON.parse(big.body).error.details.reason).toBe("bodyTooLarge");
    expect((await f.call("POST", "/pair/redeem", json, `"${"x".repeat(MAX_BODY_BYTES - 2)}"`)).status).toBe(200);
    expect(refused.map((r) => r.reason)).toEqual(["contentType", "contentType", "bodyTooLarge"]);
  });

  test("answers_close_and_no_store", async () => {
    const f = await serveFront(routes, loop);
    for (const g of [await f.call("GET", "/.well-known/finisher"), await f.call("POST", "/auth/token", json, "{}"), await f.call("GET", "/nowhere")]) {
      expect(g.headers).toMatchObject({ connection: "close", "cache-control": "no-store", "x-content-type-options": "nosniff", "dpop-nonce": "nonce-now" });
      expect(g.headers["set-cookie"]).toBeUndefined();
    }
    // Errors are `{error}` with Decision 33's status; an internal one says nothing of itself.
    const nonce = await f.call("POST", "/auth/token", json, "{}");
    expect(nonce.status).toBe(401);
    expect(JSON.parse(nonce.body).error.code).toBe("DPOP_NONCE");
    const internal = await f.call("POST", "/auth/ws-ticket", json, "{}");
    expect(internal.status).toBe(500);
    expect(internal.body).not.toContain("secret detail");
  });

  test("unknown_path_404_rpc_426", async () => {
    const f = await serveFront(routes, loop);
    expect((await f.call("GET", "/nowhere")).status).toBe(404);
    expect((await f.call("GET", "/pair/redeem")).status).toBe(404);
    const rpc = await f.call("GET", "/rpc");
    expect(rpc.status).toBe(426);
    expect(JSON.parse(rpc.body)).toEqual({ protocol: { min: 2, max: 2 } });
  });

  test("refusals_audited_with_remote (aggregated per kind and address)", async () => {
    const tmp = mkdtempSync(join(tmpdir(), "finisherd-front-"));
    try {
      const store = openControlStore(tmp, { serverId: "sid-1", checkpoint: false });
      const audit = new Audit(store);
      const f = await serveFront(routes, loop, (r) =>
        audit.append({ kind: "auth.fail", principal: { kind: "anonymous", id: "" }, credentialKind: r.credentialKind, outcome: "failed", code: r.code, details: { reason: r.reason, address: r.remote } }),
      );
      for (let i = 0; i < 3; i++) await f.call("POST", "/pair/redeem", { host: "evil.example.test" }, "{}");
      await f.call("POST", "/auth/token", { host: "evil.example.test" }, "{}");
      const rows = store.db.prepare("SELECT credentialKind, count, details FROM audit WHERE kind = 'auth.fail' ORDER BY id").all() as Array<{ credentialKind: string; count: number; details: string }>;
      expect(rows.map((r) => [r.credentialKind, r.count, JSON.parse(r.details).address])).toEqual([
        ["pairing", 3, "127.0.0.9"],
        ["dpop", 1, "127.0.0.9"],
      ]);
      // A host cycling addresses gets 64 rows a minute per kind; the rest fold into one without an address.
      for (let i = 0; i < 70; i++) {
        audit.append({ kind: "auth.fail", principal: { kind: "anonymous", id: "" }, credentialKind: "ticket", outcome: "failed", code: "HOST_REFUSED", details: { reason: "host", address: `127.1.0.${i}` } });
      }
      const tickets = store.db.prepare("SELECT count, details FROM audit WHERE kind = 'auth.fail' AND credentialKind = 'ticket'").all() as Array<{ count: number; details: string }>;
      expect(tickets.filter((r) => JSON.parse(r.details).address !== undefined)).toHaveLength(AUDIT_ADDRESSES_PER_MINUTE);
      expect(tickets.filter((r) => JSON.parse(r.details).address === undefined).map((r) => r.count)).toEqual([70 - AUDIT_ADDRESSES_PER_MINUTE]);
      await store.close();
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe("the front on a listener (review of Task 6)", () => {
  test("the loopback listener serves the front, and an unfinished request holds a pre-upgrade slot", { timeout: 20_000 }, async () => {
    const { Effect, Scope } = await import("effect");
    const { makeTicketedWebSocketServer } = await import("../src/upgrade.ts");
    const { connect } = await import("node:net");
    let port = 0;
    const policy = { ...loopbackPolicy(() => port, () => []), maxUnauthenticated: 1 };
    const scope = Effect.runSync(Scope.make());
    const bound = await Effect.runPromise(
      Scope.provide(makeTicketedWebSocketServer({ check: (() => ({ ok: false, status: 401, reason: "noTicket" })) as never, maxPayload: 1024, policy, onRequest: makeFront(routes, policy, undefined) }), scope),
    );
    port = bound.port;
    try {
      const get = () =>
        new Promise<number>((resolve) => {
          const req = request({ host: "127.0.0.1", port, path: "/.well-known/finisher", headers: { host: `127.0.0.1:${port}` } }, (res) => {
            res.resume();
            res.on("end", () => resolve(res.statusCode ?? 0));
          });
          req.on("error", () => resolve(-1));
          req.end();
        });
      expect(await get()).toBe(200);
      // A request whose body never comes keeps its socket counted: the pool of one is full.
      const slow = connect({ host: "127.0.0.1", port }, () => slow.write(`POST /pair/redeem HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{`));
      slow.on("error", () => undefined);
      await new Promise((r) => setTimeout(r, 150));
      expect(await get()).toBe(-1);
      slow.destroy();
      await new Promise((r) => setTimeout(r, 150));
      expect(await get()).toBe(200);
    } finally {
      await Effect.runPromise(Scope.close(scope, Effect.void as never));
    }
  });
});

describe("rate buckets (Decision 13)", () => {
  test("rate_buckets_bounded", () => {
    let now = 0;
    const b = new Buckets({ perMinute: 10, burst: 10 }, 4, () => now);
    for (let i = 0; i < 10; i++) expect(b.take("a")).toBe(true);
    expect(b.take("a")).toBe(false);
    expect(b.retryAfterS("a")).toBe(6);
    now += 6_000;
    expect(b.take("a")).toBe(true);
    expect(b.take("a")).toBe(false);
    // At most `max` buckets: the least recently used goes first.
    for (const k of ["b", "c", "d"]) b.take(k);
    expect(b.size).toBe(4);
    b.take("e");
    expect(b.size).toBe(4);
    // "a" was used before b, c, d and e: it was dropped, so it starts full again.
    for (let i = 0; i < 10; i++) expect(b.take("a"), `${i}`).toBe(true);
  });
});
