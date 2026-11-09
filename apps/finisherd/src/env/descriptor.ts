// The environment (5B Decisions 30, 31; spec §5.5, §5.8): what a device pairs with.
//
// - `describe()`: `{envId, name, spki, serverIds, protocol, routes, capabilities, state}`;
//   `wellKnown()` its anonymous subset `{envId, protocol, spki}` (`GET /.well-known/finisher`).
// - `name(n)`: the owner's name for it (1–64 characters, `meta.envName`; default "Finisher").
// - `state()`: `moved` while copied or moved server data waits for the owner (`env/moved.ts`):
//   then the remote listener stays off and devices are refused (`ENV_MOVED`).
// - `adopt()`: it was a move: the state clears, audited `env.adopted`.
// - `reset()`: it is a copy: a new `envId` and identity key; devices, offers and pending
//   requests deleted; every agent and MCP credential revoked through the one path (their keys
//   bind the old `envId`); `finisherd.json` rewritten; audited `env.reset {devices, credentials}`.
// Rotations and resets stage the new secrets, commit `control.db` in one transaction, then
// promote them (`secrets/vault.ts`): a crash leaves the old state or the new one.
// - `rotateIdentity(reason)`: a new identity key with the same `envId`; every device revoked
//   (they pair again); audited `identity.rotate {reason, count}`.
// Each change notifies `onChange` (the certificate is remade, the descriptor sent).
import type { EnvState, Environment as EnvironmentView, WellKnown } from "@finisher/contracts";
import { PROTOCOL_VERSION } from "@finisher/contracts";
import { Context } from "effect";
import { generateIdentityKey, type IdentityKey } from "@finisher/fin-node";
import { ulid } from "../control/ulid.ts";
import type { DeviceStore } from "../auth/devices.ts";
import { checkedName } from "../auth/devices.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { finError } from "../errors.ts";
import type { Routes } from "../net/routes.ts";
import { type ServerSecrets, writeIdentitySpki } from "../secrets/vault.ts";

/** The oldest protocol a device may speak (protocol 2: 5A's handshake). */
export const PROTOCOL_MIN = 2;
export const DEFAULT_ENV_NAME = "Finisher";
export const ENV_NAME_META = "envName";
export const ENV_STATE_META = "envState";

type By = { readonly kind: string; readonly id: string };

export interface EnvironmentDeps {
  readonly store: ControlStore;
  readonly audit: Audit;
  readonly devices: DeviceStore;
  readonly secrets: ServerSecrets;
  readonly routes: Routes;
  readonly capabilities: ReadonlyArray<string>;
  /**
   * Revokes a device through the one path (`auth/revoke.ts` `revokeDevice`, without its own
   * audit row: the reset's or rotation's row counts them): true when it was live.
   */
  readonly revokeDevice: (id: string, reason: "reset" | "rotation") => boolean;
  /** Revokes every agent and MCP credential through the one revoke path; how many ended. */
  readonly revokeCredentials: () => number;
  /** Writes `finisherd.json` again (its `envId`). */
  readonly rewriteInfo: () => void;
}

const meta = (store: ControlStore, key: string): string | undefined =>
  (store.db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined)?.value;

const setMeta = (store: ControlStore, key: string, value: string | undefined): void => {
  if (value === undefined) store.db.prepare("DELETE FROM meta WHERE key = ?").run(key);
  else store.db.prepare("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value").run(key, value);
};

/** `ENV_MOVED` (exit 7, 403): the fix speaks to the owner, in words. */
export function envMoved(): ReturnType<typeof finError> {
  return finError(
    "ENV_MOVED",
    "this server's data was copied or moved, and its owner has not said which yet",
    {},
    "The server's owner decides at the server: adopt it if the data was moved here, or reset it if it is a copy. Paired devices work again after that.",
  );
}

const noIdentity = () =>
  finError("SECRET_STORE_UNAVAILABLE", "the server's identity key is not available now", {}, "Unlock the system keychain, then restart the server.");

export class Environment {
  readonly #d: EnvironmentDeps;
  readonly #listeners = new Set<() => void>();

  constructor(deps: EnvironmentDeps) {
    this.#d = deps;
  }

  #identity(): IdentityKey {
    const id = this.#d.secrets.identity;
    if (id === undefined) throw noIdentity();
    return id;
  }

  /** The current pin; empty while the identity key is unavailable. */
  spki(): string {
    return this.#d.secrets.identity?.spki ?? "";
  }

  state(): EnvState {
    return meta(this.#d.store, ENV_STATE_META) === "moved" ? "moved" : "ok";
  }

  serverIds(): ReadonlyArray<string> {
    const rows = this.#d.store.db.prepare("SELECT serverId FROM server_ids WHERE envId = ? ORDER BY since, serverId").all(this.#d.store.envId) as Array<{ serverId: string }>;
    return rows.map((r) => r.serverId);
  }

  describe(): EnvironmentView {
    return {
      envId: this.#d.store.envId,
      name: meta(this.#d.store, ENV_NAME_META) ?? DEFAULT_ENV_NAME,
      spki: this.spki(),
      serverIds: [...this.serverIds()],
      protocol: { min: PROTOCOL_MIN, max: PROTOCOL_VERSION },
      routes: [...this.#d.routes.current()],
      capabilities: [...this.#d.capabilities],
      state: this.state(),
    };
  }

  /** `GET /.well-known/finisher` (anonymous): never the name, the routes or the server ids. */
  wellKnown(): WellKnown {
    return { envId: this.#d.store.envId, protocol: { min: PROTOCOL_MIN, max: PROTOCOL_VERSION }, spki: this.spki() };
  }

  onChange(f: () => void): () => void {
    this.#listeners.add(f);
    return () => void this.#listeners.delete(f);
  }

  #changed(): void {
    for (const f of [...this.#listeners]) {
      try {
        f();
      } catch (e) {
        process.stderr.write(`finisherd: warning: an environment listener failed: ${String(e)}\n`);
      }
    }
  }

  name(n: string): EnvironmentView {
    setMeta(this.#d.store, ENV_NAME_META, checkedName(n, "environment name"));
    this.#changed();
    return this.describe();
  }

  /** Marks the data moved (`env/moved.ts` at start); audited `env.moved {reason}`. */
  markMoved(reason: string): void {
    this.#d.store.tx(() => {
      setMeta(this.#d.store, ENV_STATE_META, "moved");
      this.#d.audit.append({ kind: "env.moved", principal: { kind: "system", id: "finisherd" }, outcome: "ok", details: { reason } });
    });
  }

  /** It was a move: the environment and its devices stay. */
  adopt(by: By): EnvironmentView {
    if (this.state() === "moved") {
      setMeta(this.#d.store, ENV_STATE_META, undefined);
      this.#d.audit.append({ kind: "env.adopted", principal: by, outcome: "ok", details: {} });
      this.#changed();
    }
    return this.describe();
  }

  /** Revokes every live device through the one path. */
  #endAllDevices(reason: "reset" | "rotation"): number {
    let count = 0;
    for (const d of this.#d.devices.list()) if (this.#d.revokeDevice(d.id, reason)) count++;
    return count;
  }

  rotateIdentity(reason: "owner" | "lost", by: By): { spki: string; revoked: number } {
    this.#identity();
    const fresh = generateIdentityKey();
    // Staged, committed, promoted (`ServerSecrets`): a crash leaves the old key or the new one.
    this.#d.secrets.stageIdentity(fresh);
    let ended = 0;
    this.#d.store.tx(() => {
      writeIdentitySpki(this.#d.store, fresh.spki);
      ended = this.#endAllDevices("rotation");
      this.#d.audit.append({ kind: "identity.rotate", principal: by, outcome: "ok", details: { reason, count: ended } });
    });
    this.#d.secrets.promoteIdentity(fresh);
    this.#changed();
    return { spki: fresh.spki, revoked: ended };
  }

  reset(by: By): { envId: string; devices: number; credentials: number } {
    this.#identity();
    const envId = ulid(Date.now());
    const fresh = generateIdentityKey();
    // First the new environment's vault, with the data key and the new key staged: a store that
    // cannot do it refuses here, before anything changed.
    const next = this.#d.secrets.stageEnv(envId, fresh);
    // The credentials bind the old `envId`: they end through the one revoke path (each audited).
    const credentials = this.#d.revokeCredentials();
    let devices = 0;
    this.#d.store.tx(() => {
      devices = this.#endAllDevices("reset");
      this.#d.store.db.prepare("DELETE FROM pair_pending").run();
      this.#d.store.db.prepare("DELETE FROM pair_offers").run();
      this.#d.devices.deleteAll();
      setMeta(this.#d.store, ENV_STATE_META, undefined);
      writeIdentitySpki(this.#d.store, fresh.spki);
      this.#d.audit.append({ kind: "env.reset", principal: by, outcome: "ok", details: { devices, credentials } });
      this.#d.store.renewEnv(envId);
    });
    this.#d.store.useEnvId(envId);
    this.#d.secrets.promoteEnv(next, fresh);
    this.#d.rewriteInfo();
    this.#changed();
    return { envId, devices, credentials };
  }
}

export class EnvironmentTag extends Context.Service<EnvironmentTag, Environment>()("finisherd/Environment") {}
