// DPoP proofs (5B Decision 14, RFC 9449) with `jose` 6.
//
// The `DPoP` header is one compact JWS: header `typ: dpop+jwt`, `alg: ES256`, `jwk` an EC P-256
// public key with no private member; the signature verifies with that key. Claims:
// - `jti` (1–64 characters), remembered with its nonce until that nonce expires: a second use
//   is a replay; at most 65,536 remembered, and past that a new proof is refused (never an old
//   entry evicted, which would reopen a replay);
// - `htm` the request's method; `htu` an origin the listener serves and the request's own path
//   (query and fragment ignored, spec §5.3: behind a proxy the origin is `publicUrl`'s);
// - `iat` within ±7 days (a sanity check only: freshness is the server nonce's);
// - `nonce` the current server nonce or the previous one, else `DPOP_NONCE` (the client retries
//   once with the `DPoP-Nonce` header's);
// - the key one the server knows (a live device's, or a pending pairing's), before its `jti` is
//   remembered: a made-up key cannot fill the replay cache;
// - `ath` = `base64url(SHA-256(access token))` wherever a token is presented.
// The key's thumbprint is RFC 7638's (`jkt`). Nothing here logs or returns a proof, a nonce or
// a `jti`; refusals name a closed reason word only.
import { createHash } from "node:crypto";
import { calculateJwkThumbprint, decodeProtectedHeader, importJWK, type JWK, jwtVerify, type JWTPayload } from "jose";
import type { HttpAnswer } from "../http/front.ts";
import type { Nonces } from "./nonces.ts";

export const IAT_WINDOW_S = 7 * 86_400;
export const MAX_REMEMBERED_JTI = 65_536;
export const JTI_MAX = 64;
/** The largest `DPoP` header read (a P-256 proof is well under 1 KiB). */
export const MAX_PROOF_LENGTH = 4096;

export interface DpopCheck {
  readonly method: string;
  /** The request's URL as the listener saw it (only its path is compared). */
  readonly url: URL;
  /** The origins `htu` may name: the listener's routes, `publicUrl`, loopback's on loopback. */
  readonly allowedOrigins: ReadonlyArray<string>;
  /** The access token presented with the proof (`Authorization: DPoP <token>`): `ath` binds it. */
  readonly accessToken?: string;
  /**
   * Whether the proof's key is one the server knows here (a live device's, or a pending
   * pairing's): checked before the `jti` is remembered, so keys made up on the spot cannot fill
   * the replay cache (review of Task 7). An unknown key is `DPOP_INVALID {reason: unknownKey}`.
   */
  readonly knownKey: (jkt: string) => boolean;
}

export type DpopReason =
  | "missing"
  | "malformed"
  | "typ"
  | "alg"
  | "jwk"
  | "signature"
  | "jti"
  | "htm"
  | "htu"
  | "iat"
  | "nonce"
  | "ath"
  | "replay"
  | "replayCache"
  | "unknownKey";

export type DpopResult = { ok: true; jkt: string; jwk: JWK } | { ok: false; code: "DPOP_INVALID" | "DPOP_NONCE"; reason: DpopReason };

const PRIVATE_MEMBERS = ["d", "p", "q", "dp", "dq", "qi", "k", "oth"];

/** `base64url(SHA-256(token))`, the `ath` of a proof that presents `token`. */
export function athOf(token: string): string {
  return createHash("sha256").update(token, "ascii").digest("base64url");
}

export type DpopRefused = Extract<DpopResult, { ok: false }>;

const bad = (reason: DpopReason): DpopRefused => ({ ok: false, code: "DPOP_INVALID", reason });

/** The parts of a proof its signature vouches for, before any freshness check. */
export async function readProof(header: string): Promise<{ jwk: JWK; claims: JWTPayload; jkt: string } | DpopRefused> {
  if (header.length > MAX_PROOF_LENGTH || header.split(".").length !== 3) return bad("malformed");
  let protectedHeader;
  try {
    protectedHeader = decodeProtectedHeader(header);
  } catch {
    return bad("malformed");
  }
  if (protectedHeader.typ !== "dpop+jwt") return bad("typ");
  if (protectedHeader.alg !== "ES256") return bad("alg");
  const jwk = protectedHeader.jwk as JWK | undefined;
  // Canonical coordinates only: 32 bytes each in exactly 43 base64url characters, so one key has
  // one thumbprint (lenient decoding would give one key many).
  const coord = (v: unknown) => typeof v === "string" && v.length === 43 && Buffer.from(v, "base64url").toString("base64url") === v;
  const isP256 = jwk?.kty === "EC" && jwk.crv === "P-256" && coord(jwk.x) && coord(jwk.y);
  if (jwk === undefined || !isP256 || PRIVATE_MEMBERS.some((m) => m in jwk)) return bad("jwk");
  const publicJwk: JWK = { kty: "EC", crv: "P-256", x: jwk.x!, y: jwk.y! };
  let claims: JWTPayload;
  try {
    const key = await importJWK(publicJwk, "ES256");
    // Time is checked below (a wide `iat` window, no `exp`): jose's own clock checks are off.
    ({ payload: claims } = await jwtVerify(header, key, { typ: "dpop+jwt", algorithms: ["ES256"], clockTolerance: Number.MAX_SAFE_INTEGER }));
  } catch {
    return bad("signature");
  }
  return { jwk: publicJwk, claims, jkt: await calculateJwkThumbprint(publicJwk, "sha256") };
}

export class DpopVerifier {
  readonly #nonces: Nonces;
  readonly #clock: () => number;
  /** `jti` → when it may be forgotten (its nonce's expiry). */
  readonly #seen = new Map<string, number>();
  readonly #max: number;

  constructor(nonces: Nonces, clock: () => number = Date.now, maxRemembered: number = MAX_REMEMBERED_JTI) {
    this.#nonces = nonces;
    this.#clock = clock;
    this.#max = maxRemembered;
  }

  /**
   * Entries go in by time, so their expiries rise but for the previous window's nonces (5 min
   * sooner): stopping at the first live entry leaves at most those a little longer, which only
   * ever refuses a repeat, never lets one through.
   */
  #forgetExpired(now: number): void {
    for (const [jti, until] of this.#seen) {
      if (until > now) break;
      this.#seen.delete(jti);
    }
  }

  async verify(header: string | undefined, check: DpopCheck): Promise<DpopResult> {
    if (header === undefined || header.length === 0) return bad("missing");
    const proof = await readProof(header);
    if ("ok" in proof) return proof;
    const { claims } = proof;
    if (typeof claims.jti !== "string" || claims.jti.length === 0 || claims.jti.length > JTI_MAX) return bad("jti");
    if (claims.htm !== check.method) return bad("htm");
    if (typeof claims.htu !== "string") return bad("htu");
    let htu: URL;
    try {
      htu = new URL(claims.htu);
    } catch {
      return bad("htu");
    }
    if (!check.allowedOrigins.includes(htu.origin) || htu.pathname !== check.url.pathname) return bad("htu");
    const now = this.#clock();
    if (typeof claims.iat !== "number" || Math.abs(now / 1000 - claims.iat) > IAT_WINDOW_S) return bad("iat");
    if (check.accessToken !== undefined && claims.ath !== athOf(check.accessToken)) return bad("ath");
    const nonce = claims.nonce;
    const until = typeof nonce === "string" ? this.#nonces.expiresAt(nonce) : undefined;
    if (until === undefined) return { ok: false, code: "DPOP_NONCE", reason: "nonce" };
    if (!check.knownKey(proof.jkt)) return bad("unknownKey");
    this.#forgetExpired(now);
    if (this.#seen.has(claims.jti)) return bad("replay");
    if (this.#seen.size >= this.#max) return bad("replayCache");
    this.#seen.set(claims.jti, until);
    return { ok: true, jkt: proof.jkt, jwk: proof.jwk };
  }

  /** `jti`s remembered now. */
  get remembered(): number {
    return this.#seen.size;
  }
}

/**
 * The front's answer to a refused proof: 401 with `WWW-Authenticate` (RFC 9449 §7.1, §8):
 * `use_dpop_nonce` for `DPOP_NONCE`, `invalid_dpop_proof` otherwise. The front adds the
 * current `DPoP-Nonce` header to every answer.
 */
export function dpopRefusal(r: DpopRefused): HttpAnswer {
  const nonce = r.code === "DPOP_NONCE";
  return {
    status: 401,
    headers: { "WWW-Authenticate": nonce ? 'DPoP error="use_dpop_nonce", error_description="Resource server requires nonce in DPoP proof"' : 'DPoP error="invalid_dpop_proof"' },
    body: {
      error: {
        code: r.code,
        message: nonce ? "the proof needs the server's current nonce" : "the proof of possession did not hold",
        details: { reason: r.reason },
        fix: nonce ? "Make the proof again with the nonce the server just sent." : "Make a new proof with this device's key for this request.",
      },
    },
  };
}
