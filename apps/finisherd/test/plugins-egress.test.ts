// The egress broker (sp8b plan Task 8, Decisions 10 and 11; spec §9.3, §15.1 "Egress"): HTTPS
// only, to approved origins, every resolved address public and connected to as checked, redirect
// rules, caps, the per-plugin rate and no secret injection. Built against a loopback HTTPS
// server with a certificate generated for this run (no key material in the tree) that the
// broker reaches by name through the test-only resolver hook (`egress-test-hooks.ts`).
import { once } from "node:events";
import { createServer, type Server } from "node:https";
import type { AddressInfo } from "node:net";
import type { TLSSocket } from "node:tls";
import { type FinError, type JobView, SCOPES } from "@finisher/contracts";
import type { NetFetch, NetFetchReply, Permission } from "@finisher/core-types";
import { generateIdentityKey, selfSignedCert } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import type { CallCtx } from "../src/jobs/kinds.ts";
import { makeHostCalls } from "../src/plugins/host/calls.ts";
import { ipv6Groups, privateReason } from "../src/plugins/host/egress-address.ts";
import { BODY_BYTES, READ_BYTES } from "../src/plugins/host/egress-bodies.ts";
import { type EgressTestHooks, makeEgress } from "../src/plugins/host/egress.ts";
import { egressTestHooks } from "../src/plugins/host/egress-test-hooks.ts";
import { makePluginLog } from "../src/plugins/host/log.ts";
import { rightsFor } from "../src/plugins/host/rights.ts";
import { LOCAL } from "../src/principal.ts";

const API = "api.egress.test";
const OTHER = "other.egress.test";
const PLUGIN = "@test/egress@1.0.0";

interface Seen {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string | string[] | undefined>;
  readonly servername: string | false | null | undefined;
  readonly body: Buffer;
}

let server: Server;
let port = 0;
let ca = "";
const seen: Seen[] = [];
let origin = "";
let other = "";

beforeAll(async () => {
  const id = generateIdentityKey();
  ca = selfSignedCert(id.pem, [API, OTHER], 1);
  server = createServer({ key: id.pem, cert: ca }, (req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      const url = new URL(req.url ?? "/", "https://x");
      seen.push({ method: req.method ?? "", path: url.pathname, headers: req.headers, servername: (req.socket as TLSSocket).servername, body: Buffer.concat(chunks) });
      const p = url.pathname;
      if (p === "/hello") return res.writeHead(200, { "content-type": "text/plain", "x-echo": "1" }).end("hello");
      const r = /^\/redirect\/(\d+)$/.exec(p);
      if (r !== null) {
        const n = Number(r[1]);
        if (n === 0) return res.writeHead(200).end("landed");
        return res.writeHead(302, { location: `/redirect/${n - 1}` }).end();
      }
      if (p === "/cross") return res.writeHead(302, { location: `https://${OTHER}:${port}/x` }).end("moved");
      if (p === "/see-other") return res.writeHead(303, { location: "/hello" }).end();
      if (p === "/to-http") return res.writeHead(301, { location: `http://${API}:${port}/hello` }).end();
      if (p === "/big") {
        const n = Number(url.searchParams.get("n"));
        res.writeHead(200, { "content-length": String(n) });
        const chunk = Buffer.alloc(1 << 20, 0x61);
        let left = n;
        const pump = (): void => {
          while (left > 0) {
            const k = Math.min(left, chunk.length);
            left -= k;
            if (!res.write(chunk.subarray(0, k))) return void res.once("drain", pump);
          }
          res.end();
        };
        return pump();
      }
      if (p === "/chunked-big") {
        // No content-length: the cap holds on the bytes read.
        res.writeHead(200);
        res.write(Buffer.alloc(600, 0x62));
        return res.end(Buffer.alloc(600, 0x62));
      }
      if (p === "/slow") return; // never answers
      if (p === "/trickle") {
        // 1000 bytes, then nothing: the bytes are in flight until the call ends.
        res.writeHead(200);
        return void res.write(Buffer.alloc(1000, 0x63));
      }
      res.writeHead(404).end();
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  port = (server.address() as AddressInfo).port;
  origin = `https://${API}:${port}`;
  other = `https://${OTHER}:${port}`;
});

afterAll(async () => {
  server.closeAllConnections();
  server.close();
});

/** Hooks: both names resolve to the loopback server; that pair alone passes the address check. */
function hooks(extra: Partial<Omit<EgressTestHooks, "testOnly">> = {}): EgressTestHooks {
  return egressTestHooks({
    lookup: async (h) => (h === API || h === OTHER ? ["127.0.0.1"] : []),
    allow: [
      { host: API, address: "127.0.0.1" },
      { host: OTHER, address: "127.0.0.1" },
    ],
    ca,
    ...extra,
  });
}

const get = (url: string, more: Partial<NetFetch["request"]> = {}, grant?: string): NetFetch => ({ request: { method: "GET", url, ...more }, ...(grant === undefined ? {} : { grant }) });

async function failure(p: Promise<unknown>): Promise<FinError> {
  try {
    await p;
  } catch (e) {
    return e as FinError;
  }
  throw new Error("expected a failure");
}

const call = (origins: string[], jobId = "j1", plugin = PLUGIN) => ({ jobId, plugin, origins });

/** The whole body behind `reply.handle`, read page by page. */
function readAll(e: ReturnType<typeof makeEgress>, jobId: string, reply: NetFetchReply): Buffer {
  const parts: Buffer[] = [];
  for (let offset = 0; ; ) {
    const page = e.read(jobId, { handle: reply.handle, offset, length: READ_BYTES });
    const b = Buffer.from(page.bytesB64, "base64");
    parts.push(b);
    offset += b.length;
    if (page.eof) break;
  }
  return Buffer.concat(parts);
}

describe("egress broker", () => {
  test("happy path: SNI and Host from the origin, the body held for net.read", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    const r = await e.fetch(call([origin]), get(`${origin}/hello?q=1`, { headers: [{ name: "X-Plugin", value: "yes" }] }));
    expect(r).toMatchObject({ status: 200, length: 5 });
    expect(r.headers).toContainEqual({ name: "x-echo", value: "1" });
    expect(readAll(e, "j1", r).toString()).toBe("hello");
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ method: "GET", path: "/hello", servername: API, headers: { host: `${API}:${port}`, "x-plugin": "yes" } });
    expect(e.stats().get("@test/egress")).toMatchObject({ requests: 1, completed: 1, bytesIn: 5 });
  });

  test("undeclared_origin_denied: scheme, host and port are exact", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    for (const url of [`${other}/hello`, `https://${API}:${port + 1}/hello`, `https://${API}/hello`, `https://sub.${API}:${port}/`]) {
      const err = await failure(e.fetch(call([origin]), get(url)));
      expect([err.code, (err.details as { origin: string }).origin]).toEqual(["EGRESS_DENIED", new URL(url).origin]);
    }
    // No origins approved at all, or malformed ones: nothing is reachable.
    expect((await failure(e.fetch(call([]), get(`${origin}/hello`)))).code).toBe("EGRESS_DENIED");
    expect((await failure(e.fetch(call([`${origin}/path`, "*.egress.test", `http://${API}:${port}`]), get(`${origin}/hello`)))).code).toBe("EGRESS_DENIED");
    expect(seen).toHaveLength(0);
    expect(e.stats().get("@test/egress")?.denied).toBe(6);
  });

  test("http_and_ws_refused", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    for (const url of [`http://${API}:${port}/hello`, `ws://${API}:${port}/`, `wss://${API}:${port}/`, `ftp://${API}/x`, "file:///etc/passwd", "data:text/plain,x"]) {
      expect((await failure(e.fetch(call([origin, `http://${API}:${port}`]), get(url)))).code).toBe("EGRESS_NOT_HTTPS");
    }
    // A WebSocket upgrade over https is refused as a header.
    const up = await failure(e.fetch(call([origin]), get(`${origin}/hello`, { headers: [{ name: "Upgrade", value: "websocket" }, { name: "Connection", value: "Upgrade" }] })));
    expect([up.code, (up.details as { reason: string }).reason]).toEqual(["INVALID_REQUEST", "header"]);
    // A same-origin redirect to http is not followed.
    const down = await e.fetch(call([origin]), get(`${origin}/to-http`));
    expect(down).toMatchObject({ status: 301, redirectNotFollowed: true });
    expect(seen.map((s) => s.path)).toEqual(["/to-http"]);
  });

  test("private_addresses_refused: literals and their IPv4-mapped forms", async () => {
    const refused = [
      "127.0.0.1", "127.255.0.9", "10.0.0.1", "10.255.255.255", "172.16.0.1", "172.31.255.254", "192.168.1.1", "169.254.169.254",
      "100.64.0.1", "100.127.255.254", "0.0.0.0", "0.1.2.3", "224.0.0.1", "239.255.255.250", "240.0.0.1", "255.255.255.255",
      "::", "::1", "fc00::1", "fd7a:115c:a1e0::1", "fe80::1", "fe80::1%eth0", "febf::1", "fec0::1", "ff02::1",
      "::ffff:127.0.0.1", "::ffff:7f00:1", "::ffff:10.0.0.1", "::ffff:a9fe:a9fe", "0:0:0:0:0:ffff:192.168.0.1", "::127.0.0.1",
      "64:ff9b::a00:1", "64:ff9b::192.168.0.1", "2002:c0a8:101::1", "2002:7f00:1::",
      // IANA special-purpose blocks that are not globally reachable.
      "192.0.0.8", "192.0.2.1", "198.51.100.1", "203.0.113.7", "198.18.0.1", "198.19.255.255", "192.88.99.1",
      "64:ff9b:1::a00:1", "64:ff9b:1::b00:7", "::ffff:0:10.0.0.1", "::ffff:0:a00:1", "2001::1", "2001:0:4136:e378::1", "100::1",
      "2001:10::1", "2001:20::1", "2001:2::1", "2001:db8::1", "3fff::1", "5f00::1", "4000::1", "::2",
    ];
    for (const a of refused) expect(privateReason(a), a).toBeDefined();
    const pub = ["11.0.0.1", "172.32.0.1", "172.15.255.255", "100.63.255.255", "100.128.0.1", "198.20.0.1", "192.0.3.1", "2a00:1::1", "2001:200::1", "::ffff:11.0.0.7", "::ffff:0:b00:7", "64:ff9b::b00:7", "2002:b00:7::1"];
    for (const a of pub) expect(privateReason(a), a).toBeUndefined();
    expect(privateReason("not-an-address")).toBe("invalid");
    expect(ipv6Groups("1:2:3:4:5:6:1.2.3.4")).toEqual([1, 2, 3, 4, 5, 6, 0x102, 0x304]);
    expect(ipv6Groups("::ffff:127.0.0.1")).toEqual([0, 0, 0, 0, 0, 0xffff, 0x7f00, 1]);
    // Through the broker: an approved literal origin is still refused, the hook never applies.
    let t = 0;
    const e = makeEgress({ hooks: hooks(), now: () => (t += 1000) });
    seen.length = 0;
    for (const host of ["127.0.0.1", "10.1.2.3", "192.168.0.10", "169.254.169.254", "100.100.100.100", "[::1]", "[fe80::1]", "[fd00::5]", "[::ffff:127.0.0.1]", "0.0.0.0", "[ff02::1]"]) {
      const o = new URL(`https://${host}:${port}`).origin;
      const err = await failure(e.fetch(call([o]), get(`${o}/hello`)));
      expect(err.code, host).toBe("EGRESS_PRIVATE_ADDRESS");
    }
    expect(seen).toHaveLength(0);
  });

  test("dns_answer_with_one_private_address_refused", async () => {
    seen.length = 0;
    const logged: string[] = [];
    const e = makeEgress({ hooks: hooks({ lookup: async () => ["11.0.0.7", "10.0.0.5"] }), log: (p, m) => void logged.push(`${p} ${m}`) });
    const err = await failure(e.fetch(call([origin]), get(`${origin}/hello`)));
    // The plugin learns the origin and the class, never the internal address: that is logged.
    expect([err.code, err.details]).toEqual(["EGRESS_PRIVATE_ADDRESS", { origin, reason: "private" }]);
    expect(logged).toEqual([`${PLUGIN} egress refused: ${origin} resolves to 10.0.0.5 (private)`]);
    // The hook's pair is (name, address): the same loopback answer for another name is refused.
    const e2 = makeEgress({ hooks: hooks({ allow: [{ host: OTHER, address: "127.0.0.1" }] }) });
    expect((await failure(e2.fetch(call([origin]), get(`${origin}/hello`)))).code).toBe("EGRESS_PRIVATE_ADDRESS");
    expect(seen).toHaveLength(0);
    expect(e.stats().get("@test/egress")?.privateAddress).toBe(1);
  });

  test("rebinding_between_resolve_and_connect_cannot_redirect: the resolver is asked once", async () => {
    let asked = 0;
    const rebinding = async (h: string): Promise<string[]> => {
      asked++;
      // First the checked loopback server, then (if ever asked again) somewhere inside.
      return h === API && asked === 1 ? ["127.0.0.1"] : ["10.0.0.9"];
    };
    const e = makeEgress({ hooks: hooks({ lookup: rebinding }) });
    seen.length = 0;
    const r = await e.fetch(call([origin]), get(`${origin}/redirect/2`));
    expect(r.status).toBe(200);
    expect(readAll(e, "j1", r).toString()).toBe("landed");
    expect(asked).toBe(1);
    // Every hop went to the checked address, named by the origin.
    expect(seen.map((s) => [s.path, s.headers.host, s.servername])).toEqual([0, 1, 2].map((i) => [`/redirect/${2 - i}`, `${API}:${port}`, API]));
  });

  test("same_origin_redirects_followed_cross_origin_returned", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    expect((await e.fetch(call([origin]), get(`${origin}/redirect/3`))).status).toBe(200);
    expect(seen).toHaveLength(4);
    // A 4th same-origin redirect comes back unfollowed.
    const four = await e.fetch(call([origin]), get(`${origin}/redirect/4`));
    expect(four).toMatchObject({ status: 302, redirectNotFollowed: true });
    expect(four.headers).toContainEqual({ name: "location", value: "/redirect/0" });
    // Cross-origin, even to an approved origin: returned with its location and body, never sent.
    seen.length = 0;
    const cross = await e.fetch(call([origin, other]), get(`${origin}/cross`));
    expect(cross).toMatchObject({ status: 302, redirectNotFollowed: true, length: 5 });
    expect(cross.headers).toContainEqual({ name: "location", value: `${other}/x` });
    expect(readAll(e, "j1", cross).toString()).toBe("moved");
    expect(seen.map((s) => s.path)).toEqual(["/cross"]);
    // 303 after a POST: the next hop is a GET without the body.
    seen.length = 0;
    const post = await e.fetch(call([origin]), get(`${origin}/see-other`, { method: "POST", bodyB64: Buffer.from("payload").toString("base64"), headers: [{ name: "content-type", value: "text/plain" }] }));
    expect(post.status).toBe(200);
    expect(seen.map((s) => [s.method, s.path, s.body.toString(), s.headers["content-type"]])).toEqual([
      ["POST", "/see-other", "payload", "text/plain"],
      ["GET", "/hello", "", undefined],
    ]);
    expect(e.stats().get("@test/egress")).toMatchObject({ redirectsFollowed: 7, redirectsReturned: 2 });
  });

  test("body_and_time_caps", async () => {
    const e = makeEgress({ hooks: hooks() });
    // 64 MiB is held and read in pages of at most 768,000 bytes.
    const max = await e.fetch(call([origin]), get(`${origin}/big?n=${BODY_BYTES}`));
    expect(max.length).toBe(BODY_BYTES);
    const page = e.read("j1", { handle: max.handle, offset: BODY_BYTES - 10, length: READ_BYTES });
    expect([Buffer.from(page.bytesB64, "base64").length, page.eof]).toEqual([10, true]);
    // A page's base64 and the reply envelope fit 5C's 1 MiB line.
    const first = e.read("j1", { handle: max.handle, offset: 0, length: READ_BYTES });
    expect(JSON.stringify({ t: "reply", id: 1, ok: true, value: first }).length).toBeLessThan(1 << 20);
    expect((() => { try { e.read("j1", { handle: max.handle, offset: 0, length: READ_BYTES + 1 }); } catch (x) { return (x as FinError).details; } })()).toMatchObject({ reason: "pageTooLarge" });
    // One byte more is refused.
    const over = await failure(e.fetch(call([origin]), get(`${origin}/big?n=${BODY_BYTES + 1}`)));
    expect(over).toMatchObject({ code: "EGRESS_TOO_LARGE", details: { what: "response", limit: BODY_BYTES } });
    // Without a content-length, the cap holds on the bytes read.
    const small = makeEgress({ hooks: hooks({ limits: { bodyBytes: 1000 } }) });
    expect((await failure(small.fetch(call([origin]), get(`${origin}/chunked-big`)))).code).toBe("EGRESS_TOO_LARGE");
    // Request bodies: at most 768,000 bytes, inline.
    const big = Buffer.alloc(READ_BYTES + 1).toString("base64");
    expect(await failure(e.fetch(call([origin]), get(`${origin}/hello`, { method: "POST", bodyB64: big })))).toMatchObject({ code: "EGRESS_TOO_LARGE", details: { what: "request" } });
    expect((await failure(e.fetch(call([origin]), get(`${origin}/hello`, { method: "POST", bodyB64: "not base64!" })))).code).toBe("INVALID_REQUEST");
    // The time cap covers the whole call.
    const quick = makeEgress({ hooks: hooks({ limits: { timeoutMs: 300 } }) });
    const t0 = Date.now();
    expect(await failure(quick.fetch(call([origin]), get(`${origin}/slow`)))).toMatchObject({ code: "EGRESS_TIMEOUT", details: { limitS: 0.3 } });
    expect(Date.now() - t0).toBeLessThan(5_000);
    expect(quick.stats().get("@test/egress")?.timeouts).toBe(1);
    // A resolver that hangs is bounded too.
    const hang = makeEgress({ hooks: hooks({ limits: { timeoutMs: 200 }, lookup: () => new Promise(() => undefined) }) });
    expect((await failure(hang.fetch(call([origin]), get(`${origin}/hello`)))).code).toBe("EGRESS_TIMEOUT");
    // Bodies belong to their job and go when it ends.
    expect(() => e.read("j2", { handle: max.handle, offset: 0, length: 1 })).toThrow(/no body this job holds/);
    expect(e.heldBytes()).toBeGreaterThanOrEqual(BODY_BYTES);
    e.release("j1");
    expect(e.heldBytes()).toBe(0);
    expect(() => e.read("j1", { handle: max.handle, offset: 0, length: 1 })).toThrow(/no body this job holds/);
  });

  test("rate_limit: 10 requests a second per plugin", async () => {
    let now = 1_000_000;
    const e = makeEgress({ hooks: hooks(), now: () => now });
    for (let i = 0; i < 10; i++) expect((await e.fetch(call([origin]), get(`${origin}/hello`))).status).toBe(200);
    const limited = await failure(e.fetch(call([origin], "j2", "@test/egress@1.0.1"), get(`${origin}/hello`)));
    expect(limited).toMatchObject({ code: "EGRESS_RATE_LIMITED", details: { limit: 10, retryable: true } });
    // Another plugin has its own rate.
    expect((await e.fetch(call([origin], "j3", "@test/other@1.0.0"), get(`${origin}/hello`))).status).toBe(200);
    now += 1000;
    expect((await e.fetch(call([origin]), get(`${origin}/hello`))).status).toBe(200);
    expect(e.stats().get("@test/egress")).toMatchObject({ requests: 11, rateLimited: 1 });
  });

  test("no_secret_reaches_plugin: a grant answers SECRET_MISSING, no header is added", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    expect(await failure(e.fetch(call([origin]), get(`${origin}/hello`, {}, "openrouter")))).toMatchObject({ code: "SECRET_MISSING", details: { name: "openrouter" } });
    expect(seen).toHaveLength(0);
    await e.fetch(call([origin]), get(`${origin}/hello`, { headers: [{ name: "accept", value: "text/plain" }] }));
    expect(Object.keys(seen[0]!.headers).sort()).toEqual(["accept", "connection", "host"]);
    expect(seen[0]!.headers.connection).toBe("close");
    // The plugin cannot set what the broker owns, nor proxy credentials.
    for (const name of ["Host", "Proxy-Authorization", "Content-Length", "Transfer-Encoding", "proxy-anything"]) {
      expect((await failure(e.fetch(call([origin]), get(`${origin}/hello`, { headers: [{ name, value: "x" }] })))).code, name).toBe("INVALID_REQUEST");
    }
    expect((await failure(e.fetch(call([origin]), get(`https://user:pw@${API}:${port}/hello`)))).code).toBe("INVALID_REQUEST");
    expect(e.stats().get("@test/egress")?.secretMissing).toBe(1);
  });

  test("in_flight_cap: 4 requests of a plugin at once; bytes in flight count against the budgets", async () => {
    const e = makeEgress({ hooks: hooks({ limits: { timeoutMs: 20_000 } }) });
    const slow = [0, 1, 2, 3].map((i) => e.fetch(call([origin], `s${i}`), get(`${origin}/slow`)).catch((x: FinError) => x));
    for (let i = 0; i < 200 && e.inFlight() < 4; i++) await new Promise((x) => setTimeout(x, 10));
    expect(e.inFlight()).toBe(4);
    expect(await failure(e.fetch(call([origin], "s4"), get(`${origin}/hello`)))).toMatchObject({ code: "EGRESS_RATE_LIMITED", details: { reason: "inFlight", limit: 4, retryable: true } });
    // Another plugin is not held up by this one.
    expect((await e.fetch(call([origin], "o1", "@test/other@1.0.0"), get(`${origin}/hello`))).status).toBe(200);
    for (let i = 0; i < 4; i++) e.release(`s${i}`);
    for (const r of await Promise.all(slow)) expect((r as FinError).details).toMatchObject({ reason: "jobEnded" });
    expect(e.inFlight()).toBe(0);
    // The job's budget counts what it holds and what is arriving.
    const job = makeEgress({ hooks: hooks({ limits: { jobBytes: 2000 } }) });
    await job.fetch(call([origin], "b1"), get(`${origin}/chunked-big`));
    expect(await failure(job.fetch(call([origin], "b1"), get(`${origin}/chunked-big`)))).toMatchObject({ code: "EGRESS_TOO_LARGE", details: { what: "held", limit: 2000 } });
    expect(job.heldBytes()).toBe(1200);
    // The server's ceiling covers every job; a refusal there is retryable.
    const server = makeEgress({ hooks: hooks({ limits: { serverBytes: 1500 } }) });
    await server.fetch(call([origin], "c1"), get(`${origin}/chunked-big`));
    expect(await failure(server.fetch(call([origin], "c2", "@test/other@1.0.0"), get(`${origin}/chunked-big`)))).toMatchObject({ code: "EGRESS_RATE_LIMITED", details: { reason: "serverBytes", retryable: true } });
    expect(server.heldBytes()).toBe(1200);
    server.release("c1");
    expect(server.heldBytes()).toBe(0);
  });

  test("release_aborts_in_flight: a job's end aborts its requests and frees their bytes", async () => {
    const e = makeEgress({ hooks: hooks({ limits: { timeoutMs: 20_000 } }) });
    const pending = e.fetch(call([origin], "jr"), get(`${origin}/trickle`)).catch((x: FinError) => x);
    for (let i = 0; i < 300 && e.heldBytes() < 1000; i++) await new Promise((x) => setTimeout(x, 10));
    expect(e.heldBytes()).toBe(1000);
    const t0 = Date.now();
    e.release("jr");
    expect(e.heldBytes()).toBe(0);
    expect(await pending).toMatchObject({ code: "INVALID_REQUEST", details: { reason: "jobEnded" } });
    expect(Date.now() - t0).toBeLessThan(5_000);
    expect(e.heldBytes()).toBe(0);
    expect(e.inFlight()).toBe(0);
    // Nothing more is fetched or held for an ended job.
    expect(await failure(e.fetch(call([origin], "jr"), get(`${origin}/hello`)))).toMatchObject({ details: { reason: "jobEnded" } });
    expect(e.heldBytes()).toBe(0);
  });

  test("lookups_are_capped: a slow resolver cannot hold the pool", async () => {
    const e = makeEgress({ hooks: hooks({ limits: { lookups: 1, timeoutMs: 20_000 }, lookup: () => new Promise(() => undefined) }) });
    const first = e.fetch(call([origin], "l1"), get(`${origin}/hello`)).catch((x: FinError) => x);
    expect(await failure(e.fetch(call([origin], "l2", "@test/other@1.0.0"), get(`${origin}/hello`)))).toMatchObject({ code: "EGRESS_RATE_LIMITED", details: { reason: "lookups", limit: 1 } });
    e.release("l1");
    expect(await first).toMatchObject({ details: { reason: "jobEnded" } });
  });

  test("headers: tokens only, no CR/LF/NUL, no __proto__; proxy variables are ignored", async () => {
    const e = makeEgress({ hooks: hooks() });
    seen.length = 0;
    for (const h of [{ name: "x-a", value: "v\r\nInjected: 1" }, { name: "x-b", value: "v\nx" }, { name: "x-c", value: "a\u0000b" }, { name: "__proto__", value: "x" }, { name: "bad name", value: "x" }, { name: "x:y", value: "x" }, { name: "", value: "x" }]) {
      expect((await failure(e.fetch(call([origin]), get(`${origin}/hello`, { headers: [h] })))).code, JSON.stringify(h)).toBe("INVALID_REQUEST");
    }
    expect(seen).toHaveLength(0);
    const keys = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];
    const saved = keys.map((k) => process.env[k]);
    try {
      for (const k of keys) process.env[k] = "http://127.0.0.1:9";
      expect((await e.fetch(call([origin]), get(`${origin}/hello`))).status).toBe(200);
      expect(seen).toHaveLength(1);
    } finally {
      keys.forEach((k, i) => (saved[i] === undefined ? delete process.env[k] : (process.env[k] = saved[i])));
    }
  });

  test("connect_falls_back_to_the_next_checked_address", async () => {
    const e = makeEgress({
      hooks: hooks({ lookup: async () => ["::1", "127.0.0.1"], allow: [{ host: API, address: "::1" }, { host: API, address: "127.0.0.1" }] }),
    });
    // The server listens on 127.0.0.1 only: ::1 refuses the connection, the next address answers.
    expect((await e.fetch(call([origin]), get(`${origin}/hello`))).status).toBe(200);
  });

  test("hooks need the test build's flag", () => {
    expect(() => makeEgress({ hooks: { ca } as unknown as EgressTestHooks })).toThrow(/test-hooks build/);
  });
});

describe("net.* through onCall", () => {
  const owner = { kind: "local", id: "local" } as const;
  const job = (grants: Permission[], id = "jc"): JobView =>
    ({ id, owner, via: { plugin: PLUGIN }, rights: rightsFor({ invoker: { ...owner, scopes: SCOPES as never, projects: "*" }, grants, class: "person", generation: 0 }), project: "p1", params: { command: "c", args: {}, project: "p1", branch: "main", rev: 1 } }) as unknown as JobView;
  // Made per test: the certificate exists only once `beforeAll` ran.
  const rig = () => {
    const egress = makeEgress({ hooks: hooks() });
    const calls = makeHostCalls({
      deps: () => ({ live: () => LOCAL, reach: { subtree: (_p, _r, _pr, b) => Effect.succeed(b) }, snapshot: () => Effect.die("unused") }),
      keyNow: () => "k",
      coldKey: () => undefined,
      log: makePluginLog({ write: () => undefined }),
      egress,
    });
    return calls;
  };
  const invokeWith = (calls: ReturnType<typeof makeHostCalls>, j: JobView, method: string, params: unknown) => {
    const ctx: CallCtx = { job: j, role: "worker", owner: j.owner, via: j.via!, rights: j.rights as NonNullable<CallCtx["rights"]> };
    return Effect.runPromise(Effect.result(calls.onCall(ctx, method, params)));
  };

  test("net_fetch_uses_the_jobs_approved_origins; bodies released with the job", async () => {
    const calls = rig();
    const invoke = (j: JobView, method: string, params: unknown) => invokeWith(calls, j, method, params);
    // Without a `net` grant the permission check refuses first.
    const none = await invoke(job([{ perm: "project:read" }]), "net.fetch", get(`${origin}/hello`));
    expect(none._tag === "Failure" && (none.failure as FinError).code).toBe("PLUGIN_PERMISSION_DENIED");
    const j = job([{ perm: "net", origins: [origin] }]);
    const denied = await invoke(j, "net.fetch", get(`${other}/hello`));
    expect(denied._tag === "Failure" && (denied.failure as FinError).code).toBe("EGRESS_DENIED");
    const ok = await invoke(j, "net.fetch", get(`${origin}/hello`));
    expect(ok._tag, JSON.stringify(ok)).toBe("Success");
    const reply = (ok as { success: NetFetchReply }).success;
    const page = await invoke(j, "net.read", { handle: reply.handle, offset: 0, length: 100 });
    expect(page._tag === "Success" && Buffer.from((page.success as { bytesB64: string }).bytesB64, "base64").toString()).toBe("hello");
    // Malformed params are protocol errors before anything is sent.
    const bad = await invoke(j, "net.fetch", { request: { method: "GET", url: `${origin}/hello` }, extra: 1 });
    expect(bad._tag === "Failure" && (bad.failure as FinError).code).toBe("PLUGIN_PROTOCOL");
    calls.release(j.id);
    const gone = await invoke(j, "net.read", { handle: reply.handle, offset: 0, length: 1 });
    expect(gone._tag === "Failure" && (gone.failure as FinError).details).toMatchObject({ reason: "unknownHandle" });
  });
});
