// The handshake's MACs (plan 5A Decision 6, spec §4.2). HMAC-SHA-256 over a domain-separated
// message in which every variable-length field is preceded by its length:
//
//   message = label ‖ for each field: (string → u16be(len) ‖ UTF-8 bytes | nonce → 32 raw bytes)
//
// The label is a fixed ASCII constant (no length prefix; no label is a prefix of another);
// `envId`, `endpoint`, `kind` and `id` are UTF-8 strings with a u16 big-endian byte length in
// front (above 65,535 bytes is refused); nonces are exactly 32 raw bytes, unprefixed. In full:
//
//   proof = HMAC(K, "finisherd/hello/v2" ‖ s(envId) ‖ s(endpoint) ‖ clientNonce ‖ serverNonce)
//   mac   = HMAC(K, "fin/prove/v2"       ‖ s(envId) ‖ s(endpoint) ‖ serverNonce ‖ clientNonce)
//   credentialKey(envId, kind, id) = HMAC(credential.master, "fin/credential-key/v2" ‖ s(envId) ‖ s(kind) ‖ s(id))
//
// The environment id is bound in (review of Tasks 8–9, M3): a `credential.master` copied with
// `auth/` into another server data, or kept across a `control.reset`, derives no key there.
//
// `tests/fixtures/auth/handshake.json` pins these bytes for the TS and Rust implementations.
// Nothing here logs: keys, nonces and MACs never leave the process except as MACs on the wire.
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { dirname, basename, join } from "node:path";
import { realpathSync } from "node:fs";

export const LABEL = { hello: "finisherd/hello/v2", prove: "fin/prove/v2", credentialKey: "fin/credential-key/v2" } as const;

export const NONCE_BYTES = 32;
export const KEY_BYTES = 32;

/** 32 fresh random bytes. */
export function nonce(): Buffer {
  return randomBytes(NONCE_BYTES);
}

function lengthPrefixed(s: string): Buffer {
  const b = Buffer.from(s, "utf8");
  if (b.length > 0xffff) throw new RangeError(`MAC field of ${b.length} bytes exceeds 65535`);
  const len = Buffer.alloc(2);
  len.writeUInt16BE(b.length);
  return Buffer.concat([len, b]);
}

/** The exact MAC input: the label raw, strings length-prefixed, nonces (Buffers) raw. */
export function macInput(label: string, fields: ReadonlyArray<string | Buffer>): Buffer {
  return Buffer.concat([Buffer.from(label, "ascii"), ...fields.map((f) => (typeof f === "string" ? lengthPrefixed(f) : f))]);
}

function checkKey(key: Buffer): void {
  if (key.length !== KEY_BYTES) throw new RangeError(`a key has ${KEY_BYTES} bytes, not ${key.length}`);
}

function checkNonce(n: Buffer, name: string): void {
  if (n.length !== NONCE_BYTES) throw new RangeError(`${name} has ${NONCE_BYTES} bytes, not ${n.length}`);
}

function hmac(key: Buffer, input: Buffer): Buffer {
  checkKey(key);
  return createHmac("sha256", key).update(input).digest();
}

/** The server's proof in `server.hello`. */
export function helloProof(key: Buffer, envId: string, endpoint: string, clientNonce: Buffer, serverNonce: Buffer): Buffer {
  checkNonce(clientNonce, "clientNonce");
  checkNonce(serverNonce, "serverNonce");
  return hmac(key, macInput(LABEL.hello, [envId, endpoint, clientNonce, serverNonce]));
}

/** The client's MAC in `auth.prove`. */
export function proveMac(key: Buffer, envId: string, endpoint: string, serverNonce: Buffer, clientNonce: Buffer): Buffer {
  checkNonce(serverNonce, "serverNonce");
  checkNonce(clientNonce, "clientNonce");
  return hmac(key, macInput(LABEL.prove, [envId, endpoint, serverNonce, clientNonce]));
}

/** An `agent`/`mcp` credential's key, derived from `credential.master` (never stored). */
export function credentialKey(master: Buffer, envId: string, kind: "agent" | "mcp", id: string): Buffer {
  return hmac(master, macInput(LABEL.credentialKey, [envId, kind, id]));
}

/** Constant-time equality after a length check. */
export function sameMac(a: Buffer, b: Buffer): boolean {
  return a.length === b.length && timingSafeEqual(a, b);
}

const PIPE = /^\\\\[.?]\\pipe\\/i;

/**
 * The endpoint both MACs bind (Decision 7): a named pipe's name lower-cased
 * (`\\.\pipe\finisher-<user>[-<hash>]`), or a socket's `realpath` (its directory's when the
 * socket does not exist yet).
 */
export function canonicalEndpoint(endpoint: string, platform: NodeJS.Platform = process.platform): string {
  if (PIPE.test(endpoint) || (platform === "win32" && endpoint.startsWith("\\\\"))) return endpoint.toLowerCase();
  try {
    return realpathSync(endpoint);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw e;
    return join(realpathSync(dirname(endpoint)), basename(endpoint));
  }
}

const B64U = /^[A-Za-z0-9_-]*$/;

/** base64url without padding; `decode` is strict (canonical input only, else `null`). */
export const b64u = {
  encode(b: Buffer): string {
    return b.toString("base64url");
  },
  decode(s: string): Buffer | null {
    if (!B64U.test(s) || s.length % 4 === 1) return null;
    const b = Buffer.from(s, "base64url");
    return b.toString("base64url") === s ? b : null;
  },
};
