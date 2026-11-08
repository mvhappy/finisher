// 7A's secret store sealed for real (5B Decision 7): `Sealed {v: 2, data, iv}`, AES-256-GCM
// under a data key `secrets-dek` (32 random bytes) kept in the vault, with the store's own
// label as associated data. The data key is loaded (or made) when the box is, at start, so no
// later call reaches the keychain for it. `v: 1` values (the
// identity box) still open; `FileSecretStore` reseals them at its next write. With the file
// store unwrapped the data key sits in `<server-data>/secrets/` beside `secrets.json`, so the
// protection is the private directory as before (`server.status` says so). Nothing here
// returns a value but `open`, which only `FileSecretStore.reveal` (for `ModelGateway`) calls.
import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import type { Vault } from "@finisher/fin-node";
import { type Sealed, type SecretBox, sealedBytes } from "./box.ts";

export const DEK_NAME = "secrets-dek";
const AAD = Buffer.from("finisher/secrets/v2", "utf8");
const TAG = 16;

export function vaultBox(vault: Vault): SecretBox {
  let dek = vault.get(DEK_NAME);
  if (dek === undefined) {
    dek = randomBytes(32);
    vault.set(DEK_NAME, dek);
  }
  const k = dek;
  const key = (): Buffer => {
    if (k.length !== 32) throw new Error("the secret store's data key is damaged");
    return k;
  };
  return {
    seal: (plaintext) => {
      const iv = randomBytes(12);
      const c = createCipheriv("aes-256-gcm", key(), iv);
      c.setAAD(AAD);
      const data = Buffer.concat([c.update(plaintext), c.final(), c.getAuthTag()]);
      return { v: 2, data: data.toString("base64url"), iv: iv.toString("base64url") };
    },
    open: (s: Sealed) => {
      if (s.v === 1) return sealedBytes(s);
      if (s.v !== 2) throw new Error("not a sealed value");
      const data = Buffer.from(s.data, "base64url");
      const iv = Buffer.from(s.iv, "base64url");
      if (iv.length !== 12 || data.length < TAG) throw new Error("not a sealed value");
      const d = createDecipheriv("aes-256-gcm", key(), iv);
      d.setAAD(AAD);
      d.setAuthTag(data.subarray(data.length - TAG));
      return Buffer.concat([d.update(data.subarray(0, data.length - TAG)), d.final()]);
    },
  };
}
