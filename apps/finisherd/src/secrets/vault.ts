// The server's vault and identity key at start (5B Decisions 4, 5, 7).
//
// - The vault opens after the control store (it is per environment): `FIN_SECRETS` picks
//   `auto` (default), `keychain` or `file`; `FIN_SECRETS_KEY_FILE` wraps the file store. In
//   `auto` a locked, absent or failing keychain falls back to the file store without a prompt,
//   and `server.status` says so (`SECRETS_FILE_STORE`). `<server-data>/secrets/` is made
//   private first (on Windows the protected owner-only DACL, checked at every start).
// - The identity key is the vault's `identity`; `control.db` `meta.identitySpki` records its
//   pin, so a key that went missing (a wiped keychain, a lost file store) is told apart from a
//   first start: then a new key is a **rotation**, every device is revoked (they pair again)
//   and it is audited `identity.rotate {reason: lost, count}`. A key whose pin differs from the
//   recorded one refuses the start (`IDENTITY_MISMATCH`: another server data's secrets).
// - **The store sticks** (`meta.secretsBackend`): `auto` keeps a file store chosen before, so a
//   keychain that unlocks later never strands secrets in the files; and when the keychain that
//   holds this environment's secrets does not answer (locked, absent), the server starts
//   **without** an identity key or a usable secret box (`SECRETS_KEYCHAIN_UNAVAILABLE`), never
//   reading the missing key as lost. Asking for another store (`FIN_SECRETS`, a key file) moves
//   to it, and a key not found there is then a rotation.
// - A new key is recorded first (`meta.identitySpki`), devices are revoked, then it is stored:
//   a crash in between is read as a lost key at the next start, never as a mismatch.
// - The PEM goes only to the TLS context (Task 10); it is never logged.
import { join } from "node:path";
import { generateIdentityKey, identityKey, type IdentityKey, Vault, type VaultBackend } from "@finisher/fin-node";
import { finError } from "../errors.ts";
import type { DeviceStore } from "../auth/devices.ts";
import { ensurePrivateDir } from "../config.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import type { SecretBox } from "./box.ts";
import { vaultBox } from "./vault-box.ts";

export const IDENTITY_SPKI_META = "identitySpki";
export const SECRETS_BACKEND_META = "secretsBackend";

export interface ServerSecrets {
  readonly vault: Vault;
  readonly backend: VaultBackend;
  /**
   * The current identity key (`rotate` replaces it, Task 5); `undefined` while the keychain that
   * holds it does not answer (then nothing that needs it, pairing or the remote listener, runs).
   */
  identity: IdentityKey | undefined;
  /** The box `FileSecretStore` seals with. */
  readonly box: SecretBox;
  /** `server.status` warnings about where secrets are kept. */
  readonly warnings: () => ReadonlyArray<Record<string, unknown>>;
}

export interface OpenSecretsDeps {
  readonly serverData: string;
  readonly store: ControlStore;
  readonly audit: Audit;
  readonly devices: DeviceStore;
  readonly env?: NodeJS.ProcessEnv;
}

/**
 * Opens the vault of the control store's environment (`FIN_SECRETS`, `FIN_SECRETS_KEY_FILE`);
 * `recorded` is the store used before: `auto` stays on a file store.
 */
export function openVault(serverData: string, envId: string, env: NodeJS.ProcessEnv = process.env, recorded?: string): Vault {
  ensurePrivateDir(join(serverData, "secrets"));
  const keyFile = env.FIN_SECRETS_KEY_FILE;
  return Vault.open(serverData, envId, effectiveMode(env.FIN_SECRETS ?? "auto", recorded), keyFile === undefined || keyFile === "" ? undefined : keyFile);
}

/** `FIN_SECRETS` as the vault opens with it: `auto` stays on a file store used before. */
export function effectiveMode(asked: string, recorded: string | undefined): string {
  return (asked === "auto" || asked === "") && (recorded === "file" || recorded === "fileWrapped") ? "file" : asked;
}

/**
 * The reason the server runs without its secrets: the keychain recorded as holding them did not
 * answer, so `auto` fell back to files (a store the owner asked for is a move, not this).
 */
export function keychainMissing(recorded: string | undefined, backend: VaultBackend): string | undefined {
  return recorded === "keychain" && backend.kind !== "keychain" ? backend.reason : undefined;
}

function writeMeta(store: ControlStore, key: string, value: string): void {
  store.db.prepare("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value").run(key, value);
}

function readMeta(store: ControlStore, key: string): string | undefined {
  const row = store.db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined;
  return row?.value;
}

export function writeIdentitySpki(store: ControlStore, spki: string): void {
  writeMeta(store, IDENTITY_SPKI_META, spki);
}

/**
 * Every live device revoked for a new identity key (`reason`: `lost` at start, `owner` for
 * `env.rotateIdentity`), audited `identity.rotate {reason, count}`. The live connections are the
 * caller's to end (none exist at start).
 */
export function revokeForRotation(deps: { devices: DeviceStore; audit: Audit }, reason: "lost" | "owner", by: { kind: string; id: string }): number {
  let count = 0;
  for (const d of deps.devices.list()) if (deps.devices.markEnded(d.id, "rotation")) count++;
  deps.devices.persist();
  deps.audit.append({ kind: "identity.rotate", principal: by, outcome: "ok", details: { reason, count } });
  return count;
}

/** Loads the identity key (Decision 5); a lost key with an identity on record is a rotation. */
export function loadIdentity(vault: Vault, deps: { store: ControlStore; audit: Audit; devices: DeviceStore }): IdentityKey {
  const recorded = readMeta(deps.store, IDENTITY_SPKI_META);
  // `IDENTITY_MISMATCH` (a start error) when the vault's key is not the recorded one.
  const existing = identityKey(vault, recorded);
  if (existing !== undefined) {
    if (recorded !== existing.spki) writeIdentitySpki(deps.store, existing.spki);
    return existing;
  }
  // Record, revoke, then store: a crash before the store is a lost key at the next start.
  const fresh = generateIdentityKey();
  writeIdentitySpki(deps.store, fresh.spki);
  if (recorded !== undefined) revokeForRotation(deps, "lost", { kind: "system", id: "finisherd" });
  vault.set("identity", Buffer.from(fresh.pem, "utf8"));
  return fresh;
}

/** Where secrets are kept, as `server.status` warns about it (never a path, never a value). */
export function secretsWarnings(backend: VaultBackend): ReadonlyArray<Record<string, unknown>> {
  if (backend.kind === "keychain") return [];
  if (backend.kind === "fileWrapped" && backend.reason === undefined) return [];
  return [
    {
      code: "SECRETS_FILE_STORE",
      reason: backend.reason ?? "requested",
      wrapped: backend.kind === "fileWrapped",
      message:
        backend.kind === "fileWrapped"
          ? "the server keeps its secrets in encrypted files in its data folder"
          : "the server keeps its secrets in files in its private data folder: that folder's permissions are their only protection",
      fix:
        backend.reason === "keychainLocked"
          ? "Unlock the system keychain and restart the server to keep secrets there."
          : "Keep the server-data folder private to your account; give the server a secrets key file to encrypt them.",
    },
  ];
}

/** A box that refuses while the keychain holding the data key does not answer. */
function unavailableBox(reason: string): SecretBox {
  const refuse = (): never => {
    throw finError(
      "SECRET_STORE_UNAVAILABLE",
      "the system keychain that holds this server's secrets does not answer",
      { reason },
      "Unlock the system keychain, then restart the server.",
    );
  };
  return { seal: refuse, open: refuse };
}

/** The vault, the identity key and the secret box of a starting server. */
export function openSecrets(deps: OpenSecretsDeps): ServerSecrets {
  const recorded = readMeta(deps.store, SECRETS_BACKEND_META);
  const vault = openVault(deps.serverData, deps.store.envId, deps.env, recorded);
  const backend = vault.backend();
  const reason = keychainMissing(recorded, backend);
  if (reason !== undefined) {
    // The keychain holds this environment's secrets and does not answer: never a rotation.
    const warning = {
      code: "SECRETS_KEYCHAIN_UNAVAILABLE",
      reason,
      message: "the system keychain that holds this server's identity and secrets does not answer: paired devices, remote access and stored provider keys are unavailable",
      fix: "Unlock the system keychain, then restart the server.",
    };
    return { vault, backend, identity: undefined, box: unavailableBox(reason), warnings: () => [warning] };
  }
  const identity = loadIdentity(vault, deps);
  if (recorded !== backend.kind) writeMeta(deps.store, SECRETS_BACKEND_META, backend.kind);
  return { vault, backend, identity, box: vaultBox(vault), warnings: () => secretsWarnings(backend) };
}
