// 6B-1 Task 17 (plan Decisions 13 and 14, review focus 6): cache entries over HTTP through
// capability URLs. `GET`/`HEAD /media/cap/<token>/<file>` is routed in `upgrade.ts` before the
// WebSocket path with the listener's own Host and Origin checks; tokens are minted by `media.url`
// (`project.read` and reach over one of the entry's projects), live in memory for at most 10
// minutes, 64 per principal, and go with their principal. Entries are synthetic: written like a
// worker's `out/<key>/` and committed through `MediaCache.commit`.
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, Principal } from "@finisher/contracts";
import { MediaCache } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { revokeCredential } from "../src/auth/revoke.ts";
import { Tickets } from "../src/auth/tickets.ts";
import { ConnectionRegistry } from "../src/connections.ts";
import { loopbackPolicy } from "../src/http/policy.ts";
import { CAP_TTL_MS, CapTokens, MAX_CAP_TOKENS_PER_PRINCIPAL, mediaUrl } from "../src/media/cap-tokens.ts";
import { makeMediaCache, type MediaCacheService } from "../src/media/cache.ts";
import { contentTypeOf, makeMediaHttp, parseRange } from "../src/media/http.ts";
import { LOCAL } from "../src/principal.ts";
import { makeTicketedWebSocketServer } from "../src/upgrade.ts";
import { pattern, writeEntry } from "./support/media-entry.ts";
import { connectProved, ok, startServer, type RunningServer } from "./support/ndjson-client.ts";

const agentOf = (id: string, projects: Principal["projects"]): Principal => ({ kind: "agent", id, scopes: ["project.read"], projects, branches: "*", human: false });
const A = "01jpa000000000000000000001";
const B = "01jpb000000000000000000002";
const ONLY_A = agentOf("01agentaaaaaaaaaaaaaaaaaaaa", [A]);
const ONLY_B = agentOf("01agentbbbbbbbbbbbbbbbbbbbb", [B]);
const MIB = 1024 * 1024;
const BIG = pattern(2 * MIB + 12_345);
const FILES: Record<string, Buffer | string> = { "proxy.mp4": BIG, "thumb.png": "png bytes", "strip.json": '{"tiles":[]}', "table.bin": pattern(100) };
/** Larger than every socket buffer on the path, so a slow reader holds the stream open. */
const HUGE = pattern(24 * MIB + 321);
const HUGE_FILES: Record<string, Buffer | string> = { "proxy.mp4": HUGE, "thumb.png": "png bytes" };

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;
const scopes: Scope.Closeable[] = [];

beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "fin-media-http-")));
  serverData = join(tmp, "sd");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  for (const s of scopes.splice(0)) await Effect.runPromise(Scope.close(s, Exit.void));
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

let jobs = 0;
/** Commits an entry of `files` serving `projects`; its key. */
async function commitEntry(cache: MediaCache, fingerprint: string, projects: string[], files: Record<string, Buffer | string> = FILES): Promise<string> {
  const out = join(tmp, "jobs", `j${++jobs}`, "out");
  const key = writeEntry(out, fingerprint, files);
  await cache.commit(out, key, projects);
  return key;
}

const failure = async <A>(e: Effect.Effect<A, FinError>): Promise<FinError> => {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit.value)}`);
  const reason = exit.cause.reasons.find((r) => r._tag === "Fail");
  if (reason === undefined || reason._tag !== "Fail") throw new Error("expected a FinError");
  return reason.error;
};

interface Got {
  status: number;
  headers: Record<string, string | string[] | undefined>;
  body: Buffer;
}

interface Rig {
  readonly port: number;
  readonly svc: MediaCacheService;
  readonly cache: MediaCache;
  readonly tokens: CapTokens;
  readonly clock: { now: number };
  /** What the HTTP side reads the cache from: set `cache` to `undefined` to model a cache that did not open. */
  readonly live: { cache: MediaCache | undefined };
  readonly key: string;
  /** A token for `file` of the rig's entry, minted for `principal` (default `LOCAL`). */
  readonly mint: (file: string, principal?: Principal) => string;
  readonly call: (method: string, path: string, headers?: Record<string, string>) => Promise<Got>;
  readonly get: (token: string, file: string, headers?: Record<string, string>, method?: string) => Promise<Got>;
}

async function rig(options: { headersTimeoutMs?: number; maxUnauthenticated?: number; stallMs?: number; files?: Record<string, Buffer | string> } = {}): Promise<Rig> {
  const { files, stallMs, ...listener } = options;
  const scope = Effect.runSync(Scope.make());
  scopes.push(scope);
  const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000 })));
  const cache = svc.cache!;
  const key = await commitEntry(cache, "fp-rig", [A], files);
  const live = { cache: svc.cache };
  const clock = { now: 1_800_000_000_000 };
  const tokens = new CapTokens({ now: () => clock.now });
  let port = 0;
  const policy = loopbackPolicy(() => port, () => ["https://app.example.test"]);
  const built = await Effect.runPromise(
    Scope.provide(
      makeTicketedWebSocketServer({
        check: () => ({ ok: false, status: 401, reason: "badTicket" }),
        maxPayload: 1024,
        policy,
        media: makeMediaHttp({ tokens, cache: () => live.cache, ...(stallMs === undefined ? {} : { stallMs }) }, policy),
        ...listener,
      }),
      scope,
    ),
  );
  port = built.port;
  const call = (method: string, path: string, headers: Record<string, string> = {}) =>
    new Promise<Got>((resolve, reject) => {
      const req = request({ host: "127.0.0.1", port, method, path, headers: { host: `127.0.0.1:${port}`, ...headers }, agent: false }, (res) => {
        const chunks: Buffer[] = [];
        res.on("data", (c: Buffer) => chunks.push(c));
        res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) }));
        res.on("error", reject);
      });
      req.on("error", reject);
      req.end();
    });
  return {
    port,
    svc,
    cache,
    tokens,
    clock,
    live,
    key,
    mint: (file, principal = LOCAL) => tokens.mint(principal, key, file).token,
    call,
    get: (token, file, headers, method = "GET") => call(method, `/media/cap/${token}/${encodeURIComponent(file)}`, headers),
  };
}

const sha = (b: Buffer) => createHash("sha256").update(b).digest("hex");

describe("tokens", () => {
  test("a token is 32 random bytes in base64url, bound to its grant, expiring in 10 minutes at most", () => {
    const clock = { now: 1_000 };
    const tokens = new CapTokens({ now: () => clock.now });
    const a = tokens.mint(ONLY_A, "k".repeat(64), "proxy.mp4");
    expect(a.token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(Buffer.from(a.token, "base64url")).toHaveLength(32);
    expect(a.expiresAt).toBe(1_000 + CAP_TTL_MS);
    expect(CAP_TTL_MS).toBeLessThanOrEqual(10 * 60_000);
    expect(tokens.mint(ONLY_A, "k".repeat(64), "proxy.mp4").token).not.toBe(a.token);
    expect(tokens.resolve(a.token)).toMatchObject({ key: "k".repeat(64), file: "proxy.mp4", principal: { kind: "agent", id: ONLY_A.id } });
    // Expired at its expiry, not a millisecond before; an expired token is gone.
    clock.now = a.expiresAt - 1;
    expect(tokens.resolve(a.token)).toBeDefined();
    clock.now = a.expiresAt;
    expect(tokens.resolve(a.token)).toBeUndefined();
    // Nothing that is not a token resolves, whatever it looks like.
    for (const t of ["", "x", "__proto__", "constructor", "a".repeat(43), "a".repeat(100_000)]) expect(tokens.resolve(t), t.slice(0, 20)).toBeUndefined();
  });

  test("at most 64 per principal: the oldest is dropped; other principals are not touched", () => {
    const tokens = new CapTokens({ now: () => 1 });
    const other = tokens.mint(ONLY_B, "k".repeat(64), "proxy.mp4").token;
    const minted = Array.from({ length: MAX_CAP_TOKENS_PER_PRINCIPAL + 3 }, () => tokens.mint(ONLY_A, "k".repeat(64), "proxy.mp4").token);
    expect(MAX_CAP_TOKENS_PER_PRINCIPAL).toBe(64);
    expect(minted.slice(0, 3).map((t) => tokens.resolve(t))).toEqual([undefined, undefined, undefined]);
    expect(minted.slice(3).every((t) => tokens.resolve(t) !== undefined)).toBe(true);
    expect(tokens.resolve(other)).toBeDefined();
    expect(tokens.size).toBe(MAX_CAP_TOKENS_PER_PRINCIPAL + 1);
  });

  test("the sweep of expired tokens keeps the table from growing", () => {
    const clock = { now: 0 };
    const tokens = new CapTokens({ now: () => clock.now });
    for (let i = 0; i < 10; i++) tokens.mint(agentOf(`01agent${i}`.padEnd(26, "x"), "*"), "k".repeat(64), "f");
    clock.now = CAP_TTL_MS;
    tokens.mint(ONLY_A, "k".repeat(64), "f");
    expect(tokens.size).toBe(1);
  });
});

describe("media.url", () => {
  test("reach_required_for_media_url: a key the caller cannot reach is NOT_FOUND, as a missing one", async () => {
    const scope = Effect.runSync(Scope.make());
    scopes.push(scope);
    const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000 })));
    const cache = svc.cache!;
    const mine = await commitEntry(cache, "fp-a", [A]);
    const theirs = await commitEntry(cache, "fp-b", [B]);
    const shared = await commitEntry(cache, "fp-ab", [A, B]);
    const orphan = await commitEntry(cache, "fp-none", []);
    const tokens = new CapTokens({ now: () => 5 });
    const deps = { cache: svc, tokens };
    const url = (p: Principal, key: string, file = "proxy.mp4") => Effect.runPromise(mediaUrl(deps, p, { key, file }));

    const got = await url(ONLY_A, mine);
    expect(got.expiresAt).toBe(5 + CAP_TTL_MS);
    expect(got.url).toMatch(/^\/media\/cap\/[A-Za-z0-9_-]{43}\/proxy\.mp4$/);
    expect(tokens.resolve(got.url.split("/")[3]!)).toMatchObject({ key: mine, file: "proxy.mp4", principal: { id: ONLY_A.id } });
    expect((await url(ONLY_A, shared)).url).toMatch(/^\/media\/cap\//);
    expect((await url(ONLY_B, shared)).url).toMatch(/^\/media\/cap\//);
    expect((await url(LOCAL, theirs)).url).toMatch(/^\/media\/cap\//);

    const missing = await failure(mediaUrl(deps, ONLY_A, { key: "0".repeat(64), file: "proxy.mp4" }));
    expect(missing.code).toBe("NOT_FOUND");
    const outside = await failure(mediaUrl(deps, ONLY_A, { key: theirs, file: "proxy.mp4" }));
    // The same answer, to the byte: nothing says whether the key exists.
    expect(JSON.stringify(outside)).toBe(JSON.stringify(missing));
    // An entry that serves no project (adopted, never served) is reachable only through `*`.
    expect((await failure(mediaUrl(deps, ONLY_A, { key: orphan, file: "proxy.mp4" }))).code).toBe("NOT_FOUND");
    expect((await url(LOCAL, orphan)).url).toMatch(/^\/media\/cap\//);
    // A principal that reaches no project at all.
    expect((await failure(mediaUrl(deps, agentOf("01agentnoneeeeeeeeeeeeeeee", []), { key: mine, file: "proxy.mp4" }))).code).toBe("NOT_FOUND");
    // No token was minted for any refusal.
    expect(tokens.size).toBe(5);
  });

  test("a file the entry does not list, a path, meta.json and a malformed key are NOT_FOUND", async () => {
    const scope = Effect.runSync(Scope.make());
    scopes.push(scope);
    const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000 })));
    const key = await commitEntry(svc.cache!, "fp-a", [A]);
    const tokens = new CapTokens();
    const deps = { cache: svc, tokens };
    for (const file of ["meta.json", "../meta.json", "..", "a/b", "", "other.bin", "PROXY.MP4", "proxy.mp4/", "proxy.mp4\0"]) {
      expect((await failure(mediaUrl(deps, LOCAL, { key, file }))).code, JSON.stringify(file)).toBe("NOT_FOUND");
    }
    for (const bad of ["", "nope", "../x", `${key}/..`, key.toUpperCase(), `${key}\0`]) {
      expect((await failure(mediaUrl(deps, LOCAL, { key: bad, file: "proxy.mp4" }))).code, JSON.stringify(bad)).toBe("NOT_FOUND");
    }
    expect(tokens.size).toBe(0);
  });

  test("a cache that did not open is CACHE_IO, not a missing key", async () => {
    mkdirSync(join(serverData, "cache"));
    writeFileSync(join(serverData, "cache", "media"), "not a directory");
    const scope = Effect.runSync(Scope.make());
    scopes.push(scope);
    const svc = await Effect.runPromise(Scope.provide(scope)(makeMediaCache(serverData, { everyMs: 3_600_000, log: () => {} })));
    expect(svc.cache).toBeUndefined();
    expect((await failure(mediaUrl({ cache: svc, tokens: new CapTokens() }, LOCAL, { key: "0".repeat(64), file: "proxy.mp4" }))).code).toBe("CACHE_IO");
  });
});

describe("serving", () => {
  test("range_requests_exact_bytes", async () => {
    const r = await rig();
    const t = r.mint("proxy.mp4");
    const whole = await r.get(t, "proxy.mp4");
    expect(whole.status).toBe(200);
    expect(whole.body.length).toBe(BIG.length);
    expect(sha(whole.body)).toBe(sha(BIG));
    expect(whole.headers).toMatchObject({
      "content-type": "video/mp4",
      "content-length": String(BIG.length),
      "accept-ranges": "bytes",
      "cache-control": "private, max-age=600, immutable",
      "cross-origin-resource-policy": "same-origin",
      "x-content-type-options": "nosniff",
    });
    expect(String(whole.headers.etag)).toMatch(/^"[0-9a-f]{64}"$/);

    const range = async (h: string) => r.get(t, "proxy.mp4", { range: h });
    const slice = async (h: string, from: number, to: number) => {
      const got = await range(h);
      expect(got.status, h).toBe(206);
      expect(got.headers["content-range"], h).toBe(`bytes ${from}-${to}/${BIG.length}`);
      expect(got.headers["content-length"], h).toBe(String(to - from + 1));
      expect(got.body.equals(BIG.subarray(from, to + 1)), h).toBe(true);
    };
    await slice("bytes=0-0", 0, 0);
    await slice("bytes=0-99", 0, 99);
    await slice("bytes=100-", 100, BIG.length - 1);
    await slice("bytes=-50", BIG.length - 50, BIG.length - 1);
    // Past the end is clamped to it; a suffix longer than the file is the whole file.
    await slice(`bytes=${BIG.length - 10}-${BIG.length + 999}`, BIG.length - 10, BIG.length - 1);
    await slice(`bytes=-${BIG.length * 3}`, 0, BIG.length - 1);
    // Across the 1 MiB reads of the stream.
    await slice(`bytes=${MIB - 7}-${MIB + 9}`, MIB - 7, MIB + 9);
    await slice(`bytes=5-${2 * MIB + 5}`, 5, 2 * MIB + 5);

    // 416: past the end, an empty file's suffix, a start after its end.
    for (const h of [`bytes=${BIG.length}-`, `bytes=${BIG.length + 5}-${BIG.length + 9}`, "bytes=-0", "bytes=9-3", "bytes=99999999999999999999-"]) {
      const got = await range(h);
      expect(got.status, h).toBe(416);
      expect(got.headers["content-range"], h).toBe(`bytes */${BIG.length}`);
      expect(got.body.length, h).toBe(0);
    }
    // A header that is not one range of bytes is ignored: the whole file, 200.
    for (const h of ["lines=0-5", "bytes=abc", "bytes=", "bytes", "bytes=0-5,10-20", "bytes=-", "0-5", "bytes=1-2-3", "bytes= 5-9", "bytes=0-0, "]) {
      const got = await range(h);
      expect(got.status, h).toBe(200);
      expect(got.body.length, h).toBe(BIG.length);
    }
    // If-Range: a validator that is not the file's ignores the range; the file's own keeps it.
    expect((await r.get(t, "proxy.mp4", { range: "bytes=0-9", "if-range": '"other"' })).status).toBe(200);
    expect((await r.get(t, "proxy.mp4", { range: "bytes=0-9", "if-range": String(whole.headers.etag) })).status).toBe(206);
  });

  test("HEAD answers the headers of GET and no body, ranges included", async () => {
    const r = await rig();
    const t = r.mint("proxy.mp4");
    const get = await r.get(t, "proxy.mp4");
    const head = await r.get(t, "proxy.mp4", {}, "HEAD");
    expect(head.status).toBe(200);
    expect(head.body.length).toBe(0);
    for (const h of ["content-type", "content-length", "accept-ranges", "cache-control", "cross-origin-resource-policy", "etag"]) expect(head.headers[h], h).toEqual(get.headers[h]);
    const part = await r.get(t, "proxy.mp4", { range: "bytes=10-19" }, "HEAD");
    expect(part.status).toBe(206);
    expect(part.headers["content-range"]).toBe(`bytes 10-19/${BIG.length}`);
    expect(part.headers["content-length"]).toBe("10");
    expect(part.body.length).toBe(0);
    expect((await r.get(t, "proxy.mp4", { range: `bytes=${BIG.length}-` }, "HEAD")).status).toBe(416);
    // Nothing else is a method of this route.
    for (const method of ["POST", "PUT", "DELETE", "PATCH"]) expect((await r.get(t, "proxy.mp4", {}, method)).status, method).toBe(405);
  });

  test("the content type is the file's extension, in the entry", async () => {
    const r = await rig();
    const types: Record<string, string> = { "proxy.mp4": "video/mp4", "thumb.png": "image/png", "strip.json": "application/json", "table.bin": "application/octet-stream" };
    for (const [file, type] of Object.entries(types)) {
      const got = await r.get(r.mint(file), file);
      expect(got.status, file).toBe(200);
      expect(got.headers["content-type"], file).toBe(type);
      expect(got.headers["content-length"]).toBe(String(got.body.length));
    }
    expect(contentTypeOf("a.webm")).toBe("video/webm");
    expect(contentTypeOf("a.jpg")).toBe("image/jpeg");
    expect(contentTypeOf("noext")).toBe("application/octet-stream");
    expect(contentTypeOf("a.MP4")).toBe("video/mp4");
  });

  test("unknown_expired_revoked_all_404", async () => {
    const r = await rig();
    const live = r.mint("proxy.mp4");
    const expired = r.mint("proxy.mp4");
    r.clock.now += 1;
    const revoked = r.mint("proxy.mp4", ONLY_A);
    const later = r.mint("proxy.mp4");
    expect((await r.get(live, "proxy.mp4", { range: "bytes=0-1" })).status).toBe(206);
    // `expired` was minted first: move the clock past its expiry but not `later`'s.
    r.clock.now += CAP_TTL_MS - 1;
    expect((await r.get(expired, "proxy.mp4")).status).toBe(404);
    expect((await r.get(live, "proxy.mp4")).status).toBe(404);
    expect((await r.get(later, "proxy.mp4")).status).toBe(200);
    expect(r.tokens.dropWhere((p) => p.id === ONLY_A.id)).toBe(1);
    const unknown = [revoked, Buffer.alloc(32, 9).toString("base64url"), "x", "..", "%2e%2e", "a".repeat(5000)];
    const answers = await Promise.all([expired, ...unknown].map((t) => r.call("GET", `/media/cap/${t}/proxy.mp4`)));
    for (const a of answers) {
      expect(a.status).toBe(404);
      expect(a.headers["cache-control"]).toBe("no-store");
      expect(a.headers["content-length"]).toBe("0");
      expect(a.headers["content-range"]).toBeUndefined();
      expect(a.headers.etag).toBeUndefined();
      expect(a.body.length).toBe(0);
    }
    // The shapes that are not a route at all answer the same.
    for (const path of ["/media/cap/", "/media/cap", "/media/cap//proxy.mp4", `/media/cap/${later}`, `/media/cap/${later}/`, "/media/", `/media/${r.key}/proxy.mp4`]) {
      const a = await r.call("GET", path);
      expect(a.status, path).toBe(404);
    }
    // A cache that no longer holds the entry: the token is good, the entry is gone.
    await r.cache.gc(0);
    expect((await r.get(later, "proxy.mp4")).status).toBe(404);
  });

  test("file_outside_entry_404: paths, encodings and names the entry does not list", async () => {
    const r = await rig();
    const t = r.mint("proxy.mp4");
    // Tokens minted straight for names `media.url` would refuse: the server still checks the entry.
    for (const file of ["meta.json", "../meta.json", "..", "other.bin", "a/b"]) {
      const bad = r.tokens.mint(LOCAL, r.key, file).token;
      for (const path of [`/media/cap/${bad}/${encodeURIComponent(file)}`, `/media/cap/${bad}/${file}`]) {
        expect((await r.call("GET", path)).status, `${file} ${path}`).toBe(404);
      }
    }
    for (const path of [
      `/media/cap/${t}/..%2fmeta.json`,
      `/media/cap/${t}/%2e%2e%2fmeta.json`,
      `/media/cap/${t}/%2e%2e`,
      `/media/cap/${t}/%2E%2E/%2E%2E/etc/passwd`,
      `/media/cap/${t}/../proxy.mp4`,
      `/media/cap/${t}/proxy.mp4/`,
      `/media/cap/${t}/proxy.mp4/..`,
      `/media/cap/${t}/proxy.mp4%00`,
      `/media/cap/${t}/proxy.mp4%2f`,
      `/media/cap/${t}/proxy%2emp4x`,
      `/media/cap/${t}/%70roxy.mp4%`,
      `/media/cap/${t}/PROXY.MP4`,
      `/media/cap/${t}/table.bin`,
      `/media/cap/${t}/meta.json`,
      `/media/cap/${t}//proxy.mp4`,
      `/media/cap/%2e%2e/${t}/proxy.mp4`,
      `/media/cap/${t}/proxy.mp4%5c..%5cmeta.json`,
    ]) {
      expect((await r.call("GET", path)).status, path).toBe(404);
    }
    // A query is no part of the file: it is ignored, and so serves the file.
    expect((await r.call("GET", `/media/cap/${t}/proxy.mp4?x=1`, { range: "bytes=0-3" })).status).toBe(206);
    // The percent-encoded name of the file itself is the file.
    expect((await r.call("GET", `/media/cap/${t}/%70roxy.mp4`, { range: "bytes=0-3" })).status).toBe(206);
  });

  test("token_bound_to_file, and to its entry", async () => {
    const r = await rig();
    const other = await commitEntry(r.cache, "fp-other", [A], { "proxy.mp4": "another entry's bytes", "extra.bin": "x" });
    const t = r.mint("proxy.mp4");
    expect((await r.get(t, "proxy.mp4", { range: "bytes=0-3" })).status).toBe(206);
    expect((await r.get(t, "table.bin")).status).toBe(404);
    expect((await r.get(t, "thumb.png")).status).toBe(404);
    expect((await r.get(t, "extra.bin")).status).toBe(404);
    // The same file name of another entry is that entry's token's, never this one's.
    const o = r.tokens.mint(LOCAL, other, "proxy.mp4").token;
    const got = await r.get(o, "proxy.mp4");
    expect(got.body.toString()).toBe("another entry's bytes");
    expect(r.key).not.toBe(other);
  });

  test("host_and_origin_checked, before the token", async () => {
    const r = await rig();
    const t = r.mint("thumb.png");
    const ok200 = await r.get(t, "thumb.png");
    expect(ok200.status).toBe(200);
    for (const host of ["evil.example.test", `evil.example.test:${r.port}`, "127.0.0.1:1", "[::2]:80"]) {
      const bad = await r.get(t, "thumb.png", { host });
      expect(bad.status, host).toBe(403);
      expect(bad.body.toString(), host).not.toContain("png bytes");
      // A rebinding page learns nothing of the token either: the same refusal for a wrong one.
      expect((await r.get("x", "thumb.png", { host })).status, host).toBe(403);
    }
    for (const origin of ["https://evil.example.test", "http://127.0.0.1:1", "null", "not a url"]) {
      const bad = await r.get(t, "thumb.png", { origin });
      expect(bad.status, origin).toBe(403);
      expect(bad.body.toString(), origin).not.toContain("png bytes");
    }
    // An allowed origin (the page's own, or a configured one) is served, and given no CORS leave.
    for (const origin of [`http://127.0.0.1:${r.port}`, "https://app.example.test"]) {
      const good = await r.get(t, "thumb.png", { origin });
      expect(good.status, origin).toBe(200);
      expect(good.headers["access-control-allow-origin"]).toBeUndefined();
      expect(good.headers["cross-origin-resource-policy"]).toBe("same-origin");
    }
  });

  test("principal_revocation_drops_tokens: the one revoke path drops them with the tickets", async () => {
    const r = await rig();
    const tickets = new Tickets();
    tickets.onDrop((pred) => r.tokens.dropWhere(pred));
    const agent = agentOf("01agentrevokedrevokedrevo", [A]);
    const mine = [r.mint("proxy.mp4", agent), r.mint("thumb.png", agent)];
    const theirs = r.mint("thumb.png", ONLY_B);
    const keep = r.mint("thumb.png", LOCAL);
    expect((await r.get(mine[1]!, "thumb.png")).status).toBe(200);
    const connections = new ConnectionRegistry();
    const audits: unknown[] = [];
    const report = revokeCredential({ tickets, connections, audit: { append: (row: unknown) => void audits.push(row) } as never }, { kind: "agent", id: agent.id }, { reason: "owner" });
    expect(report.revoked).toBe(false);
    expect((await r.get(mine[0]!, "proxy.mp4")).status).toBe(404);
    expect((await r.get(mine[1]!, "thumb.png")).status).toBe(404);
    expect((await r.get(theirs, "thumb.png")).status).toBe(200);
    expect((await r.get(keep, "thumb.png")).status).toBe(200);
    // A mint after the revocation is for whoever asks: the credential check is the chain's, which refuses the revoked.
    expect(r.tokens.size).toBe(2);
  });

  test("a stream outlives the header timeout, and its socket stops counting as unauthenticated", { timeout: 60_000 }, async () => {
    const r = await rig({ headersTimeoutMs: 400, maxUnauthenticated: 2, files: HUGE_FILES });
    const t = r.mint("proxy.mp4");
    const open = () =>
      new Promise<{ status: number; finish: () => Promise<Buffer> }>((resolve, reject) => {
        const req = request({ host: "127.0.0.1", port: r.port, path: `/media/cap/${t}/proxy.mp4`, headers: { host: `127.0.0.1:${r.port}` }, agent: false }, (res) => {
          res.pause();
          const chunks: Buffer[] = [];
          resolve({
            status: res.statusCode ?? 0,
            finish: () =>
              new Promise<Buffer>((done, fail) => {
                res.on("data", (c: Buffer) => chunks.push(c));
                res.on("end", () => done(Buffer.concat(chunks)));
                res.on("error", fail);
                res.resume();
              }),
          });
        });
        req.on("error", reject);
        req.end();
      });
    // Three streams at once on a cap of two pre-upgrade sockets: each is released once its token checks out.
    const streams = [await open(), await open(), await open()];
    expect(streams.map((s) => s.status)).toEqual([200, 200, 200]);
    // A slow reader: nothing is read for longer than the pre-upgrade timeout (checked each second).
    await new Promise((x) => setTimeout(x, 2_500));
    for (const s of streams) expect(sha(await s.finish())).toBe(sha(HUGE));
  });

  test("the entry is pinned while it is served, and the pin goes with the response, however it ends", { timeout: 60_000 }, async () => {
    const r = await rig({ files: HUGE_FILES });
    const t = r.mint("proxy.mp4");
    const pins = () => r.cache.pinsOf(r.key);
    const settled = async () => {
      for (let i = 0; i < 200 && (await pins()).length > 0; i++) await new Promise((x) => setTimeout(x, 25));
      return pins();
    };
    expect(await pins()).toEqual([]);
    // (a request that fails is no pin: `thumb.png` is not the file this token is for)
    expect((await r.get(t, "thumb.png")).status).toBe(404);
    expect(await pins()).toEqual([]);
    // A client that goes away mid-stream.
    await new Promise<void>((resolve, reject) => {
      const req = request({ host: "127.0.0.1", port: r.port, path: `/media/cap/${t}/proxy.mp4`, headers: { host: `127.0.0.1:${r.port}` }, agent: false }, (res) => {
        res.once("data", () => {
          expect(res.statusCode).toBe(200);
          req.destroy();
          resolve();
        });
      });
      req.on("error", () => undefined);
      req.end();
      setTimeout(() => reject(new Error("no data")), 10_000);
    });
    expect(await settled()).toEqual([]);
    // While one is in flight the entry survives a gc; when it is done the entry can go.
    const slow = new Promise<{ held: string[]; evicted: number }>((resolve, reject) => {
      const req = request({ host: "127.0.0.1", port: r.port, path: `/media/cap/${t}/proxy.mp4`, headers: { host: `127.0.0.1:${r.port}` }, agent: false }, (res) => {
        res.pause();
        setTimeout(async () => {
          const held = await pins();
          const evicted = (await r.cache.gc(0)).evicted;
          res.on("end", () => resolve({ held, evicted }));
          res.resume();
        }, 300);
        res.on("error", reject);
      });
      req.on("error", reject);
      req.end();
    });
    const during = await slow;
    expect(during.held).toHaveLength(1);
    expect(during.held[0]).toMatch(/^http:/);
    expect(during.evicted).toBe(0);
    expect(await settled()).toEqual([]);
    expect((await r.cache.gc(0)).evicted).toBe(1);
  });

  test("a reader that stops reading is cut off, and its pin is released", { timeout: 60_000 }, async () => {
    const r = await rig({ files: HUGE_FILES, stallMs: 400 });
    const t = r.mint("proxy.mp4");
    // Never read: the server's write side fills, then gives up (its end of the socket may stay up for the unread data).
    const req = request({ host: "127.0.0.1", port: r.port, path: `/media/cap/${t}/proxy.mp4`, headers: { host: `127.0.0.1:${r.port}` }, agent: false }, (res) => {
      res.pause();
      res.on("error", () => undefined);
    });
    req.on("error", () => undefined);
    req.end();
    const wait = async (held: boolean) => {
      for (let i = 0; i < 400; i++) {
        if (((await r.cache.pinsOf(r.key)).length > 0) === held) return true;
        await new Promise((x) => setTimeout(x, 25));
      }
      return false;
    };
    expect(await wait(true)).toBe(true);
    expect(await wait(false)).toBe(true);
    req.destroy();
  });

  test("a file swapped for a link after its commit is never served", { skip: process.platform === "win32" }, async () => {
    const r = await rig();
    const t = r.mint("thumb.png");
    expect((await r.get(t, "thumb.png")).body.toString()).toBe("png bytes");
    const secret = join(tmp, "secret.txt");
    writeFileSync(secret, "TOP SECRET");
    const dir = r.cache.entryDir(r.key)!;
    rmSync(join(dir, "thumb.png"));
    symlinkSync(secret, join(dir, "thumb.png"));
    const got = await r.get(t, "thumb.png");
    expect(got.status).toBe(404);
    expect(got.body.toString()).not.toContain("SECRET");
    // The entry directory itself replaced by a link: the same.
    rmSync(join(dir, "table.bin"));
    renameSync(dir, `${dir}.moved`);
    symlinkSync(`${dir}.moved`, dir);
    const t2 = r.mint("strip.json");
    expect((await r.get(t2, "strip.json")).status).toBe(404);
  });

  test("a cache that did not open serves nothing", async () => {
    const r = await rig();
    const t = r.mint("thumb.png");
    expect((await r.get(t, "thumb.png")).status).toBe(200);
    r.live.cache = undefined;
    expect((await r.get(t, "thumb.png")).status).toBe(404);
  });
});

describe("ranges (units)", () => {
  test("parseRange", () => {
    const p = (h: string | undefined, size = 100) => parseRange(h, size);
    expect(p(undefined)).toEqual({ kind: "whole" });
    expect(p("bytes=0-9")).toEqual({ kind: "range", start: 0, end: 9 });
    expect(p("bytes=90-")).toEqual({ kind: "range", start: 90, end: 99 });
    expect(p("bytes=-10")).toEqual({ kind: "range", start: 90, end: 99 });
    expect(p("bytes=50-5000")).toEqual({ kind: "range", start: 50, end: 99 });
    expect(p("BYTES=0-9")).toEqual({ kind: "range", start: 0, end: 9 });
    expect(p("bytes=100-")).toEqual({ kind: "unsatisfiable" });
    expect(p("bytes=-0")).toEqual({ kind: "unsatisfiable" });
    expect(p("bytes=9-3")).toEqual({ kind: "unsatisfiable" });
    expect(p("bytes=0-0", 0)).toEqual({ kind: "unsatisfiable" });
    expect(p("bytes=0-1,5-6")).toEqual({ kind: "whole" });
    expect(p("items=0-1")).toEqual({ kind: "whole" });
    expect(p("bytes=0x1-2")).toEqual({ kind: "whole" });
    expect(p("bytes=1e2-")).toEqual({ kind: "whole" });
    expect(p("bytes=-")).toEqual({ kind: "whole" });
    expect(p("bytes=99999999999999999999-")).toEqual({ kind: "unsatisfiable" });
  });
});

describe("on a running server", () => {
  test("media.url over the RPC, then the bytes over the loopback listener; revoked and expired tokens are gone", { timeout: 120_000 }, async () => {
    // An entry committed while the server is down, serving a project created through it.
    server = await startServer(serverData);
    const local = await connectProved(server);
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    await local.close();
    await server.stop();
    const offline = await MediaCache.open(serverData);
    const key = await commitEntry(offline, "fp-live", [projectId]);
    const stranger = await commitEntry(offline, "fp-stranger", ["01jpzzzzzzzzzzzzzzzzzzzzzz"]);
    server = await startServer(serverData);
    const c = await connectProved(server);

    const got = ok<{ url: string; expiresAt: number }>(await c.call("media.url", { key, file: "proxy.mp4" }));
    expect(got.url).toMatch(/^\/media\/cap\/[A-Za-z0-9_-]{43}\/proxy\.mp4$/);
    expect(got.expiresAt).toBeGreaterThan(Date.now());
    expect(got.expiresAt).toBeLessThanOrEqual(Date.now() + CAP_TTL_MS);
    const base = `http://127.0.0.1:${server.info.wsPort}`;
    const res = await fetch(base + got.url, { headers: { range: "bytes=1000-1999" } });
    expect(res.status).toBe(206);
    expect(Buffer.from(await res.arrayBuffer()).equals(BIG.subarray(1000, 2000))).toBe(true);
    const whole = await fetch(base + got.url);
    expect(sha(Buffer.from(await whole.arrayBuffer()))).toBe(sha(BIG));
    // The loopback names only: a rebinding Host is refused.
    const rebound = await new Promise<number>((resolve, reject) => {
      const req = request({ host: "127.0.0.1", port: server!.info.wsPort, path: got.url, headers: { host: "evil.example.test" }, agent: false }, (r) => resolve(r.statusCode ?? 0));
      req.on("error", reject);
      req.end();
    });
    expect(rebound).toBe(403);

    // Refusals: a missing key, one outside reach, a file the entry does not list.
    for (const [k, f] of [["0".repeat(64), "proxy.mp4"], [key, "meta.json"], [key, "nope.bin"], ["../x", "proxy.mp4"]] as const) {
      const e = await c.call("media.url", { key: k, file: f });
      expect(e.ok ? "ok" : e.error.code, `${k} ${f}`).toBe("NOT_FOUND");
    }
    // `local` reaches every project (`"*"`), the stranger's entry included.
    expect(ok<{ url: string }>(await c.call("media.url", { key: stranger, file: "proxy.mp4" })).url).toMatch(/^\/media\/cap\//);
    // The cache's pin of an entry served leaves nothing behind.
    expect(ok<{ pinned: number }>(await c.call("media.cache.status", {})).pinned).toBe(0);
    await c.close();
  });
});
