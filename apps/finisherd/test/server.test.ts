import { request } from "node:http";
import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { childEnv } from "../src/env.ts";
import { addAsset, insert, NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-"));
  serverData = join(tmp, "server-data");
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  if (server) expect(await server.stop()).toBe(0);
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function start(): Promise<RunningServer> {
  server = await startServer(serverData);
  return server;
}

async function client(): Promise<NdjsonClient> {
  const c = await NdjsonClient.connect(server!.info.socket);
  clients.push(c);
  return c;
}

async function createProject(c: NdjsonClient, name = "Demo"): Promise<string> {
  const ref = ok<{ projectId: string; rev: number }>(
    await c.call("project.create", { dir: join(tmp, "My Проєкт"), name, rate: "30" }),
  );
  expect(ref.rev).toBe(0);
  return ref.projectId;
}

describe("finisherd", () => {
  test("hello returns protocol 1; finisherd.json and fin.sock are 0600", async () => {
    const { info } = await start();
    expect(info.protocolVersion).toBe(1);
    expect(info.socket).toBe(join(serverData, "fin.sock"));
    expect(info.token.length).toBeGreaterThanOrEqual(32);
    expect(statSync(join(serverData, "finisherd.json")).mode & 0o777).toBe(0o600);
    expect(statSync(info.socket).mode & 0o777).toBe(0o600);

    const c = await client();
    const hello = ok<{ protocolVersion: number; capabilities: string[] }>(await c.call("server.hello", {}));
    expect(hello.protocolVersion).toBe(1);
    expect(hello.capabilities).toContain("ops.subscribe");
    c.send({ _tag: "Ping" });
    expect((await c.nextFrame())._tag).toBe("Pong");
  });

  test("create, commit and snapshot over the socket", async () => {
    await start();
    const c = await client();
    const projectId = await createProject(c);
    const c1 = ok<{ rev: number; bindings: Record<string, string> }>(
      await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }),
    );
    expect(c1).toMatchObject({ rev: 1, bindings: { $a: "a1" } });
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-2", 1, [insert(0, 5)]) }));

    const snap = ok<{ rev: number; project: { timeline: { tracks: Array<{ items: unknown[] }> } } }>(
      await c.call("snapshot.get", { projectId, branch: "main" }),
    );
    expect(snap.rev).toBe(2);
    expect(snap.project.timeline.tracks[0]!.items).toMatchObject([{ id: "c1", start: "0", duration: "5" }]);

    const again = ok<{ projectId: string; rev: number }>(await c.call("project.open", { dir: join(tmp, "My Проєкт") }));
    expect(again).toEqual({ projectId, rev: 2 });
  });

  test("errors arrive as FinError with a stable code", async () => {
    await start();
    const c = await client();
    const projectId = await createProject(c);
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-asset", 0, [addAsset()]) }));
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-base", 1, [insert(0, 5)]) }));
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-del", 2, [{ op: "clip.delete", clip: "c1" }]) }));

    const conflict = await c.call("tx.commit", { projectId, tx: tx("tx-late", 2, [{ op: "clip.delete", clip: "c1" }]) });
    expect(conflict.ok).toBe(false);
    if (conflict.ok) return;
    expect(conflict.error).toMatchObject({ _tag: "FinError", code: "CONFLICT", details: { againstTx: "tx-del", txId: "tx-late" } });

    const exists = await c.call("project.create", { dir: join(tmp, "My Проєкт"), name: "Demo", rate: "30" });
    expect(!exists.ok && exists.error.code).toBe("PROJECT_EXISTS");
    const missing = await c.call("snapshot.get", { projectId: "nope", branch: "main" });
    expect(!missing.ok && missing.error.code).toBe("PROJECT_NOT_FOUND");
    const notDir = await c.call("project.open", { dir: join(tmp, "nothing") });
    expect(!notDir.ok && notDir.error.code).toBe("PROJECT_NOT_FOUND");

    // A failing subscription fails inside the stream, with the same error shape.
    const sub = c.stream("ops.subscribe", { projectId: "nope", branch: "main", fromRev: 0 }, () => undefined);
    const subExit = await sub.done;
    expect(!subExit.ok && subExit.error.code).toBe("PROJECT_NOT_FOUND");
  });

  test("subscribe_during_commits_no_gap_no_dup", async () => {
    await start();
    const writer = await client();
    const reader = await client();
    const projectId = await createProject(writer);
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-2", 1, [insert(0, 5)]) }));
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-3", 2, [insert(5, 5)]) }));

    // 50 concurrent commits, all against base rev 3 (asset.add never conflicts).
    const commits = Array.from({ length: 50 }, (_, i) =>
      writer.call("tx.commit", { projectId, tx: tx(`tx-c${i}`, 3, [addAsset(`$a${i}`)]) }),
    );
    // Join once the first of them has landed, so part arrives as backlog and part live.
    await Promise.race(commits);

    const revs: number[] = [];
    let resolveAll!: () => void;
    const all = new Promise<void>((r) => (resolveAll = r));
    const sub = reader.stream("ops.subscribe", { projectId, branch: "main", fromRev: 3 }, (v) => {
      revs.push((v as { rev: number }).rev);
      if (revs.length >= 50) resolveAll();
    });
    for (const exit of await Promise.all(commits)) ok(exit);
    await all;
    // Any duplicate or late extra would show up within a short grace period.
    await new Promise((r) => setTimeout(r, 200));
    expect(revs).toEqual(Array.from({ length: 50 }, (_, i) => i + 4));

    // A commit after the burst still arrives live.
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-after", 53, [addAsset("$z")]) }));
    await expect.poll(() => revs.at(-1)).toBe(54);
    reader.interrupt(sub.id);
    await sub.done;
  });

  test("restart keeps projects; shutdown removes finisherd.json", async () => {
    await start();
    let c = await client();
    const projectId = await createProject(c);
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));
    await clients.splice(0)[0]!.close();
    expect(await server!.stop()).toBe(0);
    server = undefined;
    expect(existsSync(join(serverData, "finisherd.json"))).toBe(false);

    await start();
    c = await client();
    // By id, straight from the app.db index…
    const snap = ok<{ rev: number }>(await c.call("snapshot.get", { projectId, branch: "main" }));
    expect(snap.rev).toBe(1);
    // …and by directory.
    expect(ok(await c.call("project.open", { dir: join(tmp, "My Проєкт") }))).toEqual({ projectId, rev: 1 });
  });

  test("a second server on the same server-data refuses to start", async () => {
    await start();
    await expect(startServer(serverData, 5_000)).rejects.toThrow(/SERVER_RUNNING/);
  });

  test("WebSocket requires the bearer token", async () => {
    const { info } = await start();
    const status = (headers: Record<string, string>) =>
      new Promise<number>((resolve, reject) => {
        const req = request({
          host: "127.0.0.1",
          port: info.wsPort,
          headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", ...headers },
        });
        req.on("response", (res) => resolve(res.statusCode ?? 0));
        req.on("upgrade", (res, socket) => {
          socket.destroy();
          resolve(res.statusCode ?? 0);
        });
        req.on("error", reject);
        req.end();
      });
    expect(await status({})).toBe(401);
    expect(await status({ Authorization: "Bearer wrong" })).toBe(401);
    expect(await status({ Authorization: `Basic ${info.token}` })).toBe(401);
    expect(await status({ Authorization: `Bearer ${info.token}` })).toBe(101);

    // A full RPC round-trip over the authenticated WebSocket.
    const ws = new WebSocket(`ws://127.0.0.1:${info.wsPort}`, {
      headers: { Authorization: `Bearer ${info.token}` },
    } as unknown as string[]);
    const reply = await new Promise<string>((resolve, reject) => {
      ws.onopen = () => ws.send(`${JSON.stringify({ _tag: "Request", id: "1", tag: "server.hello", payload: {}, headers: [] })}\n`);
      ws.onmessage = (e) => resolve(String(e.data));
      ws.onerror = () => reject(new Error("websocket error"));
    });
    ws.close();
    expect(JSON.parse(reply.trim())).toMatchObject({ _tag: "Exit", requestId: "1", exit: { _tag: "Success", value: { protocolVersion: 1 } } });
  });
});

describe("childEnv", () => {
  const parent = {
    PATH: "/usr/bin",
    HOME: "/home/u",
    LANG: "uk_UA.UTF-8",
    LC_ALL: "uk_UA.UTF-8",
    LC_CTYPE: "UTF-8",
    TMPDIR: "/tmp/u",
    ELECTRON_RUN_AS_NODE: "1",
    CLAUDECODE: "1",
    CLAUDE_CODE_ENTRYPOINT: "cli",
    CLAUDE_CODE_SSE_PORT: "1234",
    NODE_OPTIONS: "--inspect",
    AWS_SECRET_ACCESS_KEY: "secret",
    FIN_SERVER: "inherited-should-not-pass",
  };

  test("keeps only the allow-list plus FIN_SERVER and FIN_SESSION from extra", () => {
    expect(childEnv({ FIN_SERVER: "unix:/s/fin.sock", FIN_SESSION: "sess-1" }, parent, "darwin")).toEqual({
      PATH: "/usr/bin",
      HOME: "/home/u",
      LANG: "uk_UA.UTF-8",
      LC_ALL: "uk_UA.UTF-8",
      LC_CTYPE: "UTF-8",
      TMPDIR: "/tmp/u",
      FIN_SERVER: "unix:/s/fin.sock",
      FIN_SESSION: "sess-1",
    });
  });

  test("drops ELECTRON_RUN_AS_NODE, CLAUDECODE and CLAUDE_CODE_*", () => {
    for (const platform of ["darwin", "linux", "win32"] as const) {
      const env = childEnv({}, parent, platform);
      expect(Object.keys(env).filter((k) => k === "ELECTRON_RUN_AS_NODE" || k === "CLAUDECODE" || k.startsWith("CLAUDE_CODE_"))).toEqual([]);
      expect(env.NODE_OPTIONS).toBeUndefined();
      expect(env.FIN_SERVER).toBeUndefined();
    }
  });
});
