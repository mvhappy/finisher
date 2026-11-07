// The connection-aware protocol and the handshake RPCs (plan 5A Task 3): a real finisherd for the
// handshake, an in-process protocol with a small frame cap for the framing cases.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import * as NodeSocketServer from "@effect/platform-node/NodeSocketServer";
import { Effect, Fiber, Layer, Schema } from "effect";
import { Rpc, RpcGroup, RpcSerialization, RpcServer } from "effect/rpc";
import { SocketServer } from "effect/socket/SocketServer";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { b64u, canonicalEndpoint, proveMac } from "../src/auth/crypto.ts";
import { Connections } from "../src/connections.ts";
import { clientHeaders, LineSplitter, makeFinProtocol, salvageId } from "../src/protocol.ts";
import { controlDbPath } from "../src/control/db.ts";
import { hello, keyOf, NdjsonClient, ok, prove, type RunningServer, startServer } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";

function auditRows(serverData: string, kind: string): Array<{ count: number; credentialKind: string | null; details: string }> {
  const db = new DatabaseSync(controlDbPath(serverData), { readOnly: true });
  try {
    return db.prepare("SELECT count, credentialKind, details FROM audit WHERE kind = ? ORDER BY id").all(kind) as never;
  } finally {
    db.close();
  }
}

describe("the handshake on a real server", () => {
  let tmp: string;
  let serverData: string;
  let server: RunningServer;
  const open: NdjsonClient[] = [];
  const connect = async () => {
    const c = await NdjsonClient.connect(server.info.socket);
    open.push(c);
    return c;
  };

  beforeAll(async () => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-proto-"));
    serverData = join(tmp, "sd");
    server = await startServer(serverData);
  });

  afterAll(async () => {
    await Promise.all(open.map((c) => c.close().catch(() => undefined)));
    await server.stop();
    rmSync(tmp, { recursive: true, force: true });
  });

  test("prove_binds_local_and_console", async () => {
    const local = ok<{ principal: { kind: string; human: boolean; scopes: string[] } }>(
      await prove(await connect(), server.info, keyOf(serverData, "local"), "local"),
    );
    expect(local.principal).toMatchObject({ kind: "local", id: "local", human: false, projects: "*", branches: "*" });
    expect(local.principal.scopes).not.toContain("diagnostics.admin");
    const console = ok<{ principal: { kind: string; human: boolean; scopes: string[] } }>(
      await prove(await connect(), server.info, keyOf(serverData, "console"), "console"),
    );
    expect(console.principal).toMatchObject({ kind: "console", id: "console", human: true });
    expect(console.principal.scopes).toContain("diagnostics.admin");
  });

  test("each_connection_gets_its_own_principal", async () => {
    const a = await connect();
    const b = await connect();
    ok(await prove(a, server.info, keyOf(serverData, "console"), "console"));
    // `b` has proved nothing: it can still prove as `local`, while `a` cannot prove again.
    const second = await prove(b, server.info, keyOf(serverData, "local"), "local");
    expect(ok<{ principal: { kind: string } }>(second).principal.kind).toBe("local");
    const again = await prove(a, server.info, keyOf(serverData, "console"), "console");
    expect(again.ok ? "" : again.error.code).toBe("INVALID_REQUEST");
  });

  test("second_prove_is_invalid_request", async () => {
    const c = await connect();
    ok(await prove(c, server.info, keyOf(serverData)));
    const again = await c.call("auth.prove", { mac: b64u.encode(Buffer.alloc(32)) });
    expect(again.ok ? "" : again.error.code).toBe("INVALID_REQUEST");
  });

  test("hello_without_nonce_has_no_proof", async () => {
    const answer = ok<Record<string, unknown>>(await (await connect()).call("server.hello", {}));
    expect(answer.protocolVersion).toBe(1);
    expect(answer).not.toHaveProperty("proof");
    expect(answer).not.toHaveProperty("serverNonce");
  });

  test("unknown_credential_gets_no_proof", async () => {
    const c = await connect();
    const { exit } = await hello(c, { credential: { kind: "agent", id: "01agent000000000000000000a" } });
    const answer = ok<Record<string, unknown>>(exit);
    expect(answer.serverNonce).toEqual(expect.any(String));
    expect(answer.envId).toBe(server.info.envId);
    expect(answer).not.toHaveProperty("proof");
    const refused = await c.call("auth.prove", { mac: b64u.encode(Buffer.alloc(32)) });
    expect(refused.ok ? null : refused.error).toMatchObject({ code: "UNAUTHENTICATED", details: { reason: "noProof" } });
  });

  test("foreign_env_gets_no_proof", async () => {
    const { exit } = await hello(await connect(), { env: "01otherenv0000000000000000" });
    const answer = ok<Record<string, unknown>>(exit);
    expect(answer.serverNonce).toEqual(expect.any(String));
    expect(answer).not.toHaveProperty("proof");
  });

  test("relayed_proof_fails_on_other_endpoint", async () => {
    const c = await connect();
    const endpoint = canonicalEndpoint(server.info.socket);
    // A client that thinks it talks to another socket gets no proof from this one.
    const other = ok<Record<string, unknown>>((await hello(c, { endpoint: `${endpoint}.elsewhere` })).exit);
    expect(other).not.toHaveProperty("proof");
    // A MAC made with the right key for another endpoint (a relay) is refused here.
    const { clientNonce, exit } = await hello(c, { env: server.info.envId });
    const answer = ok<{ serverNonce: string; envId: string; endpoint: string }>(exit);
    expect(answer.endpoint).toBe(endpoint);
    const mac = proveMac(keyOf(serverData), answer.envId, "/elsewhere/run/fin.sock", b64u.decode(answer.serverNonce)!, clientNonce);
    const refused = await c.call("auth.prove", { mac: b64u.encode(mac) });
    expect(refused.ok ? null : refused.error).toMatchObject({ code: "UNAUTHENTICATED", details: { reason: "badMac" } });
  });

  test("wrong_mac_fails_and_five_failures_close", async () => {
    const before = auditRows(serverData, "auth.fail").reduce((n, r) => n + r.count, 0);
    const c = await connect();
    for (let i = 1; i <= 5; i++) {
      const { clientNonce } = await hello(c);
      const mac = b64u.encode(Buffer.alloc(32, i));
      const refused = await c.call("auth.prove", { mac });
      expect(refused.ok ? null : refused.error).toMatchObject({ code: "UNAUTHENTICATED", details: { reason: "badMac" } });
      // No MAC or nonce comes back in the error.
      expect(JSON.stringify(refused)).not.toContain(mac);
      expect(JSON.stringify(refused)).not.toContain(b64u.encode(clientNonce));
    }
    await c.closed();
    const rows = auditRows(serverData, "auth.fail");
    expect(rows.reduce((n, r) => n + r.count, 0) - before).toBe(5);
    expect(rows.at(-1)?.credentialKind).toBe("local");
    expect(rows.map((r) => r.details).join()).not.toMatch(/[A-Za-z0-9_-]{43}/);
  });

  test("bad_frame_is_invalid_request_not_defect", async () => {
    const c = await connect();
    const frames: Array<Record<string, unknown>> = [];
    const next = () => c.nextFrame().then((f) => (frames.push(f), f));
    c.write('{"_tag":"Request","id":"41","tag":"server.hello","payload":{\n');
    const bad = await next();
    const unknown = await c.call("no.such", {});
    const missing = await c.call("project.status", {});
    const wrongType = await c.call("server.hello", { clientNonce: 7 });
    for (const exit of [unknown, missing, wrongType]) expect(exit.ok ? null : exit.error.code).toBe("INVALID_REQUEST");
    expect(bad).toMatchObject({ _tag: "Exit", requestId: "41", exit: { _tag: "Failure", cause: [{ _tag: "Fail", error: { code: "INVALID_REQUEST" } }] } });
    expect(c.transcript.some((t) => t.dir === "<<" && (t.line.includes('"Defect"') || t.line.includes('"Die"')))).toBe(false);
    // The connection still works.
    expect(ok<{ protocolVersion: number }>(await c.call("server.hello", {})).protocolVersion).toBe(1);
  });

  test("oversized_frame_is_frame_too_large_then_closed", async () => {
    const c = await connect();
    const frame = c.nextFrame();
    c.write(`{"_tag":"Request","id":"9","tag":"server.hello","headers":[],"payload":{"pad":"${"x".repeat(16 * 1024 * 1024)}"}}\n`);
    expect(await frame).toMatchObject({ _tag: "Exit", requestId: "9", exit: { cause: [{ error: { code: "FRAME_TOO_LARGE", details: { max: 16 * 1024 * 1024 } } }] } });
    await c.closed();
  });

  test("unproved_connection_is_legacy_local_and_audited", async () => {
    const before = auditRows(serverData, "auth.legacy").length;
    const c = await connect();
    ok(await c.call("fonts.status", {}));
    ok(await c.call("diagnostics.status", {}));
    const rows = auditRows(serverData, "auth.legacy");
    expect(rows.length - before).toBe(1);
    expect(JSON.parse(rows.at(-1)!.details)).toEqual({ transport: WIN ? "pipe" : "socket" });
  });
});

// The protocol in-process, over a test group, with a 1 KiB frame cap.
const Echo = RpcGroup.make(
  Rpc.make("test.echo", { payload: Schema.Struct({ n: Schema.Number }), success: Schema.Unknown }),
);

describe("the protocol in-process", () => {
  let tmp: string;
  let path: string;
  let connections: Connections;
  let fiber: Fiber.Fiber<unknown, unknown>;

  beforeAll(async () => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-proto-"));
    path = WIN ? `\\\\.\\pipe\\finisherd-proto-test-${process.pid}` : join(tmp, "p.sock");
    connections = new Connections("socket", () => path);
    const handlers = Echo.toLayer({
      "test.echo": ({ n }, { headers, client }) => Effect.succeed({ n, client: client.id, headers: Object.keys(headers).sort() }),
    });
    const layer = RpcServer.layer(Echo).pipe(
      Layer.provide(handlers),
      Layer.provide(Layer.effect(RpcServer.Protocol)(makeFinProtocol(Echo, connections, { maxFrame: 1024 }))),
      Layer.provide(Layer.effect(SocketServer)(NodeSocketServer.make({ path }))),
      Layer.provide(RpcSerialization.layerNdjson),
    );
    fiber = Effect.runFork(Layer.launch(layer));
    for (let i = 0; i < 200; i++) {
      try {
        await (await NdjsonClient.connect(path)).close();
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    throw new Error("the in-process server did not listen");
  });

  afterAll(async () => {
    await Effect.runPromise(Fiber.interrupt(fiber));
    rmSync(tmp, { recursive: true, force: true });
  });

  test("x_fin_headers_are_dropped", async () => {
    const c = await NdjsonClient.connect(path);
    const answer = ok<{ headers: string[] }>(
      await c.call("test.echo", { n: 1 }, [
        ["x-fin-principal", "console"],
        ["X-Fin-Source", "ui"],
        ["fin-actor-kind", "agent"],
      ]),
    );
    expect(answer.headers).toEqual(["fin-actor-kind"]);
    expect(clientHeaders([["x-fin-a", "1"], ["b", "2"]])).toEqual([["b", "2"]]);
    expect(clientHeaders([["a"]])).toBeUndefined();
    await c.close();
  });

  test("bad_frame_is_invalid_request_not_defect (in-process)", async () => {
    const c = await NdjsonClient.connect(path);
    const missing = await c.call("test.echo", {});
    expect(missing.ok ? null : missing.error).toMatchObject({ code: "INVALID_REQUEST", details: { tag: "test.echo" } });
    const badHeaders = await c.call("test.echo", { n: 1 }, [["only-a-name"]] as never);
    expect(badHeaders.ok ? null : badHeaders.error.code).toBe("INVALID_REQUEST");
    // A frame with no id to answer ends the connection.
    c.write("not json at all\n");
    await c.closed();
  });

  test("oversized_frame_is_frame_too_large_then_closed (in-process)", async () => {
    const c = await NdjsonClient.connect(path);
    const frame = c.nextFrame();
    // Split over two writes: the cap counts bytes since the last newline.
    c.write(`{"_tag":"Request","id":"77","tag":"test.echo","payload":{"pad":"${"y".repeat(600)}`);
    c.write(`${"z".repeat(600)}"}}\n`);
    expect(await frame).toMatchObject({ requestId: "77", exit: { cause: [{ error: { code: "FRAME_TOO_LARGE", details: { max: 1024 } } }] } });
    await c.closed();
  });

  test("disconnect_removes_connection", async () => {
    const c = await NdjsonClient.connect(path);
    const { client } = ok<{ client: number }>(await c.call("test.echo", { n: 2 }));
    expect(connections.has(client)).toBe(true);
    expect(connections.count()).toMatchObject({ anonymous: expect.any(Number) });
    await c.close();
    for (let i = 0; i < 100 && connections.has(client); i++) await new Promise((r) => setTimeout(r, 10));
    expect(connections.has(client)).toBe(false);
  });

  test("terminate answers open requests, then closes", async () => {
    const c = await NdjsonClient.connect(path);
    const { client } = ok<{ client: number }>(await c.call("test.echo", { n: 3 }));
    const n = connections.closeWhere((x) => x.clientId === client, {
      code: "CREDENTIAL_REVOKED",
      message: "revoked",
      details: {},
    } as never);
    expect(n).toBe(1);
    await c.closed();
  });
});

describe("framing helpers", () => {
  test("line splitter keeps multi-byte characters split across chunks", () => {
    const s = new LineSplitter(64);
    const bytes = Buffer.from("проєкт\n", "utf8");
    expect(s.push(bytes.subarray(0, 3)).lines).toEqual([]);
    expect(s.push(bytes.subarray(3)).lines).toEqual(["проєкт"]);
    expect(s.push("a\nb\n").lines).toEqual(["a", "b"]);
    expect(s.push("x".repeat(65)).tooLarge).toBe("x".repeat(65));
  });

  test("salvageId reads the request id near the start", () => {
    expect(salvageId('{"_tag":"Request","headers":[],"id":"12","payload":{')).toBe("12");
    expect(salvageId('{"_tag":"Request","id":5,')).toBe("5");
    expect(salvageId("garbage")).toBeUndefined();
  });
});
