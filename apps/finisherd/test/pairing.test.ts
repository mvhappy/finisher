// 5B Task 8: pairing by link or QR code (Decisions 17–21): the state machine, one-time offers,
// safety codes, caps, the pending stream, the audit and the journal on a `Pairing` with a real
// control store; the HTTP endpoints (rate limits, the status proof) on the front; the RPCs'
// scopes on a server in this process.
import { createHmac } from "node:crypto";
import { createServer, request, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, Principal, Route } from "@finisher/contracts";
import { SCOPES } from "@finisher/contracts";
import { Effect, Fiber, Stream } from "effect";
import { calculateJwkThumbprint, exportJWK, generateKeyPair, SignJWT } from "jose";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { DeviceStore } from "../src/auth/devices.ts";
import { DpopVerifier } from "../src/auth/dpop.ts";
import { Nonces } from "../src/auth/nonces.ts";
import { pairRoutes } from "../src/auth/pair-http.ts";
import { linkRoutes, MAX_LINK_ROUTES, Pairing, type RedeemCtx, safetyCode } from "../src/auth/pairing.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import type { Environment } from "../src/env/descriptor.ts";
import { makeFront } from "../src/http/front.ts";
import { loopbackPolicy } from "../src/http/policy.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { confirmed, connectProved, ok, type ServerInfo } from "./support/ndjson-client.ts";

const CONSOLE: Principal = { kind: "console", id: "console", scopes: [...SCOPES], projects: "*", branches: "*", human: true };
const DEVICE: Principal = { kind: "device", id: "01device00000000000000000a", scopes: [...SCOPES], projects: "*", branches: "*", human: true };
const MIN = 60_000;

let tmp: string;
let store: ControlStore;
let now: number;
const clock = () => now;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-pairing-"));
  mkdirSync(join(tmp, "sd"), { mode: 0o700 });
  now = 1_800_000_000_000;
  store = openControlStore(join(tmp, "sd"), { serverId: "sid-1", checkpoint: false, clock });
});
afterEach(async () => {
  await store.close();
  rmSync(tmp, { recursive: true, force: true });
});

const ROUTES: Route[] = [
  { kind: "loopback", url: "http://127.0.0.1:4100", pin: "identity" },
  { kind: "lan", url: "https://192.0.2.40:7311", pin: "identity" },
  { kind: "lan", url: "https://[2001:db8::5]:7311", pin: "identity" },
  { kind: "tailscale", url: "https://100.64.0.7:7311", pin: "identity" },
  { kind: "lan", url: "https://198.51.100.9:7311", pin: "identity" },
  { kind: "public", url: "https://studio.example.test", pin: "webpki" },
];

function env(state: "ok" | "moved" = "ok", routes: Route[] = ROUTES): Environment {
  return {
    state: () => state,
    describe: () => ({ envId: "01envenvenvenvenvenvenvenv", name: "Finisher", spki: "A".repeat(43), serverIds: [], protocol: { min: 2, max: 2 }, routes, capabilities: [], state }),
  } as unknown as Environment;
}

function setup(e = env()) {
  const audit = new Audit(store, clock);
  const devices = new DeviceStore(store, audit, clock);
  const journal: Array<{ kind: string; payload: Record<string, unknown> }> = [];
  const pairing = new Pairing({
    store,
    devices,
    audit,
    env: e,
    clock,
    journal: { enabled: true, record: (kind, _kv, _ctx, payload) => void journal.push({ kind, payload: JSON.parse(payload) }) },
  });
  return { audit, devices, journal, pairing };
}

async function deviceKey() {
  const { privateKey, publicKey } = await generateKeyPair("ES256", { extractable: true });
  const jwk = await exportJWK(publicKey);
  return { privateKey, jwk: { kty: "EC" as const, crv: "P-256" as const, x: jwk.x!, y: jwk.y! }, jkt: await calculateJwkThumbprint(jwk, "sha256") };
}

/** The offer id and secret of a link's fragment. */
const fragmentOf = (link: string) => {
  const [offerId, secret] = link.slice(link.indexOf("#") + 1).split(".");
  return { offerId: offerId!, secret: secret! };
};

const codeOf = async (p: Promise<unknown> | (() => unknown)): Promise<string | undefined> => {
  try {
    await (typeof p === "function" ? p() : p);
  } catch (e) {
    return (e as FinError).code;
  }
  return undefined;
};

const ctx: RedeemCtx = { remote: "192.0.2.77", listener: "remote", route: "lan" };

async function redeemNew(pairing: Pairing, link: string, name = "Studio tablet", c = ctx) {
  const k = await deviceKey();
  const r = await pairing.redeem({ ...fragmentOf(link), device: { name, platform: "ios", jwk: k.jwk } }, c);
  return { ...r, key: k };
}

const auditRows = () =>
  (store.db.prepare("SELECT kind, details FROM audit WHERE kind LIKE 'pair.%' OR kind = 'auth.fail' ORDER BY id").all() as Array<{ kind: string; details: string }>).map((r) => ({
    kind: r.kind,
    details: JSON.parse(r.details) as Record<string, unknown>,
  }));

describe("pairing", () => {
  test("offer → redeem → approve, with the safety code both sides compute", async () => {
    const { pairing, devices } = setup();
    const offer = pairing.offer(CONSOLE, { label: "the tablet" });
    expect(offer.link).toMatch(/^finisher:\/\/pair\?v=1&env=01envenvenvenvenvenvenvenv&spki=A{43}&r=[A-Za-z0-9_-]+#[0-9A-Za-z]{26}\.[A-Za-z0-9_-]{43}$/);
    expect(offer.qr).toContain("<svg");
    expect(offer.browserLink).toMatch(/^https:\/\/studio\.example\.test\/pair#/);
    const { pendingId, safetyCode: code, key } = await redeemNew(pairing, offer.link);
    expect(code).toBe(safetyCode(Buffer.from(fragmentOf(offer.link).secret, "base64url"), key.jkt));
    expect(pairing.status(pendingId, key.jkt)).toEqual({ state: "pending" });
    const device = pairing.approve(CONSOLE, pendingId);
    expect(device).toMatchObject({ name: "Studio tablet", platform: "ios", jkt: key.jkt, approvedBy: "console:console" });
    expect(devices.byJkt(key.jkt)?.id).toBe(device.id);
    expect(pairing.status(pendingId, key.jkt)).toEqual({ state: "approved", deviceId: device.id });
    expect(await codeOf(() => pairing.approve(CONSOLE, pendingId))).toBe("INVALID_REQUEST");
  });

  test("reject, and a request that waits 10 minutes expires; so does an unused offer", async () => {
    const { pairing } = setup();
    const a = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    pairing.reject(CONSOLE, a.pendingId);
    expect(await codeOf(() => pairing.status(a.pendingId, a.key.jkt))).toBe("PAIRING_REJECTED");
    const b = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    now += 10 * MIN;
    expect(pairing.status(b.pendingId, b.key.jkt)).toEqual({ state: "expired" });
    expect(await codeOf(() => pairing.approve(CONSOLE, b.pendingId))).toBe("PAIRING_EXPIRED");
    const offer = pairing.offer(CONSOLE, { ttlS: 60 });
    now += MIN;
    expect(await codeOf(redeemNew(pairing, offer.link))).toBe("PAIRING_EXPIRED");
    expect(await codeOf(() => pairing.offer(CONSOLE, { ttlS: 601 }))).toBe("INVALID_REQUEST");
  });

  test("first_valid_redeem_consumes: a second redeem is PAIRING_USED", async () => {
    const { pairing } = setup();
    const offer = pairing.offer(CONSOLE, {});
    await redeemNew(pairing, offer.link, "Attacker phone");
    expect(await codeOf(redeemNew(pairing, offer.link, "The real tablet"))).toBe("PAIRING_USED");
  });

  test("two concurrent redeems of one offer: one gets in, the other is PAIRING_USED (review of Task 8)", async () => {
    const { pairing } = setup();
    const offer = pairing.offer(CONSOLE, {});
    const results = await Promise.allSettled([redeemNew(pairing, offer.link, "One"), redeemNew(pairing, offer.link, "Two")]);
    expect(results.filter((r) => r.status === "fulfilled")).toHaveLength(1);
    const refused = results.find((r) => r.status === "rejected") as PromiseRejectedResult;
    expect((refused.reason as FinError).code).toBe("PAIRING_USED");
    expect(store.db.prepare("SELECT COUNT(*) AS n FROM pair_pending").get()).toEqual({ n: 1 });
  });

  test("an approved device learns its id after a restart (review of Task 8)", async () => {
    const { pairing } = setup();
    const { pendingId, key } = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    const device = pairing.approve(CONSOLE, pendingId);
    const after = setup().pairing;
    expect(after.isPendingKey(key.jkt)).toBe(true);
    expect(after.status(pendingId, key.jkt)).toEqual({ state: "approved", deviceId: device.id });
    expect(await codeOf(() => after.status(pendingId, "another-jkt"))).toBe("NOT_FOUND");
  });

  test("an approval whose transaction rolls back leaves no device in memory (review of Task 9)", async () => {
    const { pairing, devices } = setup();
    const { pendingId, key } = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    // The request's state cannot be written: the transaction rolls back after the device's row.
    store.db.exec("CREATE TRIGGER no_answer BEFORE UPDATE ON pair_pending BEGIN SELECT RAISE(ABORT, 'refused'); END;");
    expect(await codeOf(() => pairing.approve(CONSOLE, pendingId))).toBeDefined();
    expect(devices.byJkt(key.jkt)).toBeUndefined();
    expect(store.db.prepare("SELECT COUNT(*) AS n FROM devices").get()).toEqual({ n: 0 });
    store.db.exec("DROP TRIGGER no_answer");
    expect(pairing.approve(CONSOLE, pendingId).jkt).toBe(key.jkt);
  });

  test("bad_secret_does_not_consume", async () => {
    const { pairing } = setup();
    const offer = pairing.offer(CONSOLE, {});
    const k = await deviceKey();
    const { offerId } = fragmentOf(offer.link);
    for (const secret of [Buffer.alloc(32, 7).toString("base64url"), "short", ""]) {
      expect(await codeOf(pairing.redeem({ offerId, secret, device: { name: "x", platform: "ios", jwk: k.jwk } }, ctx))).toBe("PAIRING_EXPIRED");
    }
    // An unknown offer reads the same.
    expect(await codeOf(pairing.redeem({ offerId: "01nope00000000000000000000", secret: fragmentOf(offer.link).secret, device: { name: "x", platform: "ios", jwk: k.jwk } }, ctx))).toBe("PAIRING_EXPIRED");
    expect((await redeemNew(pairing, offer.link)).pendingId).toMatch(/^[0-9A-Za-z]{26}$/);
  });

  test("safety_code_vector", () => {
    const secret = Buffer.from("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff", "hex");
    const jkt = "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I";
    const mac = createHmac("sha256", secret).update(jkt).digest();
    expect(safetyCode(secret, jkt)).toBe(String(mac.readUInt32BE(0) % 1_000_000).padStart(6, "0"));
    expect(safetyCode(secret, jkt)).toMatch(/^\d{6}$/);
    // Another key, another code.
    expect(safetyCode(secret, `${jkt.slice(0, -1)}J`)).not.toBe(safetyCode(secret, jkt));
  });

  test("self_offer_loopback_only_and_approved_at_redeem; self_offer_from_device_refused", async () => {
    const { pairing } = setup();
    expect(await codeOf(() => pairing.offer(DEVICE, { self: true }))).toBe("INVALID_REQUEST");
    const remote = pairing.offer(CONSOLE, { self: true });
    // Its link names the loopback route only.
    const r = JSON.parse(Buffer.from(new URL(remote.link.replace("finisher://", "https://x/")).searchParams.get("r")!, "base64url").toString());
    expect(r).toEqual([ROUTES[0]]);
    expect(await codeOf(redeemNew(pairing, remote.link))).toBe("PAIRING_EXPIRED");
    const local = pairing.offer(CONSOLE, { self: true });
    const got = await redeemNew(pairing, local.link, "Finisher app", { remote: "127.0.0.1", listener: "loopback", route: "loopback" });
    expect(pairing.status(got.pendingId, got.key.jkt).state).toBe("approved");
  });

  test("open_offer_and_pending_caps", async () => {
    const { pairing } = setup();
    const offers = Array.from({ length: 9 }, () => pairing.offer(CONSOLE, {}));
    expect(pairing.openOffers()).toBe(8);
    // The oldest went first.
    expect(await codeOf(redeemNew(pairing, offers[0]!.link))).toBe("PAIRING_EXPIRED");
    for (const o of offers.slice(1)) await redeemNew(pairing, o.link);
    for (let i = 0; i < 8; i++) await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    expect(await codeOf(redeemNew(pairing, pairing.offer(CONSOLE, {}).link))).toBe("PAIRING_RATE_LIMITED");
  });

  test("pending_stream_snapshot_first_without_address", async () => {
    const { pairing } = setup();
    const first = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    const fiber = Effect.runFork(Stream.runCollect(Stream.take(pairing.pending(), 3)));
    await new Promise((r) => setTimeout(r, 20));
    const second = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link, "Edit bay");
    pairing.approve(CONSOLE, first.pendingId);
    const items = Array.from((await Effect.runPromise(Fiber.join(fiber))) as Iterable<Record<string, unknown>>);
    expect(items.map((i) => i.kind)).toEqual(["snapshot", "added", "ended"]);
    expect((items[0]!.pending as Array<{ pendingId: string }>).map((p) => p.pendingId)).toEqual([first.pendingId]);
    expect(items[1]).toMatchObject({ pending: { pendingId: second.pendingId, deviceName: "Edit bay", platform: "ios", safetyCode: second.safetyCode } });
    expect(items[2]).toEqual({ kind: "ended", pendingId: first.pendingId, state: "approved" });
    expect(JSON.stringify(items)).not.toContain(ctx.remote);
  });

  test("audit_rows_hold_name_and_remote_only_where_allowed; auth_pair_journal_meta_only", async () => {
    const { pairing, journal } = setup();
    const offer = pairing.offer(CONSOLE, { label: "for the tablet" });
    const { pendingId } = await redeemNew(pairing, offer.link);
    now += 1500;
    pairing.approve(CONSOLE, pendingId);
    const rows = auditRows();
    expect(rows.map((r) => r.kind)).toEqual(["pair.offer", "pair.redeem", "pair.approve"]);
    expect(rows[0]!.details).toEqual({ label: "for the tablet" });
    expect(rows[1]!.details).toEqual({ deviceName: "Studio tablet", platform: "ios", address: "192.0.2.77" });
    expect(rows[2]!.details).toMatchObject({ deviceName: "Studio tablet", address: "192.0.2.77" });
    expect(journal).toEqual([
      { kind: "auth.pair", payload: { outcome: "offered" } },
      { kind: "auth.pair", payload: { outcome: "redeemed", platform: "ios", route: "lan" } },
      { kind: "auth.pair", payload: { outcome: "approved", platform: "ios", route: "lan", micros: 1_500_000 } },
    ]);
    // No secret or link in the store, the audit or the journal; the address in the audit only.
    const { secret } = fragmentOf(offer.link);
    const stored = JSON.stringify([store.db.prepare("SELECT * FROM pair_offers").all(), store.db.prepare("SELECT * FROM pair_pending").all()]);
    for (const text of [stored, JSON.stringify(rows), JSON.stringify(journal)]) {
      expect(text).not.toContain(secret);
      expect(text).not.toContain(offer.link);
    }
    expect(stored).not.toContain("192.0.2.77");
    expect(JSON.stringify(journal)).not.toContain("192.0.2.77");
    const pendingRow = store.db.prepare("SELECT safetyCode, remote FROM pair_pending").get();
    expect(pendingRow).toEqual({ safetyCode: null, remote: null });
  });

  test("moved server data and a restart", async () => {
    expect(await codeOf(() => setup(env("moved")).pairing.offer(CONSOLE, {}))).toBe("ENV_MOVED");
    const { pairing } = setup();
    const offer = pairing.offer(CONSOLE, {});
    const waiting = await redeemNew(pairing, pairing.offer(CONSOLE, {}).link);
    // After a restart an open offer still works; a waiting request lost its code and expired.
    const again = setup().pairing;
    expect((await redeemNew(again, offer.link)).safetyCode).toMatch(/^\d{6}$/);
    expect(await codeOf(() => again.approve(CONSOLE, waiting.pendingId))).toBe("NOT_FOUND");
  });

  test("the link carries few routes, so the QR code stays small", () => {
    const capped = linkRoutes(ROUTES);
    expect(capped.map((r) => r.kind)).toEqual(["public", "tailscale", "lan"]);
    expect(capped).toHaveLength(MAX_LINK_ROUTES);
    // With the remote listener off, the loopback route alone (an SSH tunnel, the local app).
    expect(linkRoutes([ROUTES[0]!])).toEqual([ROUTES[0]]);
  });
});

describe("the pairing endpoints", () => {
  const servers: Server[] = [];
  afterEach(async () => {
    for (const s of servers.splice(0)) await new Promise((r) => s.close(r));
  });

  async function front(pairing: Pairing, audit: Audit) {
    let port = 0;
    const nonces = new Nonces(clock);
    const routes = pairRoutes({
      pairing,
      verifier: new DpopVerifier(nonces, clock),
      env: env(),
      audit,
      allowedOrigins: () => [`http://127.0.0.1:${port}`],
      routeOf: () => "loopback",
      clock,
    });
    let remote = "127.0.0.1";
    const server = createServer(makeFront(routes, loopbackPolicy(() => port, () => []), nonces, { remoteOf: () => remote }));
    servers.push(server);
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    port = (server.address() as AddressInfo).port;
    const post = (path: string, body: unknown, headers: Record<string, string> = {}) =>
      new Promise<{ status: number; body: Record<string, unknown>; headers: Record<string, unknown> }>((resolve, reject) => {
        const req = request({ host: "127.0.0.1", port, method: "POST", path, headers: { host: `127.0.0.1:${port}`, "content-type": "application/json", ...headers } }, (res) => {
          let t = "";
          res.on("data", (d) => (t += d));
          res.on("end", () => resolve({ status: res.statusCode ?? 0, body: t ? JSON.parse(t) : {}, headers: res.headers }));
        });
        req.on("error", reject);
        req.end(JSON.stringify(body));
      });
    return { post, port: () => port, setRemote: (r: string) => (remote = r), nonces };
  }

  test("redeem_rate_limits: 10 a minute per address, 30 per server", async () => {
    const { pairing, audit } = setup();
    const f = await front(pairing, audit);
    const bad = { offerId: "01nope00000000000000000000", secret: "x", device: { name: "x", platform: "ios", jwk: (await deviceKey()).jwk } };
    for (let i = 0; i < 10; i++) expect((await f.post("/pair/redeem", bad)).status).toBe(410);
    const limited = await f.post("/pair/redeem", bad);
    expect(limited.status).toBe(429);
    expect(limited.headers["retry-after"]).toBeDefined();
    // Other addresses get in until the server's 30 (a refused address takes no server token).
    for (let i = 0; i < 20; i++) {
      f.setRemote(`127.0.1.${i}`);
      expect((await f.post("/pair/redeem", bad)).status, `${i}`).toBe(410);
    }
    f.setRemote("127.0.2.1");
    expect((await f.post("/pair/redeem", bad)).status).toBe(429);
    now += MIN;
    expect((await f.post("/pair/redeem", bad)).status).toBe(410);
    expect(auditRows().filter((r) => r.kind === "auth.fail").length).toBeGreaterThan(0);
  });

  test("status_needs_pending_key: a DPoP proof by the request's own key", async () => {
    const { pairing, audit } = setup();
    const f = await front(pairing, audit);
    const offer = pairing.offer(CONSOLE, {});
    const k = await deviceKey();
    const redeemed = await f.post("/pair/redeem", { ...fragmentOf(offer.link), device: { name: "Tablet", platform: "android", jwk: k.jwk } });
    expect(redeemed.status).toBe(200);
    const { pendingId } = redeemed.body as { pendingId: string };
    const prove = async (key: Awaited<ReturnType<typeof deviceKey>>, nonce?: string) =>
      new SignJWT({ jti: `j${Math.random()}`, htm: "POST", htu: `http://127.0.0.1:${f.port()}/pair/status`, iat: Math.floor(now / 1000), ...(nonce === undefined ? {} : { nonce }) })
        .setProtectedHeader({ typ: "dpop+jwt", alg: "ES256", jwk: key.jwk })
        .sign(key.privateKey);
    // No nonce: told to use the one the server sent.
    const first = await f.post("/pair/status", { pendingId }, { dpop: await prove(k) });
    expect(first.status).toBe(401);
    expect(String(first.headers["www-authenticate"])).toContain("use_dpop_nonce");
    const nonce = String(first.headers["dpop-nonce"]);
    expect((await f.post("/pair/status", { pendingId }, { dpop: await prove(k, nonce) })).body).toEqual({ state: "pending" });
    // Another key is refused before anything is remembered.
    const other = await f.post("/pair/status", { pendingId }, { dpop: await prove(await deviceKey(), nonce) });
    expect(other.status).toBe(401);
    expect((other.body.error as { details: { reason: string } }).details.reason).toBe("unknownKey");
    pairing.approve(CONSOLE, pendingId);
    const approved = await f.post("/pair/status", { pendingId }, { dpop: await prove(k, nonce) });
    expect(approved.body).toMatchObject({ state: "approved", environment: { envId: "01envenvenvenvenvenvenvenv" } });
  });
});

describe("the RPCs", () => {
  test("approval_is_human_only_and_confirmed (the console's code; local is refused)", async () => {
    const sd = join(tmp, "server");
    mkdirSync(sd, { mode: 0o700 });
    let ready!: (i: ServerInfo) => void;
    const info = new Promise<ServerInfo>((r) => (ready = r));
    const fiber = Effect.runFork(serve({ serverData: sd, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
    try {
      const server = { info: await info, serverData: sd };
      const local = await connectProved(server, "local");
      const c = await connectProved(server, "console");
      const code = (e: { ok: boolean; error?: { code: string } }) => (e.ok ? undefined : e.error!.code);
      expect(code(await local.call("auth.pair.offer", {}))).toBe("HUMAN_ONLY");
      expect(code(await c.call("auth.pair.offer", {}))).toBe("CONFIRMATION_REQUIRED");
      const offer = ok<{ link: string; qr: string }>(await confirmed(c, "auth.pair.offer", { label: "tablet" }));
      expect(offer.link).toMatch(/^finisher:\/\/pair\?v=1&env=/);
      // A self offer needs no code from the console.
      expect(code(await c.call("auth.pair.offer", { self: true }))).toBeUndefined();
      expect(code(await c.call("auth.pair.approve", { pendingId: "01nope00000000000000000000" }))).toBe("CONFIRMATION_REQUIRED");
      expect(code(await confirmed(c, "auth.pair.approve", { pendingId: "01nope00000000000000000000" }))).toBe("NOT_FOUND");
      expect(code(await local.call("auth.pair.reject", { pendingId: "x" }))).toBe("HUMAN_ONLY");
      await local.close();
      await c.close();
    } finally {
      await Effect.runPromise(Fiber.interrupt(fiber));
    }
  });
});
