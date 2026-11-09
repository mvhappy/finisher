// 5B Task 9: revoking a device (Decisions 24, 25): immediate everywhere (its live WebSocket
// subscriptions end within 100 ms, its tokens and tickets stop), by the owner or by itself;
// `auth.devices.watch` and the device RPCs, audited with the name only where allowed.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { serve, StderrLogger } from "../src/main.ts";
import { pairDevice } from "./support/device.ts";
import { confirmed, connectProved, generationOf, type NdjsonClient, ok, type ServerInfo } from "./support/ndjson-client.ts";
import { openWs } from "./support/ws-client.ts";

let tmp: string;
let fiber: Fiber.Fiber<unknown, unknown> | undefined;
let server: { info: ServerInfo; serverData: string };
const clients: NdjsonClient[] = [];

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-device-revoke-"));
  const serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  server = { info: await info, serverData };
});

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  if (fiber !== undefined) await Effect.runPromise(Fiber.interrupt(fiber));
  fiber = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function asConsole(): Promise<NdjsonClient> {
  const c = await connectProved(server, "console");
  clients.push(c);
  return c;
}

const code = (e: { ok: boolean; error?: { code: string } }) => (e.ok ? undefined : e.error!.code);

describe("revoking a device", () => {
  test("revoke_closes_live_subscription_within_100ms", async () => {
    const c = await asConsole();
    const projectId = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    const { device, deviceId } = await pairDevice(server);
    const generation = await generationOf(c, projectId);
    const sock = await device.rawWs();
    const closed = new Promise<number>((resolve) => sock.once("close", () => resolve(performance.now())));
    sock.send(`${JSON.stringify({ _tag: "Request", id: "1", tag: "ops.subscribe", payload: { projectId, branch: "main", fromRev: 0, generation }, headers: [] })}\n`);
    await new Promise((r) => setTimeout(r, 100));
    const at = performance.now();
    expect(ok(await confirmed(c, "auth.device.revoke", { id: deviceId }))).toEqual({ revoked: true });
    expect((await closed) - at).toBeLessThan(100);
    // Revoking again changes nothing.
    expect(ok(await confirmed(c, "auth.device.revoke", { id: deviceId }))).toEqual({ revoked: false });
  });

  test("revoke_drops_tokens_and_tickets; revoked_device_token_is_device_revoked", async () => {
    const c = await asConsole();
    const { device, deviceId } = await pairDevice(server);
    const token = await device.token();
    const ticket = await device.ticket(token);
    ok(await confirmed(c, "auth.device.revoke", { id: deviceId }));
    // The ticket taken before is refused at the upgrade; the token and the key say why.
    await expect(openWs(server.info.wsPort, ["finisher.v2", `finisher.ticket.${ticket}`])).rejects.toMatchObject({ status: 401 });
    const again = await device.call("/auth/token", {});
    expect([again.status, (again.body.error as { code: string }).code]).toEqual([401, "DEVICE_REVOKED"]);
    const withToken = await device.call("/auth/ws-ticket", {}, token);
    expect((withToken.body.error as { code: string }).code).toBe("DEVICE_REVOKED");
  });

  test("revoke_self_from_device", async () => {
    const { device, deviceId } = await pairDevice(server);
    const ws = await device.ws();
    const other = await device.rawWs();
    const otherClosed = new Promise<void>((r) => other.once("close", () => r()));
    const r = (await ws.call("auth.revokeSelf", {})).exit as { _tag: string };
    expect(r._tag).toBe("Success");
    // Its other WebSocket ends at once; the caller's closes after the reply.
    await otherClosed;
    const after = await device.call("/auth/token", {});
    expect((after.body.error as { code: string }).code).toBe("DEVICE_REVOKED");
    const c = await asConsole();
    const all = ok<{ devices: Array<{ id: string; revokedAt?: number }> }>(await c.call("auth.device.list", { all: true })).devices;
    expect(all.find((d) => d.id === deviceId)?.revokedAt).toBeDefined();
    expect(ok<{ devices: unknown[] }>(await c.call("auth.device.list", {})).devices).toEqual([]);
    const audit = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await c.call("auth.audit.list", { kind: "device.revoke" })).entries;
    expect(audit[0]!.details).toMatchObject({ deviceId, deviceName: "Test tablet", reason: "self" });
  });

  test("a device revoking itself by id gets its answer (review of Task 9)", async () => {
    const { device, deviceId } = await pairDevice(server);
    const ws = await device.ws();
    const r = (await ws.call("auth.device.revoke", { id: deviceId })).exit as { _tag: string; value: { revoked: boolean } };
    expect(r).toMatchObject({ _tag: "Success", value: { revoked: true } });
    expect(((await device.call("/auth/token", {})).body.error as { code: string }).code).toBe("DEVICE_REVOKED");
  });

  test("devices_watch_snapshot_and_changes; rename_audited_with_id_only", async () => {
    const c = await asConsole();
    const { device: first, deviceId: firstId } = await pairDevice(server, "First");
    const items: Array<{ kind: string } & Record<string, unknown>> = [];
    const watch = c.stream("auth.devices.watch", {}, (v) => items.push(v as never));
    await new Promise((r) => setTimeout(r, 100));
    const { deviceId: secondId } = await pairDevice(server, "Second");
    ok(await confirmed(c, "auth.device.rename", { id: firstId, name: "Edit bay" }));
    await first.token();
    ok(await confirmed(c, "auth.device.revoke", { id: secondId }));
    await new Promise((r) => setTimeout(r, 200));
    expect(items.map((i) => i.kind)).toEqual(["snapshot", "added", "renamed", "seen", "revoked"]);
    expect((items[0]!.devices as Array<{ id: string }>).map((d) => d.id)).toEqual([firstId]);
    expect(items[1]).toMatchObject({ device: { id: secondId, name: "Second" } });
    expect(items[2]).toMatchObject({ device: { id: firstId, name: "Edit bay" } });
    expect(items[3]).toMatchObject({ id: firstId, route: "loopback" });
    expect(items[4]).toEqual({ kind: "revoked", id: secondId });
    const renames = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await c.call("auth.audit.list", { kind: "device.rename" })).entries;
    expect(renames.map((e) => e.details)).toEqual([{ deviceId: firstId }]);
    expect(code(await c.call("auth.device.rename", { id: firstId, name: "x" }))).toBe("CONFIRMATION_REQUIRED");
    void watch;
  });
});
