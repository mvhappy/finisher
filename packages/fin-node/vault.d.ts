// The vault and the identity key (5B Task 2, Decisions 4–6).

/** Which store a vault uses; `reason` says why a file store stands in for the keychain. */
export interface VaultBackend {
  kind: "keychain" | "file" | "fileWrapped";
  reason?: "keychainLocked" | "keychainAbsent" | "keychainError";
}

/**
 * Small named secrets (`identity`, `secrets-dek`): the OS keychain (service `finisher`,
 * account `<envId>:<name>`) or `<server-data>/secrets/<name>` (0600; AES-256-GCM with a key
 * file). Names are 1–64 lower-case letters, digits and dashes.
 */
export declare class Vault {
  private constructor();
  /**
   * `mode`: `auto` (the keychain when it answers without a prompt, else the file store, never
   * blocking), `keychain` (or `SECRET_STORE_UNAVAILABLE`) or `file`. `keyFile`: 32 random bytes
   * in base64url wrapping the file store (`SECRETS_KEY_INVALID` when unreadable).
   */
  static open(serverData: string, envId: string, mode: string, keyFile?: string): Vault;
  backend(): VaultBackend;
  /** The value, or `undefined`; `SECRETS_KEY_INVALID` when a wrapped file does not open. */
  get(name: string): Buffer | undefined;
  set(name: string, value: Buffer): void;
  delete(name: string): boolean;
}

/** The identity key: its PKCS#8 PEM (for the TLS context only, never logged), its pin, made now or not. */
export interface IdentityKey {
  pem: string;
  spki: string;
  created: boolean;
}

/**
 * The vault's `identity` (`created: false`), or `undefined` when there is none;
 * `IDENTITY_MISMATCH` when its pin is not `expectedSpki`, `SECRET_CORRUPT` when it is damaged.
 */
export declare function identityKey(vault: Vault, expectedSpki?: string): IdentityKey | undefined;
/** A new identity key (`created: true`), stored nowhere: the caller records its pin, then stores it. */
export declare function generateIdentityKey(): IdentityKey;
/** `base64url(SHA-256(SubjectPublicKeyInfo DER))` of a P-256 PKCS#8 PEM (43 characters). */
export declare function spkiPin(pem: string): string;
