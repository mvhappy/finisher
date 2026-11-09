// 5B Task 9: device access tokens and tickets (Decision 16), units and over a server in this
// process: tokens need a known live key, are bound to it, live 900 s, 4 per device; tickets
// need `ath`; a device's WebSocket is the device principal.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { DeviceTokens, MAX_TOKENS_PER_DEVICE } from "../src/auth/device-tokens.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { deviceKey, pairDevice, post, TestDevice } from "./support/device.ts";
import { confirmed, connectProved, ok, type ServerInfo } from "./support/ndjson-client.ts";

describe("DeviceTokens (units)", () => {
  test("token_expires_at_900s; four_tokens_per_device; bound to the key", () => {
    let now = 1_800_000_000_000;
    const t = new DeviceTokens(() => now);
    const d = { id: "01device00000000000000000a", jkt: "jkt-a" };
    const first = t.issue(d);
    expect(first.expiresIn).toBe(900);
    expect(first.accessToken).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(t.check(first.accessToken, "jkt-a")).toEqual({ deviceId: d.id });
    expect(t.check(first.accessToken, "jkt-b")).toEqual({ refused: "DPOP_INVALID" });
    now += 899_999;
    expect(t.check(first.accessToken, "jkt-a")).toEqual({ deviceId: d.id });
    now += 1;
    expect(t.check(first.accessToken, "jkt-a")).toEqual({ refused: "TOKEN_EXPIRED" });
    const tokens = Array.from({ length: MAX_TOKENS_PER_DEVICE + 1 }, () => t.issue(d).accessToken);
    expect(t.check(tokens[0]!, "jkt-a")).toEqual({ refused: "TOKEN_EXPIRED" });
    for (const tok of tokens.slice(1)) expect(t.check(tok, "jkt-a")).toEqual({ deviceId: d.id });
    expect(t.dropDevice(d.id)).toBe(MAX_TOKENS_PER_DEVICE);
    expect(t.check(tokens[1]!, "jkt-a")).toEqual({ refused: "DEVICE_REVOKED" });
    expect(t.check("never-issued-token-xxxxxxxxxxxxxxxxx", "jkt-a")).toEqual({ refused: "TOKEN_EXPIRED" });
  });
});

let tmp: string;
let serverData: string;
let fiber: Fiber.Fiber<unknown, unknown> | undefined;
let server: { info: ServerInfo; serverData: string };

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-device-tokens-"));
  serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  server = { info: await info, serverData };
});

afterEach(async () => {
  if (fiber !== undefined) await Effect.runPromise(Fiber.interrupt(fiber));
  fiber = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

describe("over the loopback listener", () => {
  test("token_needs_known_live_key", async () => {
    const stranger = new TestDevice(server.info.wsPort, await deviceKey());
    const got = await stranger.call("/auth/token", {});
    expect(got.status).toBe(401);
    expect(got.body).toMatchObject({ error: { code: "DPOP_INVALID", details: { reason: "unknownKey" } } });
    // Without a proof at all.
    expect((await post(server.info.wsPort, "/auth/token", {})).status).toBe(401);
    const { device } = await pairDevice(server);
    expect(await device.token()).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  test("token_needs_matching_jkt and ws_ticket_needs_ath", async () => {
    const { device } = await pairDevice(server, "One");
    const { device: other } = await pairDevice(server, "Two");
    const token = await device.token();
    // Another device's key with the first one's token.
    const stolen = await other.call("/auth/ws-ticket", {}, token);
    expect(stolen.status).toBe(401);
    expect((stolen.body.error as { code: string }).code).toBe("DPOP_INVALID");
    // A proof without `ath`.
    const proof = await device.proof("/auth/ws-ticket");
    const noAth = await post(server.info.wsPort, "/auth/ws-ticket", {}, { dpop: proof, authorization: `DPoP ${token}` });
    expect(noAth.status).toBe(401);
    expect(noAth.body).toMatchObject({ error: { code: "DPOP_INVALID", details: { reason: "ath" } } });
    // No token.
    expect((await device.call("/auth/ws-ticket", {})).status).toBe(401);
    const ticket = await device.ticket(token);
    expect(ticket.length).toBeGreaterThan(20);
  });

  test("device_ws_is_device_principal", async () => {
    const { device, deviceId } = await pairDevice(server);
    const ws = await device.ws();
    const me = (await ws.call("auth.whoami", {})).exit as { _tag: string; value: { principal: Record<string, unknown> } };
    expect(me._tag).toBe("Success");
    expect(me.value.principal).toMatchObject({ kind: "device", id: deviceId, projects: "*", branches: "*", human: true });
    // The owner here: a human-only call needs no code, `server.stop` would.
    const list = (await ws.call("auth.device.list", {})).exit as { _tag: string; value: { devices: Array<{ id: string; lastRoute?: string }> } };
    expect(list.value.devices.map((d) => d.id)).toEqual([deviceId]);
    expect(list.value.devices[0]!.lastRoute).toBe("loopback");
    const stop = (await ws.call("server.stop", {})).exit as { _tag: string; cause?: unknown };
    expect(stop._tag).toBe("Failure");
    expect(JSON.stringify(stop)).toContain("CONFIRMATION_REQUIRED");
    ws.close();
  });

  test("token requests are rate limited per device, before anything is remembered (review of Task 9)", async () => {
    const { device } = await pairDevice(server);
    let limited: Awaited<ReturnType<typeof device.call>> | undefined;
    for (let i = 0; i < 32 && limited === undefined; i++) {
      const got = await device.call("/auth/token", {});
      if (got.status === 429) limited = got;
    }
    expect(limited).toBeDefined();
    expect((limited!.body.error as { code: string }).code).toBe("TOKEN_RATE_LIMITED");
    expect(Number(limited!.headers["retry-after"])).toBeGreaterThan(0);
  });

  test("a device's human-only calls need no code; its source is the app's", async () => {
    const { device, deviceId } = await pairDevice(server, "Mine");
    const ws = await device.ws();
    const renamed = (await ws.call("auth.device.rename", { id: deviceId, name: "Edit bay" })).exit as { _tag: string; value: { device: { name: string } } };
    expect(renamed._tag).toBe("Success");
    expect(renamed.value.device.name).toBe("Edit bay");
    const offer = (await ws.call("auth.pair.offer", {})).exit as { _tag: string };
    expect(offer._tag).toBe("Success");
    // The diagnostics source of a device's calls is the app's (5A's `sourceOf`).
    const { sourceOf } = await import("../src/diagnostics.ts");
    expect(sourceOf({ kind: "device", id: deviceId, scopes: [], projects: "*", branches: "*", human: true })).toBe("ui");
    ws.close();
  });

  test("moved_env_refuses_tokens", async () => {
    const { device } = await pairDevice(server);
    // The server data marked moved behind the server's back is read live by the token route.
    const c = await connectProved(server, "console");
    ok(await confirmed(c, "env.name", { name: "Edit suite" }));
    await c.close();
    const { openControlStore } = await import("../src/control/db.ts");
    const store = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    store.db.prepare("INSERT INTO meta (key, value) VALUES ('envState', 'moved') ON CONFLICT (key) DO UPDATE SET value = excluded.value").run();
    store.db.prepare("DELETE FROM server_ids WHERE serverId = 'sid-test'").run();
    await store.close();
    const got = await device.call("/auth/token", {});
    expect(got.status).toBe(403);
    expect((got.body.error as { code: string }).code).toBe("ENV_MOVED");
  });
});
