// 5B Task 5: the environment descriptor, copied or moved server data, adopting, resetting and
// rotating the identity (Decisions 30, 31), on servers in this process.
import { randomBytes } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FinError, type Principal, SCOPES } from "@finisher/contracts";
import { generateIdentityKey, spkiPin } from "@finisher/fin-node";
import { Effect, Exit, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { DeviceStore } from "../src/auth/devices.ts";
import { revokeDevice } from "../src/auth/revoke.ts";
import { withChain } from "../src/chain/chain.ts";
import { Connections } from "../src/connections.ts";
import type { Audit, AuditInput } from "../src/control/audit.ts";
import { openControlStore } from "../src/control/db.ts";
import { envMoved } from "../src/env/descriptor.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { IDENTITY_SPKI_META, openVault, recoverStaged, STAGED_DEK, STAGED_IDENTITY } from "../src/secrets/vault.ts";
import { confirmed, connectProved, hello, NdjsonClient, ok, type Exit as RpcExit, type ServerInfo } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
const running: Array<Fiber.Fiber<unknown, unknown>> = [];
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-env-"));
  serverData = join(tmp, "a");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  for (const f of running.splice(0)) await Effect.runPromise(Fiber.interrupt(f));
  rmSync(tmp, { recursive: true, force: true });
});

async function start(sd = serverData): Promise<{ info: ServerInfo; serverData: string; stop: () => Promise<void> }> {
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(serve({ serverData: sd, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  running.push(fiber);
  return {
    info: await info,
    serverData: sd,
    stop: async () => {
      running.splice(running.indexOf(fiber), 1);
      await Effect.runPromise(Fiber.interrupt(fiber));
    },
  };
}

async function as(server: { info: ServerInfo; serverData: string }, who: "console" | "local"): Promise<NdjsonClient> {
  const c = await connectProved(server, who);
  clients.push(c);
  return c;
}

const codeOf = (e: RpcExit) => (e.ok ? undefined : e.error.code);
const fileInfo = (sd: string) => JSON.parse(readFileSync(join(sd, "finisherd.json"), "utf8")) as ServerInfo & { envId: string };
const DEVICE: Principal = { kind: "device", id: "01device00000000000000000a", scopes: [...SCOPES], projects: "*", branches: "*", human: true };

/** Adds paired devices to a stopped server's data, as an approval would. */
async function addDevices(sd: string, n: number): Promise<string[]> {
  const store = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
  const audit = { append() {}, failures: () => 0 } as unknown as Audit;
  const devices = new DeviceStore(store, audit);
  const ids = Array.from({ length: n }, (_, i) => devices.add({ name: `Device ${i}`, platform: "ios", jwk: { kty: "EC", crv: "P-256", x: `x${i}`, y: `y${i}` }, jkt: `jkt-${i}`, approvedBy: "console:console" }).id);
  // This helper is no server: it leaves no server id behind.
  store.db.prepare("DELETE FROM server_ids WHERE serverId = 'sid-test'").run();
  await store.close();
  return ids;
}

const liveDevices = async (sd: string) => {
  const store = openControlStore(sd, { serverId: "sid-test", checkpoint: false });
  const rows = store.db.prepare("SELECT id, revokedReason FROM devices").all() as Array<{ id: string; revokedReason: string | null }>;
  const audit = store.db.prepare("SELECT kind, details FROM audit WHERE kind IN ('identity.rotate', 'env.reset', 'env.moved', 'env.adopted') ORDER BY id").all() as Array<{ kind: string; details: string }>;
  const pin = (store.db.prepare("SELECT value FROM meta WHERE key = ?").get(IDENTITY_SPKI_META) as { value: string }).value;
  store.db.prepare("DELETE FROM server_ids WHERE serverId = 'sid-test'").run();
  await store.close();
  return { rows, audit: audit.map((a) => ({ kind: a.kind, details: JSON.parse(a.details) })), pin };
};

describe("the descriptor", () => {
  test("describe_matches_well_known_subset", async () => {
    const s = await start();
    const c = await as(s, "console");
    const env = ok<{ envId: string; name: string; spki: string; serverIds: string[]; protocol: { min: number; max: number }; routes: unknown[]; state: string; capabilities: string[] }>(
      await c.call("env.describe", {}),
    );
    expect(env.envId).toBe(s.info.envId);
    expect(env.spki).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(env).toMatchObject({ name: "Finisher", protocol: { min: 2, max: 2 }, state: "ok" });
    expect(env.serverIds).toHaveLength(1);
    expect(env.routes).toEqual([{ kind: "loopback", url: `http://127.0.0.1:${s.info.wsPort}`, pin: "identity" }]);
    expect(env.capabilities).toContain("env.describe");
    // `server.status` carries the same state, pin and routes.
    const status = ok<{ env: { state: string; spki: string; routes: unknown[] } }>(await c.call("server.status", {}));
    expect(status.env).toEqual({ state: "ok", spki: env.spki, routes: env.routes });
    // The pin is the vault's identity key's.
    await s.stop();
    const store = openControlStore(serverData, { serverId: "sid-x", checkpoint: false });
    expect(spkiPin(openVault(serverData, store.envId, { FIN_SECRETS: "file" }).get("identity")!.toString())).toBe(env.spki);
    await store.close();
  });

  test("env.name is the owner's, 1–64 characters, with the typed code", async () => {
    const s = await start();
    const c = await as(s, "console");
    expect(codeOf(await c.call("env.name", { name: "Edit suite" }))).toBe("CONFIRMATION_REQUIRED");
    expect(ok<{ name: string }>(await confirmed(c, "env.name", { name: "Edit suite" })).name).toBe("Edit suite");
    expect(codeOf(await confirmed(c, "env.name", { name: "x".repeat(65) }))).toBe("INVALID_NAME");
    expect(ok<{ name: string }>(await c.call("env.describe", {})).name).toBe("Edit suite");
  });

  test("env_writes_are_human_only_and_confirmed", async () => {
    const s = await start();
    const local = await as(s, "local");
    // `local` reads the descriptor; it changes nothing.
    expect(codeOf(await local.call("env.describe", {}))).toBeUndefined();
    for (const tag of ["env.name", "env.adopt", "env.reset", "env.rotateIdentity"]) {
      expect(codeOf(await local.call(tag, tag === "env.name" ? { name: "x" } : {})), tag).toBe("HUMAN_ONLY");
    }
    const c = await as(s, "console");
    for (const tag of ["env.adopt", "env.reset", "env.rotateIdentity"]) expect(codeOf(await c.call(tag, {})), tag).toBe("CONFIRMATION_REQUIRED");
  });
});

describe("moved server data", () => {
  test("renewed_server_id_marks_moved, then adopt_keeps_env_and_devices", async () => {
    const first = await start();
    const envId = first.info.envId;
    await first.stop();
    await addDevices(serverData, 2);
    // A copy of the server data elsewhere: its id renews, and the original's id ran its env.
    const copy = join(tmp, "b");
    cpSync(serverData, copy, { recursive: true });
    rmSync(join(copy, "finisherd.json"), { force: true });
    const moved = await start(copy);
    const c = await as(moved, "console");
    const env = ok<{ envId: string; state: string; serverIds: string[] }>(await c.call("env.describe", {}));
    expect(env).toMatchObject({ envId, state: "moved" });
    expect(env.serverIds).toHaveLength(2);
    const status = ok<{ env: { state: string }; warnings: Array<{ code: string }> }>(await c.call("server.status", {}));
    expect(status.env.state).toBe("moved");
    expect(status.warnings.map((w) => w.code)).toContain("ENV_MOVED");
    // `local` and `console` keep working.
    expect(codeOf(await (await as(moved, "local")).call("server.status", {}))).toBeUndefined();
    // It was a move: the owner adopts it; the environment and its devices stay.
    expect(ok<{ state: string; envId: string }>(await confirmed(c, "env.adopt", {}))).toMatchObject({ state: "ok", envId });
    await moved.stop();
    const after = await liveDevices(copy);
    expect(after.rows.filter((r) => r.revokedReason === null)).toHaveLength(2);
    expect(after.audit.map((a) => a.kind)).toEqual(["env.moved", "env.adopted"]);
    expect(after.audit[0]!.details).toMatchObject({ reason: "otherDirectory" });
    // A second start of the adopted copy is not moved again.
    const again = await start(copy);
    expect(ok<{ state: string }>(await (await as(again, "console")).call("env.describe", {})).state).toBe("ok");
  });

  test("moved_refuses_devices_and_keeps_local (the chain's refusal)", async () => {
    let moved = true;
    const connections = new Connections("ws", () => "ws://127.0.0.1:1");
    connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(1)!.principal = DEVICE;
    connections.attach(2, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(2)!.principal = { ...DEVICE, kind: "console", id: "console" };
    const audited: AuditInput[] = [];
    const chained = withChain(
      { "fonts.status": () => Effect.succeed({}) },
      {
        connections,
        projects: { isOpen: () => false, dirOf: () => undefined, resolveBranch: () => Effect.succeed("main"), branches: () => Effect.succeed([]) },
        audit: { append: (e: AuditInput) => audited.push(e) } as unknown as Audit,
        refuse: (p) => (p.kind === "device" && moved ? envMoved() : undefined),
      },
    ) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
    const call = async (client: number) => {
      const exit = await Effect.runPromiseExit(chained["fonts.status"]!({}, { client: { id: client }, headers: {} }));
      return Exit.isFailure(exit) && exit.cause.reasons[0]?._tag === "Fail" ? (exit.cause.reasons[0].error as FinError).code : undefined;
    };
    expect(await call(1)).toBe("ENV_MOVED");
    expect(await call(2)).toBeUndefined();
    expect(envMoved().fix).not.toMatch(/finisherd|`/);
    moved = false;
    expect(await call(1)).toBeUndefined();
  });
});

describe("reset and rotation", () => {
  test("reset_mints_env_drops_devices_and_credentials", async () => {
    const first = await start();
    await first.stop();
    await addDevices(serverData, 2);
    const s = await start();
    const c = await as(s, "console");
    const minted = ok<{ file: { env: string; id: string; kind: string } }>(await confirmed(c, "auth.credential.mint", { kind: "agent", label: "before reset" }));
    const before = ok<{ envId: string; spki: string }>(await c.call("env.describe", {}));
    const reset = ok<{ envId: string; devices: number; credentials: number }>(await confirmed(c, "env.reset", {}));
    expect(reset.envId).not.toBe(before.envId);
    expect(reset).toMatchObject({ devices: 2, credentials: 1 });
    const after = ok<{ envId: string; spki: string; serverIds: string[] }>(await c.call("env.describe", {}));
    expect(after.envId).toBe(reset.envId);
    expect(after.spki).not.toBe(before.spki);
    expect(after.serverIds).toHaveLength(1);
    // `finisherd.json` names the new environment, and new connections prove against it.
    expect(fileInfo(serverData).envId).toBe(reset.envId);
    const fresh = await as({ info: fileInfo(serverData), serverData }, "local");
    expect(codeOf(await fresh.call("server.status", {}))).toBeUndefined();
    // The old credential is revoked, and its file names an environment this server no longer is.
    const listed = ok<{ credentials: Array<{ id: string; status: string }> }>(await c.call("auth.credential.list", { all: true }));
    expect(listed.credentials.find((x) => x.id === minted.file.id)?.status).toBe("revoked");
    expect(minted.file.env).toBe(before.envId);
    const probe = await NdjsonClient.connect(s.info.socket);
    clients.push(probe);
    const answer = ok<{ proof?: string; envId: string }>((await hello(probe, { env: minted.file.env, credential: { kind: "agent", id: minted.file.id } })).exit);
    expect(answer.envId).toBe(reset.envId);
    // Bound to the old environment, the hello gets no proof.
    expect(answer.proof).toBeUndefined();
    await s.stop();
    const stored = await liveDevices(serverData);
    expect(stored.rows).toEqual([]);
    expect(stored.audit.at(-1)).toEqual({ kind: "env.reset", details: { devices: 2, credentials: 1 } });
    expect(stored.pin).toBe(after.spki);
    // The restarted server keeps the new environment and its key.
    const again = await start();
    expect(ok<{ envId: string; spki: string }>(await (await as(again, "console")).call("env.describe", {}))).toMatchObject({ envId: reset.envId, spki: after.spki });
  });

  test("rotate_identity_revokes_every_device_and_changes_pin", async () => {
    const first = await start();
    await first.stop();
    const ids = await addDevices(serverData, 3);
    const s = await start();
    const c = await as(s, "console");
    const before = ok<{ envId: string; spki: string }>(await c.call("env.describe", {}));
    const rotated = ok<{ spki: string; revoked: number }>(await confirmed(c, "env.rotateIdentity", {}));
    expect(rotated.revoked).toBe(3);
    expect(rotated.spki).not.toBe(before.spki);
    const after = ok<{ envId: string; spki: string }>(await c.call("env.describe", {}));
    expect(after).toEqual(expect.objectContaining({ envId: before.envId, spki: rotated.spki }));
    await s.stop();
    const stored = await liveDevices(serverData);
    expect(stored.rows.map((r) => [r.id, r.revokedReason])).toEqual(ids.map((id) => [id, "rotation"]));
    expect(stored.audit).toEqual([{ kind: "identity.rotate", details: { reason: "owner", count: 3 } }]);
    expect(stored.pin).toBe(rotated.spki);
  });
});

describe("crash safety (review of Task 5)", () => {
  test("a rotation that crashed after its commit promotes the staged key at the next start", async () => {
    const first = await start();
    const before = ok<{ spki: string }>(await (await as(first, "console")).call("env.describe", {}));
    await first.stop();
    // What a crash between the commit and the promotion leaves: the new key staged, its pin recorded.
    const store = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    const vault = openVault(serverData, store.envId, { FIN_SECRETS: "file" });
    const fresh = generateIdentityKey();
    vault.set(STAGED_IDENTITY, Buffer.from(fresh.pem));
    store.db.prepare("UPDATE meta SET value = ? WHERE key = ?").run(fresh.spki, IDENTITY_SPKI_META);
    store.db.prepare("DELETE FROM server_ids WHERE serverId = 'sid-test'").run();
    await store.close();
    const s = await start();
    const after = ok<{ spki: string }>(await (await as(s, "console")).call("env.describe", {}));
    expect(after.spki).toBe(fresh.spki);
    expect(after.spki).not.toBe(before.spki);
  });

  test("recoverStaged promotes only what committed", () => {
    const vault = openVault(serverData, "01envenvenvenvenvenvenvenv", { FIN_SECRETS: "file" });
    const old = generateIdentityKey();
    const fresh = generateIdentityKey();
    vault.set("identity", Buffer.from(old.pem));
    vault.set("secrets-dek", Buffer.alloc(32, 1));
    // Never committed: the recorded pin is still the old one.
    vault.set(STAGED_IDENTITY, Buffer.from(fresh.pem));
    vault.set(STAGED_DEK, Buffer.alloc(32, 2));
    expect(recoverStaged(vault, old.spki)).toBe("discarded");
    expect(vault.get("identity")?.toString()).toBe(old.pem);
    expect(vault.get("secrets-dek")).toEqual(Buffer.alloc(32, 1));
    expect(vault.get(STAGED_IDENTITY)).toBeUndefined();
    // Committed: promoted with its data key.
    vault.set(STAGED_IDENTITY, Buffer.from(fresh.pem));
    vault.set(STAGED_DEK, Buffer.alloc(32, 2));
    expect(recoverStaged(vault, fresh.spki)).toBe("promoted");
    expect(vault.get("identity")?.toString()).toBe(fresh.pem);
    expect(vault.get("secrets-dek")).toEqual(Buffer.alloc(32, 2));
    expect(recoverStaged(vault, fresh.spki)).toBe("none");
  });

  test("a reset staged for another environment in a wrapped store is discarded unread", () => {
    const key = join(tmp, "key");
    writeFileSync(key, randomBytes(32).toString("base64url"));
    const env = { FIN_SECRETS: "file", FIN_SECRETS_KEY_FILE: key };
    const old = generateIdentityKey();
    const here = openVault(serverData, "01envenvenvenvenvenvenvenv", env);
    here.set("identity", Buffer.from(old.pem));
    // A reset that crashed before its commit: staged under the new environment's associated data.
    openVault(serverData, "01newnewnewnewnewnewnewnew", env).set(STAGED_IDENTITY, Buffer.from(generateIdentityKey().pem));
    expect(recoverStaged(here, old.spki)).toBe("discarded");
    expect(here.get("identity")?.toString()).toBe(old.pem);
  });

  test("reset and restart with a wrapped file store (FIN_SECRETS_KEY_FILE)", async () => {
    const key = join(tmp, "key");
    writeFileSync(key, randomBytes(32).toString("base64url"));
    const saved = process.env.FIN_SECRETS_KEY_FILE;
    process.env.FIN_SECRETS_KEY_FILE = key;
    try {
      const s = await start();
      const c = await as(s, "console");
      const reset = ok<{ envId: string }>(await confirmed(c, "env.reset", {}));
      const after = ok<{ spki: string }>(await c.call("env.describe", {}));
      await s.stop();
      const again = await start();
      expect(ok<{ envId: string; spki: string }>(await (await as(again, "console")).call("env.describe", {}))).toMatchObject({ envId: reset.envId, spki: after.spki });
    } finally {
      if (saved === undefined) delete process.env.FIN_SECRETS_KEY_FILE;
      else process.env.FIN_SECRETS_KEY_FILE = saved;
    }
  });

  test("revokeDevice drops a device's tokens and tickets and ends its connections", () => {
    const dropped: Principal[] = [];
    const revoked: string[] = [];
    const audited: unknown[] = [];
    let closed = 0;
    let tokensDropped = 0;
    const live = [{ principal: DEVICE }, { principal: { ...DEVICE, id: "01other0000000000000000000" } }];
    let ended = false;
    const deps = {
      tickets: { dropWhere: (pred: (p: Principal) => boolean) => [DEVICE].filter((p) => pred(p) && dropped.push(p)).length },
      connections: {
        revoke: (ref: { kind: string; id: string }) => void revoked.push(`${ref.kind}:${ref.id}`),
        closeWhere: (pred: (c: never) => boolean, error: FinError) => {
          expect(error.code).toBe("DEVICE_REVOKED");
          const n = live.filter(pred as (c: unknown) => boolean).length;
          closed += n;
          return n;
        },
      },
      audit: { append: (e: unknown) => void audited.push(e) } as unknown as Audit,
      devices: { get: () => ({ name: "Tablet" }), markEnded: () => !ended && (ended = true), persist: () => undefined },
      tokens: { dropDevice: () => ++tokensDropped },
    };
    const r = revokeDevice(deps as never, DEVICE.id, { reason: "owner", by: { kind: "console", id: "console" }, remote: "127.0.0.1" });
    expect(r).toEqual({ revoked: true, tickets: 1, connections: 1 });
    expect(revoked).toEqual([`device:${DEVICE.id}`]);
    expect([closed, tokensDropped]).toEqual([1, 1]);
    expect(audited).toEqual([
      { kind: "device.revoke", principal: { kind: "console", id: "console" }, outcome: "ok", details: { deviceId: DEVICE.id, deviceName: "Tablet", reason: "owner", count: 1, address: "127.0.0.1" } },
    ]);
    // A second revocation changes nothing and audits nothing.
    expect(revokeDevice(deps as never, DEVICE.id, { reason: "owner", by: { kind: "console", id: "console" } }).revoked).toBe(false);
    expect(audited).toHaveLength(1);
  });
});

