// 5B Task 10: the TLS-only remote listener (Decisions 26–29, 34) on loopback: off by default,
// self-signed and pinned to the identity key, or the owner's files (reloaded, bad ones kept out),
// `settings.write` applying live, its Host list, `FIN_LISTEN`, a fixed loopback port.
import { createHash, generateKeyPairSync } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { request } from "node:https";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect as tlsConnect } from "node:tls";
import { generateIdentityKey, selfSignedCert } from "@finisher/fin-node";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { openControlStore } from "../src/control/db.ts";
import { serve, type ServeOptions, StderrLogger } from "../src/main.ts";
import type { RemoteListener } from "../src/net/remote.ts";
import { checkSetting, readServerSettings, remoteConfigOf } from "../src/net/server-settings.ts";
import { confirmed, connectProved, type NdjsonClient, ok, type ServerInfo } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
const fibers: Array<Fiber.Fiber<unknown, unknown>> = [];
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-remote-"));
  serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
});
afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  for (const f of fibers.splice(0)) await Effect.runPromise(Fiber.interrupt(f));
  rmSync(tmp, { recursive: true, force: true });
});

async function start(more: Partial<ServeOptions> = {}) {
  let ready!: (i: ServerInfo) => void;
  let remote: RemoteListener | undefined;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  fibers.push(Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo), onRemote: (r) => (remote = r), ...more }).pipe(Effect.provide(StderrLogger))));
  const server = { info: await info, serverData };
  const c = await connectProved(server, "console");
  clients.push(c);
  return { server, c, remote: () => remote! };
}

type Listener = { transport: string; endpoint: string; spki?: string; mode?: string; source?: string };
const listeners = async (c: NdjsonClient) => ok<{ listeners: Listener[]; warnings: Array<Record<string, unknown>> }>(await c.call("server.status", {}));
const wssPort = (l: Listener[]) => Number(new URL(l.find((x) => x.transport === "wss")!.endpoint.replace("wss://", "https://")).port);
const code = (e: { ok: boolean; error?: { code: string } }) => (e.ok ? undefined : e.error!.code);

/** The pin of the certificate a TLS client sees on `port`. */
function peerPin(port: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const s = tlsConnect({ host: "127.0.0.1", port, servername: "localhost", rejectUnauthorized: false }, () => {
      const der = s.getPeerX509Certificate()!.publicKey.export({ type: "spki", format: "der" });
      s.end();
      resolve(createHash("sha256").update(der).digest("base64url"));
    });
    s.on("error", reject);
  });
}

function get(port: number, path: string, host = `127.0.0.1:${port}`): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const r = request({ host: "127.0.0.1", port, path, method: "GET", headers: { host }, rejectUnauthorized: false }, (res) => {
      let t = "";
      res.on("data", (d) => (t += d));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body: t }));
    });
    r.on("error", reject);
    r.end();
  });
}

const freePort = () =>
  new Promise<number>((resolve) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => {
      const p = (s.address() as { port: number }).port;
      s.close(() => resolve(p));
    });
  });

function ownerFiles(name = "a") {
  const id = generateIdentityKey();
  const certFile = join(tmp, `${name}.crt`);
  const keyFile = join(tmp, `${name}.key`);
  writeFileSync(certFile, selfSignedCert(id.pem, ["127.0.0.1"], 90));
  writeFileSync(keyFile, id.pem);
  return { certFile, keyFile, spki: id.spki };
}

describe("the remote listener", () => {
  test("remote_off_by_default", async () => {
    const { c } = await start();
    expect((await listeners(c)).listeners.map((l) => l.transport)).not.toContain("wss");
  });

  test("self_signed_listener_pins_identity; status_lists_listeners_with_pin; the well-known subset over TLS", async () => {
    const { c } = await start({ remote: { forceTls: true, listen: "127.0.0.1:0" } });
    const status = await listeners(c);
    const wss = status.listeners.find((l) => l.transport === "wss")!;
    const env = ok<{ spki: string; envId: string }>(await c.call("env.describe", {}));
    expect(wss).toMatchObject({ spki: env.spki, mode: "selfSigned", source: "test" });
    const port = wssPort(status.listeners);
    expect(await peerPin(port)).toBe(env.spki);
    const wk = await get(port, "/.well-known/finisher");
    expect(wk.status).toBe(200);
    expect(JSON.parse(wk.body)).toMatchObject({ envId: env.envId, spki: env.spki });
    const audit = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await c.call("auth.audit.list", { kind: "listener.start" })).entries;
    expect(audit[0]!.details).toEqual({ spki: env.spki, mode: "selfSigned", source: "test" });
  });

  test("remote_host_allow_list", async () => {
    const { c } = await start({ remote: { forceTls: true, listen: "127.0.0.1:0" } });
    const port = wssPort((await listeners(c)).listeners);
    expect((await get(port, "/.well-known/finisher", "evil.example.test")).status).toBe(403);
    expect((await get(port, "/.well-known/finisher", `evil.example.test:${port}`)).status).toBe(403);
    expect((await get(port, "/.well-known/finisher", `localhost:${port}`)).status).toBe(200);
  });

  test("non_loopback_without_tls_refused (the check and settings.write)", async () => {
    const refused = () => checkSetting("server.remote", { enabled: true, listen: "0.0.0.0:7311" });
    expect(refused).toThrowError(expect.objectContaining({ code: "LISTEN_NEEDS_TLS" }));
    expect(checkSetting("server.remote", { enabled: true, listen: "127.0.0.1:7311" })).toBeDefined();
    const { c } = await start();
    expect(code(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "0.0.0.0:0" } }))).toBe("LISTEN_NEEDS_TLS");
    expect(() => readFileSync(join(serverData, "settings.json"), "utf8")).toThrow();
  });

  test("settings_write_applies_live, keeps other keys, is audited without the value", async () => {
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ fonts: { keep: true }, server: { origins: ["https://app.example.test"] } }));
    const { c } = await start();
    ok(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "127.0.0.1:0", tls: { mode: "selfSigned" } } }));
    const on = await listeners(c);
    expect(on.listeners.find((l) => l.transport === "wss")).toMatchObject({ mode: "selfSigned", source: "settings" });
    const first = wssPort(on.listeners);
    expect((await get(first, "/.well-known/finisher")).status).toBe(200);
    const next = await freePort();
    ok(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: `127.0.0.1:${next}`, tls: { mode: "selfSigned" } } }));
    expect(wssPort((await listeners(c)).listeners)).toBe(next);
    ok(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: false } }));
    expect((await listeners(c)).listeners.map((l) => l.transport)).not.toContain("wss");
    const file = JSON.parse(readFileSync(join(serverData, "settings.json"), "utf8"));
    expect(file).toMatchObject({ fonts: { keep: true }, server: { origins: ["https://app.example.test"], remote: { enabled: false } } });
    const rows = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await c.call("auth.audit.list", { kind: "settings.write" })).entries;
    expect(rows.map((r) => r.details)).toEqual([{ path: "server.remote" }, { path: "server.remote" }, { path: "server.remote" }]);
    expect(ok<{ entries: unknown[] }>(await c.call("auth.audit.list", { kind: "listener.stop" })).entries.length).toBeGreaterThan(0);
  });

  test("settings_write_refuses_unknown_paths and bad values; settings_write_is_human_only_and_confirmed", async () => {
    const { server, c } = await start();
    expect(code(await confirmed(c, "settings.write", { path: "privacy", value: {} }))).toBe("INVALID_REQUEST");
    expect(code(await confirmed(c, "settings.write", { path: "server.projectRoots", value: ["relative/path"] }))).toBe("SETTINGS_INVALID");
    expect(code(await confirmed(c, "settings.write", { path: "server.loopback", value: { port: 70000 } }))).toBe("SETTINGS_INVALID");
    expect(code(await c.call("settings.write", { path: "server.origins", value: [] }))).toBe("CONFIRMATION_REQUIRED");
    const local = await connectProved(server, "local");
    clients.push(local);
    expect(code(await local.call("settings.write", { path: "server.origins", value: [] }))).toBe("HUMAN_ONLY");
  });

  test("files_mode_serves_owner_cert_and_reloads; bad_files_keep_old_context_and_warn", async () => {
    const a = ownerFiles("a");
    const { c, remote } = await start();
    const bad = await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "127.0.0.1:0", tls: { mode: "files", certFile: join(tmp, "missing.crt"), keyFile: a.keyFile } } });
    expect(code(bad)).toBe("TLS_FILES_INVALID");
    ok(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "127.0.0.1:0", tls: { mode: "files", certFile: a.certFile, keyFile: a.keyFile } } }));
    const port = wssPort((await listeners(c)).listeners);
    expect(await peerPin(port)).toBe(a.spki);
    // Renewed files are read again.
    const b = ownerFiles("b");
    writeFileSync(a.certFile, readFileSync(b.certFile));
    writeFileSync(a.keyFile, readFileSync(b.keyFile));
    remote().reload();
    expect(await peerPin(port)).toBe(b.spki);
    // A pair that does not match keeps the old context, and says so.
    const other = generateKeyPairSync("ec", { namedCurve: "P-256" }).privateKey.export({ type: "pkcs8", format: "pem" }) as string;
    writeFileSync(a.keyFile, other);
    remote().reload();
    expect(await peerPin(port)).toBe(b.spki);
    expect((await listeners(c)).warnings).toContainEqual(expect.objectContaining({ code: "TLS_FILES_INVALID", reason: "keyMismatch" }));
  });

  test("a wss RPC session outlives the header timeout; the per-address cap and Origin hold (review of Task 10)", { timeout: 40_000 }, async () => {
    const { c, server } = await start({ remote: { forceTls: true, listen: "127.0.0.1:0", remoteOf: (() => "remote-a") as never } });
    const port = wssPort((await listeners(c)).listeners);
    const { pairDevice } = await import("./support/device.ts");
    const { WebSocket } = await import("ws");
    const { device } = await pairDevice(server);
    const ticket = await device.ticket();
    const ws = new WebSocket(`wss://127.0.0.1:${port}/rpc`, ["finisher.v2", `finisher.ticket.${ticket}`], { rejectUnauthorized: false });
    await new Promise<void>((r, j) => (ws.once("open", () => r()), ws.once("error", j)));
    let closed = false;
    ws.on("close", () => (closed = true));
    await new Promise((r) => setTimeout(r, 11_000));
    expect(closed).toBe(false);
    const reply = new Promise<string>((r) => ws.once("message", (d) => r(String(d))));
    ws.send(`${JSON.stringify({ _tag: "Request", id: "1", tag: "auth.whoami", payload: {}, headers: [] })}\n`);
    expect(await reply).toContain('"kind":"device"');
    // Every socket counts as one address here: past 4 waiting ones, a fifth is closed.
    const { connect } = await import("node:net");
    const held = await Promise.all(
      Array.from({ length: 5 }, () => new Promise<{ open: () => boolean; end: () => void }>((r) => {
        let open = true;
        const s = connect({ host: "127.0.0.1", port }, () => r({ open: () => open, end: () => s.destroy() }));
        s.on("close", () => (open = false));
        s.on("error", () => (open = false));
      })),
    );
    await new Promise((r) => setTimeout(r, 150));
    expect(held.filter((h) => h.open())).toHaveLength(4);
    for (const h of held) h.end();
    ws.close();
    await new Promise((r) => setTimeout(r, 150));
    // A page elsewhere is refused by Origin.
    const refused = await new Promise<number>((resolve, reject) => {
      const r = request({ host: "127.0.0.1", port, path: "/.well-known/finisher", headers: { host: `127.0.0.1:${port}`, origin: "https://evil.example.test" }, rejectUnauthorized: false }, (res) => (res.resume(), resolve(res.statusCode ?? 0)));
      r.on("error", reject);
      r.end();
    });
    expect(refused).toBe(403);
  });

  test("FIN_LISTEN through serve; a failed apply restores the file and the listener", async () => {
    const saved = process.env.FIN_LISTEN;
    process.env.FIN_LISTEN = "127.0.0.1:0";
    try {
      const { c } = await start();
      expect((await listeners(c)).listeners.find((l) => l.transport === "wss")).toMatchObject({ source: "env", mode: "selfSigned" });
    } finally {
      if (saved === undefined) delete process.env.FIN_LISTEN;
      else process.env.FIN_LISTEN = saved;
    }
    for (const f of fibers.splice(0)) await Effect.runPromise(Fiber.interrupt(f));
    for (const x of clients.splice(0)) await x.close();
    const { c } = await start();
    ok(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "127.0.0.1:0", tls: { mode: "selfSigned" } } }));
    const port = wssPort((await listeners(c)).listeners);
    // A port someone else holds: the change fails, and the old listener and file stay.
    const blocker = createServer();
    await new Promise<void>((r) => blocker.listen(0, "127.0.0.1", () => r()));
    const taken = (blocker.address() as { port: number }).port;
    expect(code(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: `127.0.0.1:${taken}`, tls: { mode: "selfSigned" } } }))).toBe("SERVER_START_FAILED");
    await new Promise((r) => blocker.close(r));
    expect(JSON.parse(readFileSync(join(serverData, "settings.json"), "utf8")).server.remote.listen).toBe("127.0.0.1:0");
    expect((await listeners(c)).listeners.find((l) => l.transport === "wss")).toBeDefined();
    expect((await get(wssPort((await listeners(c)).listeners), "/.well-known/finisher")).status).toBe(200);
    void port;
  });

  test("listener_not_started_while_moved", async () => {
    const { c } = await start();
    const store = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    store.db.prepare("INSERT INTO meta (key, value) VALUES ('envState', 'moved') ON CONFLICT (key) DO UPDATE SET value = excluded.value").run();
    store.db.prepare("DELETE FROM server_ids WHERE serverId = 'sid-test'").run();
    await store.close();
    expect(code(await confirmed(c, "settings.write", { path: "server.remote", value: { enabled: true, listen: "127.0.0.1:0", tls: { mode: "selfSigned" } } }))).toBe("ENV_MOVED");
    expect((await listeners(c)).listeners.map((l) => l.transport)).not.toContain("wss");
    // Not applied, so the file holds no remote section.
    expect(JSON.parse(readFileSync(join(serverData, "settings.json"), "utf8")).server?.remote).toBeUndefined();
  });

  test("fin_listen_wins_over_settings", () => {
    const settings = { remote: { enabled: false }, projectRoots: [], invalid: [] };
    expect(remoteConfigOf(settings, {})).toBeUndefined();
    expect(remoteConfigOf(settings, { FIN_LISTEN: "0.0.0.0:7311" })).toMatchObject({ listen: "0.0.0.0:7311", tls: { mode: "selfSigned" }, source: "env" });
    expect(remoteConfigOf(settings, { FIN_LISTEN: "0.0.0.0:7311", FIN_TLS_CERT: "/c.pem", FIN_TLS_KEY: "/k.pem" })).toMatchObject({ tls: { mode: "files", certFile: "/c.pem", keyFile: "/k.pem" } });
    expect(remoteConfigOf({ ...settings, remote: { enabled: true, tls: { mode: "selfSigned" } } }, {})).toMatchObject({ listen: "0.0.0.0:7311", source: "settings" });
  });

  test("fixed_loopback_port; an invalid section is ignored with a warning", async () => {
    const port = await freePort();
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ server: { loopback: { port }, remote: { enabled: "yes" } } }));
    expect(readServerSettings(serverData)).toMatchObject({ loopbackPort: port, invalid: [{ key: "server.remote", code: "SETTINGS_INVALID" }] });
    const { server, c } = await start();
    expect(server.info.wsPort).toBe(port);
    expect((await listeners(c)).warnings).toContainEqual(expect.objectContaining({ code: "SETTINGS_INVALID", key: "server.remote" }));
  });
});
