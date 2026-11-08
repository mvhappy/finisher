// The vault and the identity key binding (5B Task 2): `Vault` over
// crates/fin-node/src/vault/mod.rs; values cross as Buffers. Native failures carry their code
// (`SECRETS_KEY_INVALID`, `SECRET_STORE_UNAVAILABLE`, `IDENTITY_MISMATCH`, …).
import { native, sync } from "./native.js";

/** Small named secrets in the OS keychain or the server data's private `secrets/` folder. */
export class Vault {
  #native;
  constructor(n) {
    this.#native = n;
  }
  static open(serverData, envId, mode, keyFile) {
    return new Vault(sync(() => native.Vault.open(serverData, envId, mode, keyFile ?? undefined)));
  }
  backend() {
    const b = this.#native.backend();
    return { kind: b.kind, ...(b.reason == null ? {} : { reason: b.reason }) };
  }
  get(name) {
    return sync(() => this.#native.get(name)) ?? undefined;
  }
  set(name, value) {
    sync(() => this.#native.set(name, value));
  }
  delete(name) {
    return sync(() => this.#native.delete(name));
  }
  /** The native vault, for `identityKey` and `newIdentityKey`. */
  get native() {
    return this.#native;
  }
}

/** The vault's identity key, or `undefined`; `IDENTITY_MISMATCH` when not `expectedSpki`'s. Never makes one. */
export function identityKey(vault, expectedSpki) {
  return sync(() => native.identityKey(vault.native, expectedSpki ?? undefined)) ?? undefined;
}

/** A new identity key, stored nowhere yet. */
export function generateIdentityKey() {
  return sync(() => native.generateIdentityKey());
}

/** `base64url(SHA-256(SubjectPublicKeyInfo))` of a P-256 PKCS#8 PEM, 43 characters. */
export function spkiPin(pem) {
  return sync(() => native.spkiPin(pem));
}
