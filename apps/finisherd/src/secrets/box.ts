// The seal/open seam (plan 7A Decision 22; sub-project 7 §10). `v: 1` is the identity box: the
// 0600 file in the private server-data directory is the protection, and the sandbox denies the
// directory to agents. `v: 2` (5B Decision 7, `vault-box.ts`) is AES-256-GCM under a data key
// kept in the vault; v1 values still open and are resealed at the next write.

export interface SealedV1 {
  readonly v: 1;
  /** base64url */
  readonly data: string;
}

export interface SealedV2 {
  readonly v: 2;
  /** base64url: the ciphertext, then the 16-byte tag. */
  readonly data: string;
  /** base64url: the 96-bit nonce. */
  readonly iv: string;
}

export type Sealed = SealedV1 | SealedV2;

export interface SecretBox {
  seal(plaintext: Buffer): Sealed;
  open(s: Sealed): Buffer;
}

const B64URL = /^[A-Za-z0-9_-]*$/;

/** Whether `s` has the shape of a sealed value of either version (its contents are the box's). */
export function isSealed(s: unknown): s is Sealed {
  const x = s as { v?: unknown; data?: unknown; iv?: unknown } | null;
  if (typeof x?.data !== "string" || !B64URL.test(x.data)) return false;
  return x.v === 1 || (x.v === 2 && typeof x.iv === "string" && B64URL.test(x.iv));
}

/** Decodes a `Sealed`'s data, or throws when it is not version 1 base64url. */
export function sealedBytes(s: Sealed): Buffer {
  if (s?.v !== 1 || typeof s.data !== "string" || !B64URL.test(s.data)) throw new Error("not a sealed value");
  return Buffer.from(s.data, "base64url");
}

export const identityBox: SecretBox = {
  seal: (plaintext) => ({ v: 1, data: plaintext.toString("base64url") }),
  open: (s) => sealedBytes(s),
};
