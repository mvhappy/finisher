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
// - At start a missing key is made, recorded, then stored: a crash in between is read as a lost
//   key at the next start. A rotation or reset of a live key is staged, committed, promoted
//   (`ServerSecrets.stageIdentity`, `recoverStaged`).
// - The PEM goes only to the TLS context (Task 10); it is never logged.
import { join } from "node:path";
import { generateIdentityKey, identityKey, type IdentityKey, spkiPin, Vault, type VaultBackend } from "@finisher/fin-node";
import { finError } from "../errors.ts";
import type { DeviceStore } from "../auth/devices.ts";
import { ensurePrivateDir } from "../config.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import type { SecretBox } from "./box.ts";
import { DEK_NAME, vaultBox } from "./vault-box.ts";

export const IDENTITY_SPKI_META = "identitySpki";
export const SECRETS_BACKEND_META = "secretsBackend";

export interface ServerSecrets {
  /** The vault of the current environment (`promoteEnv` replaces it). */
  vault: Vault;
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
  /**
   * A change of key or environment is **staged, committed, promoted** (review of Task 5), so a
   * crash leaves the old state or the new one, never a server that refuses to start:
   * `stageIdentity`/`stageEnv` write the new entries under staged names (`identity-next`,
   * `secrets-dek-next`; for a reset in the new environment's vault, the same store); the
   * caller then commits `control.db` (the new pin, the new `envId`) in one transaction; then
   * `promoteIdentity`/`promoteEnv` put them in place. At every start `recoverStaged` promotes
   * staged entries whose identity pin is the recorded one and discards any others.
   */
  stageIdentity(fresh: IdentityKey): void;
  promoteIdentity(fresh: IdentityKey): void;
  /** The new environment's vault (exactly this store, or it throws), with the data key and `fresh` staged in it. */
  stageEnv(envId: string, fresh: IdentityKey): Vault;
  promoteEnv(next: Vault, fresh: IdentityKey): void;
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

export const STAGED_IDENTITY = "identity-next";
export const STAGED_DEK = "secrets-dek-next";

/**
 * At start, before the identity loads: a staged identity whose pin is the recorded one was
 * committed (a rotation or reset that crashed before promoting it): promote it, with a staged
 * data key. Anything else staged (unreadable here, another pin) never committed: discard it.
 */
export function recoverStaged(vault: Vault, recordedSpki: string | undefined): "promoted" | "discarded" | "none" {
  let unreadable = false;
  const read = (name: string): Buffer | undefined => {
    try {
      return vault.get(name);
    } catch {
      // Sealed for another environment (a reset that never committed), or damaged.
      unreadable = true;
      return undefined;
    }
  };
  const drop = (name: string) => {
    try {
      vault.delete(name);
    } catch {
      // Nothing to drop, or the store refuses: the next start tries again.
    }
  };
  const staged = read(STAGED_IDENTITY);
  const stagedDek = read(STAGED_DEK);
  let promote = false;
  if (staged !== undefined) {
    try {
      promote = spkiPin(staged.toString("utf8")) === recordedSpki;
    } catch {
      promote = false;
    }
  }
  if (promote) {
    if (stagedDek !== undefined) vault.set(DEK_NAME, stagedDek);
    vault.set("identity", staged!);
  }
  drop(STAGED_IDENTITY);
  drop(STAGED_DEK);
  return promote ? "promoted" : staged !== undefined || stagedDek !== undefined || unreadable ? "discarded" : "none";
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
    const refuse = (): never => {
      throw finError("SECRET_STORE_UNAVAILABLE", "the server's secrets are not available now", { reason }, "Unlock the system keychain, then restart the server.");
    };
    return {
      vault,
      backend,
      identity: undefined,
      box: unavailableBox(reason),
      warnings: () => [warning],
      stageIdentity: refuse,
      promoteIdentity: refuse,
      stageEnv: refuse,
      promoteEnv: refuse,
    };
  }
  recoverStaged(vault, readMeta(deps.store, IDENTITY_SPKI_META));
  const identity = loadIdentity(vault, deps);
  if (recorded !== backend.kind) writeMeta(deps.store, SECRETS_BACKEND_META, backend.kind);
  const pem = (k: IdentityKey) => Buffer.from(k.pem, "utf8");
  const secrets: ServerSecrets = {
    vault,
    backend,
    identity,
    box: vaultBox(vault),
    warnings: () => secretsWarnings(backend),
    stageIdentity: (fresh) => secrets.vault.set(STAGED_IDENTITY, pem(fresh)),
    promoteIdentity: (fresh) => {
      secrets.vault.set("identity", pem(fresh));
      secrets.vault.delete(STAGED_IDENTITY);
      secrets.identity = { ...fresh, created: false };
    },
    stageEnv: (envId, fresh) => {
      // Exactly this store: `keychain` insists on it, a file store stays one.
      const mode = backend.kind === "keychain" ? "keychain" : "file";
      const next = openVault(deps.serverData, envId, { ...(deps.env ?? process.env), FIN_SECRETS: mode });
      if (next.backend().kind !== backend.kind) {
        throw finError("SECRET_STORE_UNAVAILABLE", "the server's secret store changed since it started", {}, "Restart the server, then reset the environment again.");
      }
      const dek = secrets.vault.get(DEK_NAME);
      if (dek !== undefined) next.set(STAGED_DEK, dek);
      next.set(STAGED_IDENTITY, pem(fresh));
      return next;
    },
    promoteEnv: (next, fresh) => {
      const old = secrets.vault;
      const dek = next.get(STAGED_DEK);
      if (dek !== undefined) next.set(DEK_NAME, dek);
      next.set("identity", pem(fresh));
      next.delete(STAGED_DEK);
      next.delete(STAGED_IDENTITY);
      // A keychain keeps entries per environment: the old ones go (best effort; a file store
      // keeps one file per name, rewritten above).
      if (backend.kind === "keychain") {
        for (const name of ["identity", DEK_NAME]) {
          try {
            old.delete(name);
          } catch (e) {
            process.stderr.write(`finisherd: warning: cannot remove an old keychain entry: ${String(e)}\n`);
          }
        }
      }
      secrets.vault = next;
      secrets.identity = { ...fresh, created: false };
    },
  };
  return secrets;
}
