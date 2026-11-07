// The seal/open seam (plan 7A Decision 22; sub-project 7 §10). The file backend's box is the
// identity: the 0600 file in the private server-data directory is the protection, and the
// sandbox denies the directory to agents. The keychain backend (5B) seals for real.

export interface Sealed {
  readonly v: 1;
  /** base64url */
  readonly data: string;
}

export interface SecretBox {
  seal(plaintext: Buffer): Sealed;
  open(s: Sealed): Buffer;
}

const B64URL = /^[A-Za-z0-9_-]*$/;

/** Decodes a `Sealed`'s data, or throws when it is not version 1 base64url. */
export function sealedBytes(s: Sealed): Buffer {
  if (s?.v !== 1 || typeof s.data !== "string" || !B64URL.test(s.data)) throw new Error("not a sealed value");
  return Buffer.from(s.data, "base64url");
}

export const identityBox: SecretBox = {
  seal: (plaintext) => ({ v: 1, data: plaintext.toString("base64url") }),
  open: (s) => sealedBytes(s),
};
