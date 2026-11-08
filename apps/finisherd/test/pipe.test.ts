// The Windows pipe transport (plan 5A Task 11). `pipe.ts` runs the real protocol over a fake
// native listener on every platform (the Duplex adapter, peers, the frame cap, `Link.terminate`,
// a client that goes away) and over fin-node's real `PipeServer` on Windows, where a real
// finisherd also shows squatting (`PIPE_SQUATTED`), peers in the audit log and shutdown.
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { PipeEvent } from "@finisher/fin-node";
import { Effect, Fiber, Layer, Schema, type Scope } from "effect";
import { Rpc, RpcGroup, RpcSerialization, RpcServer } from "effect/rpc";
import { SocketServer } from "effect/socket/SocketServer";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { socketPath } from "../src/config.ts";
import { Connections } from "../src/connections.ts";
import { controlDbPath } from "../src/control/db.ts";
import { makePipeSocketServer, type NativePipe, pipeStartError, PipeStream } from "../src/pipe.ts";
import { makeFinProtocol } from "../src/protocol.ts";
import { type Frame, hello, NdjsonClient, ok, prove, keyOf, startServer } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const SID = "S-1-5-21-1000-2000-3000-1001";

/** The test side of a fake pipe connection: what the client sent and what it received. */
class FakeClient {
  readonly id: number;
  #inbound: Array<Buffer | null> = [];
  #reader: ((b: Buffer | null) => void) | undefined;
  #text = "";
  #frames: Frame[] = [];
  #waiters: Array<(f: Frame) => void> = [];
  #closedResolve!: () => void;
  readonly whenClosed = new Promise<void>((r) => (this.#closedResolve = r));
  closed = false;
  gone = false;

  constructor(id: number) {
    this.id = id;
  }
  /** Client → server bytes. */
  write(text: string): void {
    this.#deliver(Buffer.from(text, "utf8"));
  }
  /** The client disconnects. */
  leave(): void {
    this.gone = true;
    this.#deliver(null);
  }
  nextFrame(): Promise<Frame> {
    const f = this.#frames.shift();
    return f !== undefined ? Promise.resolve(f) : new Promise((r) => this.#waiters.push(r));
  }
  /** The server's `read`. */
  read(): Promise<Buffer | null> {
    const next = this.#inbound.shift();
    if (next !== undefined) return Promise.resolve(next);
    if (this.closed) return Promise.resolve(null);
    return new Promise((r) => (this.#reader = r));
  }
  /** The server's `write`. */
  receive(data: Uint8Array): Promise<void> {
    if (this.gone || this.closed) return Promise.reject(Object.assign(new Error("gone"), { code: "ERROR_BROKEN_PIPE" }));
    this.#text += Buffer.from(data).toString("utf8");
    for (let nl = this.#text.indexOf("\n"); nl >= 0; nl = this.#text.indexOf("\n")) {
      const frame = JSON.parse(this.#text.slice(0, nl)) as Frame;
      this.#text = this.#text.slice(nl + 1);
      const w = this.#waiters.shift();
      if (w !== undefined) w(frame);
      else this.#frames.push(frame);
    }
    return Promise.resolve();
  }
  /** The server's `close`. */
  close(): void {
    this.closed = true;
    this.#deliver(null);
    this.#closedResolve();
  }
  #deliver(b: Buffer | null): void {
    const r = this.#reader;
    this.#reader = undefined;
    if (r !== undefined) r(b);
    else this.#inbound.push(b);
  }
}

/** A fake of fin-node's `PipeServer`: `connect` plays a client the listener accepted. */
class FakePipe implements NativePipe {
  readonly clients = new Map<number, FakeClient>();
  #events: ((e: PipeEvent) => void) | undefined;
  #backlog: PipeEvent[] = [];
  #next = 1;
  shutDown = false;

  onEvent(cb: (e: PipeEvent) => void): void {
    this.#events = cb;
    for (const e of this.#backlog.splice(0)) cb(e);
  }
  emit(e: PipeEvent): void {
    if (this.#events === undefined) this.#backlog.push(e);
    else this.#events(e);
  }
  connect(pid = 4242, sid = SID): FakeClient {
    const c = new FakeClient(this.#next++);
    this.clients.set(c.id, c);
    this.emit({ type: "connection", id: c.id, pid, sid });
    return c;
  }
  read(id: number): Promise<Buffer | null> {
    return this.clients.get(id)?.read() ?? Promise.resolve(null);
  }
  write(id: number, data: Uint8Array): Promise<void> {
    return this.clients.get(id)?.receive(data) ?? Promise.reject(Object.assign(new Error("gone"), { code: "ERROR_BROKEN_PIPE" }));
  }
  close(id: number): void {
    this.clients.get(id)?.close();
  }
  shutdown(): void {
    this.shutDown = true;
    for (const c of this.clients.values()) c.close();
  }
}

const request = (id: string, payload: unknown, tag = "test.peer") => `${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`;

// The protocol over a test group: `test.peer` answers the connection's peer and id,
// `test.hang` never answers (for `terminate`).
const Group = RpcGroup.make(
  Rpc.make("test.peer", { payload: Schema.Struct({ n: Schema.Number }), success: Schema.Unknown }),
  Rpc.make("test.hang", { payload: Schema.Struct({}), success: Schema.Unknown }),
);

function serveGroup(server: Effect.Effect<SocketServer["Service"], unknown, Scope.Scope>, connections: Connections) {
  const handlers = Group.toLayer({
    "test.peer": ({ n }, { client }) => Effect.sync(() => ({ n, client: client.id, peer: connections.get(client.id)?.peer ?? null })),
    "test.hang": () => Effect.never,
  });
  return RpcServer.layer(Group).pipe(
    Layer.provide(handlers),
    Layer.provide(Layer.effect(RpcServer.Protocol)(makeFinProtocol(Group, connections, { maxFrame: 1024 }))),
    Layer.provide(Layer.effect(SocketServer)(server)),
    Layer.provide(RpcSerialization.layerNdjson),
  );
}

const until = async (f: () => boolean, ms = 2000) => {
  for (const end = Date.now() + ms; !f(); ) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 5));
  }
};

describe("pipe.ts over a fake native listener", () => {
  let fake: FakePipe;
  let connections: Connections;
  let fiber: Fiber.Fiber<unknown, unknown>;
  const refused: PipeEvent[] = [];

  beforeAll(() => {
    fake = new FakePipe();
    connections = new Connections("pipe", () => "\\\\.\\pipe\\fake");
    const server = makePipeSocketServer("\\\\.\\pipe\\fake", { native: () => fake, onRefused: (r) => void refused.push(r) });
    fiber = Effect.runFork(Layer.launch(serveGroup(server, connections)));
  });

  afterAll(async () => {
    await Effect.runPromise(Fiber.interrupt(fiber));
  });

  test("a_connection_runs_the_protocol_and_carries_its_peer", async () => {
    const c = fake.connect(4242, SID);
    c.write(request("1", { n: 7 }));
    const frame = await c.nextFrame();
    expect(frame).toMatchObject({ _tag: "Exit", requestId: "1", exit: { _tag: "Success", value: { n: 7, peer: { pid: 4242, sid: SID } } } });
    c.leave();
  });

  test("chunks_split_anywhere_are_one_frame", async () => {
    const c = fake.connect();
    const line = request("2", { n: 8 });
    for (const part of [line.slice(0, 5), line.slice(5, 20), line.slice(20)]) c.write(part);
    expect(await c.nextFrame()).toMatchObject({ requestId: "2", exit: { _tag: "Success" } });
    c.leave();
  });

  test("oversized_frame_is_frame_too_large_then_closed", async () => {
    const c = fake.connect();
    c.write(`{"_tag":"Request","id":"77","tag":"test.peer","payload":{"pad":"${"y".repeat(600)}`);
    c.write(`${"z".repeat(600)}"}}\n`);
    expect(await c.nextFrame()).toMatchObject({ requestId: "77", exit: { cause: [{ error: { code: "FRAME_TOO_LARGE", details: { max: 1024 } } }] } });
    await c.whenClosed;
  });

  test("terminate_answers_open_requests_then_closes (revocation reaches pipe streams)", async () => {
    const c = fake.connect();
    c.write(request("1", { n: 1 }));
    const client = ((await c.nextFrame()).exit as { value: { client: number } }).value.client;
    c.write(request("h", {}, "test.hang"));
    await new Promise((r) => setTimeout(r, 20));
    const n = connections.closeWhere((x) => x.clientId === client, {
      code: "CREDENTIAL_REVOKED",
      message: "revoked",
      details: {},
    } as never);
    expect(n).toBe(1);
    expect(await c.nextFrame()).toMatchObject({ requestId: "h", exit: { cause: [{ error: { code: "CREDENTIAL_REVOKED" } }] } });
    await c.whenClosed;
  });

  test("a_client_that_leaves_is_dropped_and_closed", async () => {
    const c = fake.connect();
    c.write(request("1", { n: 1 }));
    const { value } = (await c.nextFrame()).exit as { value: { client: number } };
    expect(connections.has(value.client)).toBe(true);
    c.leave();
    await until(() => !connections.has(value.client));
    await c.whenClosed;
  });

  test("refused_clients_are_reported", () => {
    fake.emit({ type: "refused", pid: 9, sid: "S-1-5-21-9-9-9-1002", reason: "peer" });
    expect(refused).toEqual([{ type: "refused", pid: 9, sid: "S-1-5-21-9-9-9-1002", reason: "peer" }]);
  });
});

describe("PipeStream", () => {
  test("reads_are_pulled_in_order_and_null_ends_the_stream", async () => {
    const fake = new FakePipe();
    const c = fake.connect();
    const s = new PipeStream(fake, c.id);
    const got: string[] = [];
    s.on("data", (d: Buffer) => got.push(d.toString()));
    const ended = new Promise((r) => s.once("end", r));
    c.write("a");
    c.write("b");
    c.leave();
    await ended;
    expect(got.join("")).toBe("ab");
  });

  test("queued_writes_go_out_in_order_in_one_native_write", async () => {
    const fake = new FakePipe();
    const c = fake.connect();
    let writes = 0;
    const native = { ...fake, read: (id: number) => fake.read(id), close: (id: number) => fake.close(id), write: (id: number, d: Uint8Array) => (writes++, fake.write(id, d)) };
    const s = new PipeStream(native, c.id);
    const frames = Array.from({ length: 50 }, (_, i) => `{"_tag":"Pong","n":${i}}\n`);
    for (const f of frames) s.write(f);
    await new Promise((r) => s.end(r));
    const got: unknown[] = [];
    for (let i = 0; i < 50; i++) got.push(await c.nextFrame());
    expect(got).toEqual(frames.map((f) => JSON.parse(f)));
    expect(writes).toBeLessThan(50);
  });

  test("a_write_to_a_gone_client_fails_with_its_code_and_destroy_closes", async () => {
    const fake = new FakePipe();
    const c = fake.connect();
    const s = new PipeStream(fake, c.id);
    c.gone = true;
    const err = await new Promise<Error & { code?: string }>((r) => s.once("error", r).write("x"));
    expect(err.code).toBe("ERROR_BROKEN_PIPE");
    await c.whenClosed;
  });

  test("pipe_squatted_has_a_fix_in_words", () => {
    const e = pipeStartError("\\\\.\\pipe\\x", Object.assign(new Error("held"), { code: "PIPE_SQUATTED", finNode: true }));
    expect(e).toMatchObject({ code: "PIPE_SQUATTED", details: { pipe: "\\\\.\\pipe\\x" } });
    expect(e.fix).not.toMatch(/finisherd|fin |--|`/);
    expect(pipeStartError("p", Object.assign(new Error("x"), { code: "SERVER_START_FAILED", finNode: true })).code).toBe("SERVER_START_FAILED");
  });
});

describe.skipIf(!WIN)("pipe.ts over the native listener (Windows)", () => {
  let connections: Connections;
  let fiber: Fiber.Fiber<unknown, unknown>;
  const path = `\\\\.\\pipe\\finisherd-pipe-test-${process.pid}`;
  const open: NdjsonClient[] = [];

  beforeAll(async () => {
    connections = new Connections("pipe", () => path.toLowerCase());
    fiber = Effect.runFork(Layer.launch(serveGroup(makePipeSocketServer(path), connections)));
    for (let i = 0; i < 200; i++) {
      try {
        open.push(await NdjsonClient.connect(path));
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    throw new Error("the pipe server did not listen");
  });

  afterEach(async () => {
    await Promise.all(open.splice(0).map((c) => c.close().catch(() => undefined)));
  });

  afterAll(async () => {
    await Effect.runPromise(Fiber.interrupt(fiber));
  });

  const connect = async () => {
    const c = await NdjsonClient.connect(path);
    open.push(c);
    return c;
  };

  test("peer_pid_and_sid_reported", async () => {
    const c = await connect();
    const answer = ok<{ peer: { pid: number; sid: string } }>(await c.call("test.peer", { n: 1 }));
    expect(answer.peer.pid).toBe(process.pid);
    expect(answer.peer.sid).toMatch(/^S-1-5-21-\d+-\d+-\d+-\d+$/);
  });

  test("oversized_frame_is_frame_too_large_then_closed (native)", async () => {
    const c = await connect();
    const frame = c.nextFrame();
    c.write(`{"_tag":"Request","id":"77","tag":"test.peer","payload":{"pad":"${"y".repeat(600)}`);
    c.write(`${"z".repeat(600)}"}}\n`);
    expect(await frame).toMatchObject({ requestId: "77", exit: { cause: [{ error: { code: "FRAME_TOO_LARGE" } }] } });
    await c.closed();
  });

  test("terminate_answers_open_requests_then_closes (native)", async () => {
    const c = await connect();
    const { client } = ok<{ client: number }>(await c.call("test.peer", { n: 2 }));
    const hang = c.call("test.hang", {});
    await new Promise((r) => setTimeout(r, 50));
    expect(connections.closeWhere((x) => x.clientId === client, { code: "CREDENTIAL_REVOKED", message: "revoked", details: {} } as never)).toBe(1);
    const exit = await hang;
    expect(exit.ok ? null : exit.error.code).toBe("CREDENTIAL_REVOKED");
    await c.closed();
  });

  test("many_requests_in_flight_are_all_answered", async () => {
    const c = await connect();
    const got = await Promise.all(Array.from({ length: 900 }, (_, i) => c.call("test.peer", { n: i })));
    expect(got.every((e) => e.ok)).toBe(true);
  });

  test("shutdown_closes_all_connections_and_frees_the_name", async () => {
    const name = `${path}-shutdown`;
    const conns = new Connections("pipe", () => name.toLowerCase());
    const launch = () => Effect.runFork(Layer.launch(serveGroup(makePipeSocketServer(name), conns)));
    const first = launch();
    const clients: NdjsonClient[] = [];
    for (let i = 0; clients.length < 3 && i < 200; i++) {
      try {
        clients.push(await NdjsonClient.connect(name));
      } catch {
        await new Promise((r) => setTimeout(r, 10));
      }
    }
    expect(clients).toHaveLength(3);
    const closed = Promise.all(clients.map((c) => c.closed()));
    await Effect.runPromise(Fiber.interrupt(first));
    await closed;
    // `shutdown` returned once the name was free: the next server's first instance is accepted.
    const second = launch();
    const c = await (async () => {
      for (let i = 0; i < 200; i++) {
        try {
          return await NdjsonClient.connect(name);
        } catch {
          await new Promise((r) => setTimeout(r, 10));
        }
      }
      throw new Error("the second server did not listen");
    })();
    expect(ok<{ n: number }>(await c.call("test.peer", { n: 3 })).n).toBe(3);
    await c.close();
    await Effect.runPromise(Fiber.interrupt(second));
  });
});

describe.skipIf(!WIN)("finisherd over the native pipe (Windows)", () => {
  let tmp: string;

  beforeAll(() => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-pipe-"));
  });

  afterAll(() => {
    rmSync(tmp, { recursive: true, force: true });
  });

  test("first_instance_or_pipe_squatted", async () => {
    const serverData = join(tmp, "squatted");
    const squatter = createServer(() => {});
    await new Promise<void>((r) => squatter.listen(socketPath(serverData), r));
    try {
      await expect(startServer(serverData)).rejects.toThrow(/PIPE_SQUATTED/);
    } finally {
      await new Promise((r) => squatter.close(r));
    }
  });

  test("peer_in_the_audit_log", async () => {
    const serverData = join(tmp, "audit");
    const server = await startServer(serverData);
    try {
      const proved = await NdjsonClient.connect(server.info.socket);
      ok(await prove(proved, server.info, keyOf(serverData)));
      const c = await NdjsonClient.connect(server.info.socket);
      await hello(c);
      const bad = await c.call("auth.prove", { mac: "A".repeat(43) });
      expect(bad.ok ? null : bad.error.code).toBe("UNAUTHENTICATED");
      const db = new DatabaseSync(controlDbPath(serverData), { readOnly: true });
      const rows = db.prepare("SELECT details FROM audit WHERE kind = 'auth.fail'").all() as Array<{ details: string }>;
      db.close();
      const peers = rows.map((r) => (JSON.parse(r.details) as { peer?: { pid: number; sid: string } }).peer);
      expect(peers).toContainEqual({ pid: process.pid, sid: expect.stringMatching(/^S-1-5-21-/) });
      await Promise.all([proved.close(), c.close()]);
    } finally {
      await server.stop();
    }
  });
});
