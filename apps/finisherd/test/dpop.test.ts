// 5B Task 7: DPoP proofs (Decision 14) and server nonces (Decision 15). RFC vectors from
// `tests/fixtures/auth/dpop.json`; every other proof is made here with keys generated per run.
import { readFileSync } from "node:fs";
import { calculateJwkThumbprint, exportJWK, generateKeyPair, type JWK, SignJWT } from "jose";
import { describe, expect, test } from "vitest";
import { athOf, DpopVerifier, dpopRefusal, MAX_REMEMBERED_JTI, readProof } from "../src/auth/dpop.ts";
import { NONCE_ROTATE_MS, Nonces } from "../src/auth/nonces.ts";

const FIX = JSON.parse(readFileSync(new URL("../../../tests/fixtures/auth/dpop.json", import.meta.url), "utf8"));
const ORIGIN = "https://studio.example.test:7311";
const URL_TOKEN = new URL(`${ORIGIN}/auth/token`);

function clock(start = 1_800_000_000_000) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

async function keypair(alg = "ES256") {
  const { privateKey, publicKey } = await generateKeyPair(alg, { extractable: true });
  return { privateKey, jwk: await exportJWK(publicKey), full: await exportJWK(privateKey) };
}

let jtiN = 0;
async function proof(
  key: Awaited<ReturnType<typeof keypair>>,
  claims: Record<string, unknown>,
  header: { typ?: string; alg?: string; jwk?: JWK } = {},
): Promise<string> {
  return new SignJWT({ jti: `jti-${jtiN++}`, htm: "POST", htu: URL_TOKEN.href, ...claims })
    .setProtectedHeader({ typ: header.typ ?? "dpop+jwt", alg: header.alg ?? "ES256", jwk: header.jwk ?? key.jwk })
    .sign(key.privateKey);
}

function setup() {
  const c = clock();
  const nonces = new Nonces(c.now);
  const verifier = new DpopVerifier(nonces, c.now);
  const iat = () => Math.floor(c.now() / 1000);
  const check = { method: "POST", url: URL_TOKEN, allowedOrigins: [ORIGIN], knownKey: () => true };
  return { c, nonces, verifier, iat, check };
}

describe("DPoP", () => {
  test("rfc9449_vectors: the RFC's proofs verify, carry its claims and its thumbprint", async () => {
    for (const which of ["token", "resource"] as const) {
      const v = FIX.rfc9449[which];
      const read = await readProof(v.proof);
      expect("ok" in read).toBe(false);
      if ("ok" in read) continue;
      expect(read.jkt).toBe(FIX.rfc9449.jkt);
      expect(read.claims).toMatchObject({ htm: v.htm, htu: v.htu, iat: v.iat });
      if (which === "resource") {
        expect(read.claims.ath).toBe(v.ath);
        expect(athOf(v.accessToken)).toBe(v.ath);
      }
    }
    // As a request to this server they fail closed: no server nonce, a stale `iat`, another origin.
    const { verifier } = setup();
    const r = await verifier.verify(FIX.rfc9449.token.proof, { method: "POST", url: new URL("https://server.example.com/token"), allowedOrigins: ["https://server.example.com"], knownKey: () => true });
    expect(r).toMatchObject({ ok: false, code: "DPOP_INVALID", reason: "iat" });
  });

  test("thumbprint_rfc7638", async () => {
    expect(await calculateJwkThumbprint(FIX.rfc7638.jwk, "sha256")).toBe(FIX.rfc7638.thumbprint);
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const r = await verifier.verify(await proof(k, { iat: iat(), nonce: nonces.current() }), check);
    expect(r).toMatchObject({ ok: true, jkt: await calculateJwkThumbprint(k.jwk, "sha256") });
  });

  test("typ_alg_and_jwk_checked", async () => {
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const ok = { iat: iat(), nonce: nonces.current() };
    expect(await verifier.verify(await proof(k, ok, { typ: "JWT" }), check)).toMatchObject({ reason: "typ" });
    // An HMAC "proof" and an unsigned one.
    const hs = await new SignJWT({ ...ok, jti: "hs", htm: "POST", htu: URL_TOKEN.href })
      .setProtectedHeader({ typ: "dpop+jwt", alg: "HS256", jwk: k.jwk })
      .sign(new Uint8Array(32));
    expect(await verifier.verify(hs, check)).toMatchObject({ reason: "alg" });
    const b64 = (o: unknown) => Buffer.from(JSON.stringify(o)).toString("base64url");
    const none = `${b64({ typ: "dpop+jwt", alg: "none", jwk: k.jwk })}.${b64({ ...ok, jti: "none", htm: "POST", htu: URL_TOKEN.href })}.`;
    expect(await verifier.verify(none, check)).toMatchObject({ reason: "alg" });
    // An RSA key, a JWK with its private part, another key than the signer's.
    const rsa = await keypair("RS256");
    expect(await verifier.verify(await proof(rsa, ok, { alg: "RS256" }), check)).toMatchObject({ reason: "alg" });
    expect(await verifier.verify(await proof(k, ok, { jwk: k.full }), check)).toMatchObject({ reason: "jwk" });
    const other = await keypair();
    expect(await verifier.verify(await proof(k, ok, { jwk: other.jwk }), check)).toMatchObject({ reason: "signature" });
    expect(await verifier.verify("not.a.proof", check)).toMatchObject({ code: "DPOP_INVALID" });
    expect(await verifier.verify(undefined, check)).toMatchObject({ reason: "missing" });
  });

  test("htm_must_match", async () => {
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    expect(await verifier.verify(await proof(k, { htm: "GET", iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "htm" });
  });

  test("htu_behind_proxy: the origin from publicUrl, the path kept, the query ignored", async () => {
    const { verifier, nonces, iat } = setup();
    const k = await keypair();
    const publicUrl = "https://edit.example.test";
    const check = { method: "POST", url: new URL("https://127.0.0.1:7311/auth/token?x=1"), allowedOrigins: [ORIGIN, publicUrl], knownKey: () => true };
    const r = await verifier.verify(await proof(k, { htu: `${publicUrl}/auth/token?ignored=1#frag`, iat: iat(), nonce: nonces.current() }), check);
    expect(r.ok).toBe(true);
    expect(await verifier.verify(await proof(k, { htu: `${publicUrl}/auth/ws-ticket`, iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "htu" });
  });

  test("htu_other_origin_refused", async () => {
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    for (const htu of ["https://evil.example.test/auth/token", "http://studio.example.test:7311/auth/token", "https://studio.example.test:7312/auth/token", "not a url"]) {
      expect(await verifier.verify(await proof(k, { htu, iat: iat(), nonce: nonces.current() }), check), htu).toMatchObject({ reason: "htu" });
    }
  });

  test("iat_sanity_window", async () => {
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const day = 86_400;
    expect((await verifier.verify(await proof(k, { iat: iat() - 6 * day, nonce: nonces.current() }), check)).ok).toBe(true);
    expect(await verifier.verify(await proof(k, { iat: iat() - 8 * day, nonce: nonces.current() }), check)).toMatchObject({ reason: "iat" });
    expect(await verifier.verify(await proof(k, { iat: iat() + 8 * day, nonce: nonces.current() }), check)).toMatchObject({ reason: "iat" });
    expect(await verifier.verify(await proof(k, { nonce: nonces.current() }), check)).toMatchObject({ reason: "iat" });
  });

  test("old_nonce_is_use_dpop_nonce (status, WWW-Authenticate, header)", async () => {
    const { c, verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const missing = await verifier.verify(await proof(k, { iat: iat() }), check);
    expect(missing).toMatchObject({ ok: false, code: "DPOP_NONCE" });
    const old = nonces.current();
    c.advance(2 * NONCE_ROTATE_MS);
    const stale = await verifier.verify(await proof(k, { iat: iat(), nonce: old }), check);
    expect(stale).toMatchObject({ ok: false, code: "DPOP_NONCE" });
    if (stale.ok) return;
    const answer = dpopRefusal(stale);
    expect(answer.status).toBe(401);
    expect(answer.headers?.["WWW-Authenticate"]).toMatch(/^DPoP error="use_dpop_nonce"/);
    expect(JSON.stringify(answer)).not.toContain(old);
    // The client retries once with the header's nonce, and gets through.
    expect((await verifier.verify(await proof(k, { iat: iat(), nonce: nonces.current() }), check)).ok).toBe(true);
    expect(dpopRefusal({ ok: false, code: "DPOP_INVALID", reason: "htm" }).headers?.["WWW-Authenticate"]).toBe('DPoP error="invalid_dpop_proof"');
  });

  test("previous_nonce_still_valid", async () => {
    const { c, verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const first = nonces.current();
    c.advance(NONCE_ROTATE_MS);
    const second = nonces.current();
    expect(second).not.toBe(first);
    expect(nonces.accepts(first)).toBe(true);
    expect((await verifier.verify(await proof(k, { iat: iat(), nonce: first }), check)).ok).toBe(true);
    c.advance(NONCE_ROTATE_MS);
    nonces.current();
    expect(nonces.accepts(first)).toBe(false);
    expect(nonces.accepts(second)).toBe(true);
    expect(first).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  test("replayed_jti_refused", async () => {
    const { c, verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const p = await proof(k, { jti: "once", iat: iat(), nonce: nonces.current() });
    expect((await verifier.verify(p, check)).ok).toBe(true);
    expect(await verifier.verify(p, check)).toMatchObject({ reason: "replay" });
    // Another proof with the same `jti` too, while its nonce lives.
    expect(await verifier.verify(await proof(k, { jti: "once", iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "replay" });
    // Once its nonce expired, the `jti` is forgotten, and the old proof fails on its nonce instead.
    c.advance(2 * NONCE_ROTATE_MS);
    expect(await verifier.verify(p, check)).toMatchObject({ code: "DPOP_NONCE" });
    expect((await verifier.verify(await proof(k, { jti: "once", iat: iat(), nonce: nonces.current() }), check)).ok).toBe(true);
  });

  test("replay_cache_full_refuses (never evicting an old entry)", async () => {
    const c = clock();
    const nonces = new Nonces(c.now);
    const verifier = new DpopVerifier(nonces, c.now, 3);
    const k = await keypair();
    const check = { method: "POST", url: URL_TOKEN, allowedOrigins: [ORIGIN], knownKey: () => true };
    const iat = Math.floor(c.now() / 1000);
    const first = await proof(k, { iat, nonce: nonces.current() });
    expect((await verifier.verify(first, check)).ok).toBe(true);
    for (let i = 0; i < 2; i++) expect((await verifier.verify(await proof(k, { iat, nonce: nonces.current() }), check)).ok).toBe(true);
    expect(await verifier.verify(await proof(k, { iat, nonce: nonces.current() }), check)).toMatchObject({ reason: "replayCache" });
    expect(await verifier.verify(first, check)).toMatchObject({ reason: "replay" });
    expect(MAX_REMEMBERED_JTI).toBe(65_536);
  });

  test("ath_required_with_token", async () => {
    const { verifier, nonces, iat } = setup();
    const k = await keypair();
    const token = "an-access-token";
    const check = { method: "POST", url: new URL(`${ORIGIN}/auth/ws-ticket`), allowedOrigins: [ORIGIN], accessToken: token, knownKey: () => true };
    const htu = `${ORIGIN}/auth/ws-ticket`;
    expect(await verifier.verify(await proof(k, { htu, iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "ath" });
    expect(await verifier.verify(await proof(k, { htu, ath: athOf("another-token"), iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "ath" });
    expect((await verifier.verify(await proof(k, { htu, ath: athOf(token), iat: iat(), nonce: nonces.current() }), check)).ok).toBe(true);
  });

  test("a nonce lives exactly as long as its jti is remembered (review of Task 7)", async () => {
    const c = clock(10 * NONCE_ROTATE_MS);
    const nonces = new Nonces(c.now);
    const n = nonces.current();
    // Asked for late in the next window: still the previous one, and no longer than its window allows.
    c.advance(NONCE_ROTATE_MS + NONCE_ROTATE_MS * 0.98);
    expect(nonces.accepts(n)).toBe(true);
    expect(nonces.expiresAt(n)).toBe(12 * NONCE_ROTATE_MS);
    c.advance(NONCE_ROTATE_MS * 0.03);
    expect(nonces.accepts(n)).toBe(false);
    // A clock that runs backwards brings no window back.
    const later = nonces.current();
    c.advance(-3 * NONCE_ROTATE_MS);
    expect(nonces.current()).toBe(later);
    expect(nonces.accepts(n)).toBe(false);
    // After an idle gap the old one is gone at once.
    c.advance(10 * NONCE_ROTATE_MS);
    expect(nonces.accepts(later)).toBe(false);
  });

  test("only known keys are remembered; keys and claims are read strictly (review of Task 7)", async () => {
    const { verifier, nonces, iat, check } = setup();
    const k = await keypair();
    const unknown = { ...check, knownKey: () => false };
    for (let i = 0; i < 5; i++) {
      expect(await verifier.verify(await proof(k, { iat: iat(), nonce: nonces.current() }), unknown)).toMatchObject({ reason: "unknownKey" });
    }
    expect(verifier.remembered).toBe(0);
    // A coordinate with other padding bits is the same point but not canonical: refused.
    const x = k.jwk.x!;
    const last = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    const twin = x.slice(0, -1) + last[(last.indexOf(x.at(-1)!) ^ 1) % 64]!;
    expect(Buffer.from(twin, "base64url").equals(Buffer.from(x, "base64url"))).toBe(true);
    expect(await verifier.verify(await proof(k, { iat: iat(), nonce: nonces.current() }, { jwk: { ...k.jwk, x: twin } }), check)).toMatchObject({ reason: "jwk" });
    // `htu` must be a string; `htm` is compared exactly.
    expect(await verifier.verify(await proof(k, { htu: [URL_TOKEN.href], iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "htu" });
    expect(await verifier.verify(await proof(k, { htm: "post", iat: iat(), nonce: nonces.current() }), check)).toMatchObject({ reason: "htm" });
    expect(await verifier.verify(await proof(k, { iat: iat(), nonce: nonces.current() }), { ...check, method: "post" })).toMatchObject({ reason: "htm" });
  });
});
