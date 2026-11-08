// 5B Task 2: the vault through fin-node (file and wrapped file stores), the identity key and
// its pin in `control.db`, a lost key as a rotation, a foreign key refusing the start, the
// vault box sealing 7A's secrets, and `server.status` saying where secrets are kept.
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkPrivateDir, spkiPin, Vault } from "@finisher/fin-node";
import { Effect, Exit, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { manualClock } from "../src/agent/clock.ts";
import { DeviceStore } from "../src/auth/devices.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { identityBox } from "../src/secrets/box.ts";
import { FileSecretStore } from "../src/secrets/store.ts";
import { DEK_NAME, vaultBox } from "../src/secrets/vault-box.ts";
import { effectiveMode, IDENTITY_SPKI_META, keychainMissing, loadIdentity, openSecrets, openVault, SECRETS_BACKEND_META, secretsWarnings } from "../src/secrets/vault.ts";
import { connectProved, ok, type ServerInfo } from "./support/ndjson-client.ts";

const ENV = "01envenvenvenvenvenvenvenv";
const KEY = "sk-or-v1-0123456789abcdef0123456789abcdef";
const unix = process.platform !== "win32";
let tmp: string;
let serverData: string;
let store: ControlStore | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-vault-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await store?.close();
  store = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const codeOf = (f: () => unknown): string | undefined => {
  try {
    f();
  } catch (e) {
    return (e as { code?: string }).code ?? String(e);
  }
  return undefined;
};

const keyFile = (bytes = randomBytes(32)) => {
  const p = join(tmp, `key-${bytes.toString("hex").slice(0, 6)}`);
  writeFileSync(p, `${bytes.toString("base64url")}\n`, { mode: 0o600 });
  return p;
};

const fileEnv = { FIN_SECRETS: "file" };

describe("Vault (fin-node)", () => {
  test("file_store_is_private", () => {
    const vault = openVault(serverData, ENV, fileEnv);
    expect(vault.backend()).toEqual({ kind: "file" });
    vault.set("identity", Buffer.from("v"));
    expect(vault.get("identity")?.toString()).toBe("v");
    expect(vault.get("absent")).toBeUndefined();
    const dir = join(serverData, "secrets");
    if (unix) {
      expect(statSync(dir).mode & 0o777).toBe(0o700);
      expect(statSync(join(dir, "identity")).mode & 0o777).toBe(0o600);
    }
    // Windows: the directory's DACL (fin-node `checkPrivateDir`; `{ok: true}` elsewhere).
    expect(checkPrivateDir(dir).ok).toBe(true);
    expect(vault.delete("identity")).toBe(true);
    expect(codeOf(() => vault.set("../escape", Buffer.from("x")))).toBe("INVALID_REQUEST");
  });

  test("wrapped_file_store_round_trips", () => {
    const k = keyFile();
    const vault = openVault(serverData, ENV, { FIN_SECRETS: "file", FIN_SECRETS_KEY_FILE: k });
    expect(vault.backend()).toEqual({ kind: "fileWrapped" });
    vault.set("identity", Buffer.from("a private value"));
    expect(readFileSync(join(serverData, "secrets", "identity")).includes(Buffer.from("a private value"))).toBe(false);
    expect(openVault(serverData, ENV, { FIN_SECRETS_KEY_FILE: k }).get("identity")?.toString()).toBe("a private value");
  });

  test("wrapped_file_with_wrong_key_is_secrets_key_invalid", () => {
    openVault(serverData, ENV, { FIN_SECRETS_KEY_FILE: keyFile() }).set("identity", Buffer.from("v"));
    const wrong = openVault(serverData, ENV, { FIN_SECRETS_KEY_FILE: keyFile() });
    expect(codeOf(() => wrong.get("identity"))).toBe("SECRETS_KEY_INVALID");
    const bad = join(tmp, "short-key");
    writeFileSync(bad, randomBytes(8).toString("base64url"));
    expect(codeOf(() => openVault(serverData, ENV, { FIN_SECRETS_KEY_FILE: bad }))).toBe("SECRETS_KEY_INVALID");
    expect(codeOf(() => Vault.open(serverData, ENV, "sometimes"))).toBe("INVALID_REQUEST");
  });
});

describe("the identity key", () => {
  const open = () => {
    const clock = manualClock();
    store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false, clock: clock.now });
    const audit = new Audit(store, clock.now);
    return { store, audit, devices: new DeviceStore(store, audit, clock.now) };
  };
  const meta = () => (store!.db.prepare("SELECT value FROM meta WHERE key = ?").get(IDENTITY_SPKI_META) as { value: string } | undefined)?.value;
  const jwk = { kty: "EC" as const, crv: "P-256" as const, x: "x", y: "y" };

  test("identity_key_persists", async () => {
    const deps = open();
    const vault = openVault(serverData, deps.store.envId, fileEnv);
    const first = loadIdentity(vault, deps);
    expect(first.created).toBe(true);
    expect(first.spki).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(spkiPin(first.pem)).toBe(first.spki);
    expect(meta()).toBe(first.spki);
    const again = loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    expect(again).toEqual({ ...first, created: false });
  });

  test("identity_spki_mismatch_refuses_start", async () => {
    const deps = open();
    loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    // Another server data's secrets: the vault holds a key whose pin is not the recorded one.
    store!.db.prepare("UPDATE meta SET value = ? WHERE key = ?").run("A".repeat(43), IDENTITY_SPKI_META);
    expect(codeOf(() => loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps))).toBe("IDENTITY_MISMATCH");
    // And the server refuses to start with it.
    await store!.close();
    store = undefined;
    const exit = await Effect.runPromiseExit(serve({ serverData }).pipe(Effect.provide(StderrLogger)));
    expect(Exit.isFailure(exit)).toBe(true);
    expect(JSON.stringify(exit)).toContain("IDENTITY_MISMATCH");
    expect(JSON.stringify(exit)).not.toContain("PRIVATE KEY");
  });

  test("lost_identity_with_devices_is_rotation", () => {
    const deps = open();
    const first = loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    const a = deps.devices.add({ name: "Tablet", platform: "ios", jwk: { ...jwk, x: "a" }, jkt: "jkt-a", approvedBy: "console:console" });
    deps.devices.add({ name: "Laptop", platform: "macos", jwk: { ...jwk, x: "b" }, jkt: "jkt-b", approvedBy: "console:console" });
    // The file store is lost (a wiped keychain is the same): a new key at the next start.
    rmSync(join(serverData, "secrets"), { recursive: true });
    const next = loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    expect(next.created).toBe(true);
    expect(next.spki).not.toBe(first.spki);
    expect(meta()).toBe(next.spki);
    expect(deps.devices.list()).toEqual([]);
    expect(deps.devices.list(true).find((d) => d.id === a.id)?.revokedAt).toBeDefined();
    const row = store!.db.prepare("SELECT revokedReason FROM devices WHERE id = ?").get(a.id) as { revokedReason: string };
    expect(row.revokedReason).toBe("rotation");
    const audit = store!.db.prepare("SELECT details FROM audit WHERE kind = 'identity.rotate'").all() as Array<{ details: string }>;
    expect(audit.map((r) => JSON.parse(r.details))).toEqual([{ reason: "lost", count: 2 }]);
  });

  test("the store sticks: auto keeps a file store; a silent keychain is no rotation (review of Task 2)", () => {
    expect(effectiveMode("auto", "file")).toBe("file");
    expect(effectiveMode("auto", "fileWrapped")).toBe("file");
    expect(effectiveMode("auto", "keychain")).toBe("auto");
    expect(effectiveMode("auto", undefined)).toBe("auto");
    expect(effectiveMode("keychain", "file")).toBe("keychain");
    // The recorded keychain did not answer: the server runs without its secrets, it rotates nothing.
    expect(keychainMissing("keychain", { kind: "file", reason: "keychainLocked" })).toBe("keychainLocked");
    // The owner asked for the file store: a move.
    expect(keychainMissing("keychain", { kind: "file" })).toBeUndefined();
    expect(keychainMissing("keychain", { kind: "keychain" })).toBeUndefined();
    expect(keychainMissing("file", { kind: "file", reason: "keychainAbsent" })).toBeUndefined();
    // The first start records the store.
    const deps = open();
    openSecrets({ ...deps, serverData, env: fileEnv });
    expect((store!.db.prepare("SELECT value FROM meta WHERE key = ?").get(SECRETS_BACKEND_META) as { value: string }).value).toBe("file");
  });

  test("a new key is recorded before it is stored (review of Task 2)", () => {
    const deps = open();
    const vault = openVault(serverData, deps.store.envId, fileEnv);
    const first = loadIdentity(vault, deps);
    // A crash after the record and before the store: the next start reads a lost key, never a mismatch.
    vault.delete("identity");
    const next = loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    expect(next.spki).not.toBe(first.spki);
    expect(meta()).toBe(next.spki);
  });

  test("a first start is no rotation", () => {
    const deps = open();
    loadIdentity(openVault(serverData, deps.store.envId, fileEnv), deps);
    expect(store!.db.prepare("SELECT COUNT(*) AS n FROM audit WHERE kind = 'identity.rotate'").get()).toEqual({ n: 0 });
  });
});

describe("the vault box (Decision 7)", () => {
  test("vault_box_seals_and_opens", () => {
    const vault = openVault(serverData, ENV, fileEnv);
    const box = vaultBox(vault);
    const sealed = box.seal(Buffer.from(KEY));
    expect(sealed.v).toBe(2);
    expect(Buffer.from(sealed.data, "base64url").includes(Buffer.from(KEY))).toBe(false);
    expect(box.open(sealed).toString()).toBe(KEY);
    // The data key lives in the vault; a box over the same vault opens it, another vault's does not.
    expect(vault.get(DEK_NAME)?.length).toBe(32);
    expect(vaultBox(openVault(serverData, ENV, fileEnv)).open(sealed).toString()).toBe(KEY);
    const elsewhere = join(tmp, "other");
    mkdirSync(elsewhere, { mode: 0o700 });
    const other = vaultBox(openVault(elsewhere, ENV, fileEnv));
    other.seal(Buffer.from("make its own key"));
    expect(() => other.open(sealed)).toThrow();
    // Tampering is caught.
    const flipped = Buffer.from(sealed.data, "base64url");
    flipped[0]! ^= 1;
    expect(() => box.open({ ...sealed, data: flipped.toString("base64url") })).toThrow();
  });

  test("v1_values_open_and_reseal", async () => {
    const dir = join(tmp, "agent-secrets");
    // Written by 7A's identity box.
    const before = new FileSecretStore(dir, { box: identityBox });
    const id = await before.put({ provider: "openrouter", label: "default", value: Buffer.from(KEY) });
    expect(JSON.parse(readFileSync(join(dir, "secrets.json"), "utf8")).entries[0].sealed.v).toBe(1);
    const store = new FileSecretStore(dir, { box: vaultBox(openVault(serverData, ENV, fileEnv)) });
    expect(store.reveal(id).toString()).toBe(KEY);
    // The next write reseals every older value.
    await store.put({ provider: "openrouter", label: "project", projectId: "01jb0000000000000000000001", value: Buffer.from(`${KEY}x`) });
    const doc = JSON.parse(readFileSync(join(dir, "secrets.json"), "utf8")) as { entries: Array<{ id: string; sealed: { v: number } }> };
    expect(doc.entries.map((e) => e.sealed.v)).toEqual([2, 2]);
    expect(readFileSync(join(dir, "secrets.json"), "utf8")).not.toContain(Buffer.from(KEY).toString("base64url"));
    expect(store.reveal(id).toString()).toBe(KEY);
  });

  test("secret_store_runs_on_windows", async () => {
    const dir = join(tmp, "win-secrets");
    const store = new FileSecretStore(dir, { platform: "win32", box: vaultBox(openVault(serverData, ENV, fileEnv)) });
    const id = await store.put({ provider: "openrouter", label: "default", value: Buffer.from(KEY) });
    expect(store.reveal(id).toString()).toBe(KEY);
    expect(readdirSync(dir)).toContain("secrets.json");
  });
});

describe("server.status", () => {
  test("warnings say where secrets are kept", () => {
    expect(secretsWarnings({ kind: "keychain" })).toEqual([]);
    expect(secretsWarnings({ kind: "fileWrapped" })).toEqual([]);
    expect(secretsWarnings({ kind: "file", reason: "keychainLocked" })[0]).toMatchObject({ code: "SECRETS_FILE_STORE", reason: "keychainLocked", wrapped: false });
    expect(secretsWarnings({ kind: "file" })[0]).toMatchObject({ code: "SECRETS_FILE_STORE", reason: "requested" });
  });

  test("a started server reports its file store and keeps the identity", async () => {
    let ready!: (i: ServerInfo) => void;
    const info = new Promise<ServerInfo>((r) => (ready = r));
    const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
    try {
      const c = await connectProved({ info: await info, serverData }, "console");
      const status = ok<{ warnings: Array<Record<string, unknown>> }>(await c.call("server.status", {}));
      expect(status.warnings.find((w) => w.code === "SECRETS_FILE_STORE")).toMatchObject({ reason: "requested" });
      await c.close();
    } finally {
      await Effect.runPromise(Fiber.interrupt(fiber));
    }
    // The key and its pin persist.
    const db = openControlStore(serverData, { serverId: "sid-x", checkpoint: false });
    const pin = (db.db.prepare("SELECT value FROM meta WHERE key = ?").get(IDENTITY_SPKI_META) as { value: string }).value;
    expect(spkiPin(openVault(serverData, db.envId, fileEnv).get("identity")!.toString())).toBe(pin);
    // `openSecrets` gives the same identity the server used.
    const audit = new Audit(db);
    expect(openSecrets({ serverData, store: db, audit, devices: new DeviceStore(db, audit), env: fileEnv }).identity?.spki).toBe(pin);
    await db.close();
  });
});

