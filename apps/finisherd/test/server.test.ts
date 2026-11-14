import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { lockExclusive } from "@finisher/fin-node";
import { socketPath } from "../src/config.ts";
import { childEnv } from "../src/env.ts";
import { until } from "./support/jobs.ts";
import { commit, generationOf, addAsset, connectProved, insert, NdjsonClient, ok, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

/**
 * Windows has no SIGTERM: `stop()` is TerminateProcess, so there is no graceful shutdown (no
 * exit 0, `finisherd.json` stays behind until the next start replaces it). There are no Unix
 * modes either, and LockFileEx locks are mandatory (the lock file is unreadable while held).
 */
const WIN = process.platform === "win32";

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
  if (server) {
    const code = await server.stop();
    if (!WIN) expect(code).toBe(0);
  }
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function start(): Promise<RunningServer> {
  server = await startServer(serverData);
  return server;
}

async function client(): Promise<NdjsonClient> {
  const c = await connectProved(server!);
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
  test("server_starts_with_the_job_engine", async () => {
    const { stderr } = await start();
    // One start line with the host's capabilities: the sandbox spawner, applying no profile
    // until one is ready on this OS (plan 7C Tasks 7 and 16).
    await until(() => /finisherd job engine: killTree=\w+ .*sandboxed=false/.test(stderr()));
    const line = stderr().split("\n").find((l) => l.includes("finisherd job engine:")) ?? "";
    // Job Objects on Windows; a delegated cgroup or process groups on Linux; process groups on macOS.
    expect(line).toMatch(WIN ? /killTree=jobObject/ : process.platform === "linux" ? /killTree=(cgroup|processGroup)/ : /killTree=processGroup/);
    expect(line).toContain("sandbox=fin-sandbox");
    // The engine's start ran: an empty jobs root, and the store's chain is in control.db.
    expect(readdirSync(join(serverData, "jobs"))).toEqual([]);
  });

  test.skipIf(WIN)("jobs_dir_is_private", async () => {
    await start();
    expect(statSync(join(serverData, "jobs")).mode & 0o777).toBe(0o700);
  });

  test("hello returns protocol 2; finisherd.json and run/fin.sock are 0600; finisherd_json_has_no_token", async () => {
    const { info } = await start();
    expect(info.protocolVersion).toBe(2);
    expect(info.socket).toBe(WIN ? socketPath(serverData) : join(serverData, "run", "fin.sock"));
    expect(Object.keys(JSON.parse(readFileSync(join(serverData, "finisherd.json"), "utf8"))).sort()).toEqual(
      ["diagnostics", "envId", "pid", "protocolVersion", "socket", "wsPort"],
    );
    if (!WIN) {
      expect(statSync(join(serverData, "finisherd.json")).mode & 0o777).toBe(0o600);
      expect(statSync(info.socket).mode & 0o777).toBe(0o600);
    }

    const c = await client();
    const hello = ok<{ protocolVersion: number; capabilities: string[] }>(await c.call("server.hello", {}));
    expect(hello.protocolVersion).toBe(2);
    expect(hello.capabilities).toContain("ops.subscribe");
    c.send({ _tag: "Ping" });
    expect((await c.nextFrame())._tag).toBe("Pong");
  });

  test("create, commit and snapshot over the socket", async () => {
    await start();
    const c = await client();
    const projectId = await createProject(c);
    const c1 = ok<{ rev: number; bindings: Record<string, string> }>(
      await commit(c, projectId, tx("tx-1", 0, [addAsset()])),
    );
    expect(c1).toMatchObject({ rev: 1, bindings: { $a: "a1" } });
    ok(await commit(c, projectId, tx("tx-2", 1, [insert(0, 5)])));

    const snap = ok<{ rev: number; project: { timelines: Array<{ tracks: Array<{ items: unknown[] }> }> } }>(
      await c.call("snapshot.get", { projectId, branch: "main" }),
    );
    expect(snap.rev).toBe(2);
    expect(snap.project.timelines[0]!.tracks[0]!.items).toMatchObject([{ id: "c1", start: "0", duration: "5" }]);

    const again = ok<{ projectId: string; rev: number }>(await c.call("project.open", { dir: join(tmp, "My Проєкт") }));
    expect(again).toEqual({ projectId, rev: 2 });
  });

  test("errors arrive as FinError with a stable code", async () => {
    await start();
    const c = await client();
    const projectId = await createProject(c);
    ok(await commit(c, projectId, tx("tx-asset", 0, [addAsset()])));
    ok(await commit(c, projectId, tx("tx-base", 1, [insert(0, 5)])));
    ok(await commit(c, projectId, tx("tx-del", 2, [{ op: "clip.delete", clip: "c1" }])));

    const conflict = await commit(c, projectId, tx("tx-late", 2, [{ op: "clip.delete", clip: "c1" }]));
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
    const sub = c.stream("ops.subscribe", { projectId: "nope", branch: "main", fromRev: 0, generation: { incarnation: "i", n: 0 } }, () => undefined);
    const subExit = await sub.done;
    expect(!subExit.ok && subExit.error.code).toBe("PROJECT_NOT_FOUND");
  });

  test("subscribe_during_commits_no_gap_no_dup", async () => {
    await start();
    const writer = await client();
    const reader = await client();
    const projectId = await createProject(writer);
    ok(await commit(writer, projectId, tx("tx-1", 0, [addAsset()])));
    ok(await commit(writer, projectId, tx("tx-2", 1, [insert(0, 5)])));
    ok(await commit(writer, projectId, tx("tx-3", 2, [insert(5, 5)])));

    // 50 concurrent commits, all against base rev 3 (asset.add never conflicts).
    const commits = Array.from({ length: 50 }, (_, i) =>
      commit(writer, projectId, tx(`tx-c${i}`, 3, [addAsset(`$a${i}`)])),
    );
    // Join once the first of them has landed, so part arrives as backlog and part live.
    await Promise.race(commits);

    const revs: number[] = [];
    let resolveAll!: () => void;
    const all = new Promise<void>((r) => (resolveAll = r));
    const generation = await generationOf(reader, projectId);
    const sub = reader.stream("ops.subscribe", { projectId, branch: "main", fromRev: 3, generation }, (v) => {
      revs.push((v as { rev: number }).rev);
      if (revs.length >= 50) resolveAll();
    });
    for (const exit of await Promise.all(commits)) ok(exit);
    await all;
    // Any duplicate or late extra would show up within a short grace period.
    await new Promise((r) => setTimeout(r, 200));
    expect(revs).toEqual(Array.from({ length: 50 }, (_, i) => i + 4));

    // A commit after the burst still arrives live.
    ok(await commit(writer, projectId, tx("tx-after", 53, [addAsset("$z")])));
    await expect.poll(() => revs.at(-1)).toBe(54);
    reader.interrupt(sub.id);
    await sub.done;
  });

  test("restart keeps projects; SIGTERM removes fin.sock and finisherd.json and releases the lock", async () => {
    await start();
    let c = await client();
    const projectId = await createProject(c);
    ok(await commit(c, projectId, tx("tx-1", 0, [addAsset()])));
    await clients.splice(0)[0]!.close();
    const code = await server!.stop();
    server = undefined;
    if (!WIN) {
      expect(code).toBe(0);
      for (const f of ["finisherd.json", join("run", "fin.sock")]) expect(existsSync(join(serverData, f)), f).toBe(false);
    }
    // The lock file stays (never unlinked) but is released: this process can take it.
    lockExclusive(join(serverData, "finisherd.lock")).release();

    await start();
    c = await client();
    // By id, straight from the app.db index…
    const snap = ok<{ rev: number }>(await c.call("snapshot.get", { projectId, branch: "main" }));
    expect(snap.rev).toBe(1);
    // …and by directory.
    expect(ok(await c.call("project.open", { dir: join(tmp, "My Проєкт") }))).toEqual({ projectId, rev: 1 });
  });

  test("a second server on the same server-data refuses to start; the first stays reachable", async () => {
    await start();
    await expect(startServer(serverData, 5_000)).rejects.toThrow(/SERVER_RUNNING/);
    const c = await client();
    expect(ok<{ protocolVersion: number }>(await c.call("server.hello", {})).protocolVersion).toBe(2);
  });

  test("servers on two server-data dirs run side by side (distinct sockets/pipes)", async () => {
    const { info } = await start();
    const other = await startServer(join(tmp, "other-server-data"));
    try {
      expect(other.info.socket).not.toBe(info.socket);
      for (const socket of [info.socket, other.info.socket]) {
        const c = await NdjsonClient.connect(socket);
        expect(ok<{ protocolVersion: number }>(await c.call("server.hello", {})).protocolVersion).toBe(2);
        await c.close();
      }
    } finally {
      await other.stop();
    }
  });

  test("two servers racing over a stale socket: exactly one wins", async () => {
    mkdirSync(join(serverData, "run"), { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "run", "fin.sock"), ""); // stale leftover, nothing listening
    const results = await Promise.allSettled([startServer(serverData, 10_000), startServer(serverData, 10_000)]);
    const winners = results.filter((r): r is PromiseFulfilledResult<RunningServer> => r.status === "fulfilled");
    const losers = results.filter((r): r is PromiseRejectedResult => r.status === "rejected");
    expect(winners).toHaveLength(1);
    expect(String(losers[0]!.reason)).toMatch(/SERVER_RUNNING/);
    server = winners[0]!.value;
    const c = await client();
    ok(await c.call("server.hello", {}));
    if (!WIN) expect(readFileSync(join(serverData, "finisherd.lock"), "utf8").trim()).toBe(String(server.info.pid));
  });

  test("a lock file left by a crashed process (no lock held) does not block startup", async () => {
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "finisherd.lock"), ""); // nobody holds it: as after a crash
    const { info } = await start();
    if (!WIN) expect(readFileSync(join(serverData, "finisherd.lock"), "utf8").trim()).toBe(String(info.pid));
  });

  test("filesystem failures at startup are SERVER_START_FAILED with a cause", async () => {
    const notADir = join(tmp, "plain-file");
    writeFileSync(notADir, "");
    await expect(startServer(join(notADir, "sd"), 5_000)).rejects.toThrow(/SERVER_START_FAILED: .*"cause":"ENOTDIR"/);
  });

  test("an existing 0755 server-data becomes 0700; a symlinked one is refused", async () => {
    mkdirSync(serverData, { mode: 0o755 });
    chmodSync(serverData, 0o755);
    await start();
    if (!WIN) expect(statSync(serverData).mode & 0o777).toBe(0o700);
    expect(server!.child.exitCode).toBeNull();

    const link = join(tmp, "link");
    // A junction needs no privilege on Windows and is a symlink to lstat.
    symlinkSync(serverData, link, WIN ? "junction" : "dir");
    await expect(startServer(link, 5_000)).rejects.toThrow(/SERVER_DATA_UNSAFE/);
  });
});

describe("socketPath", () => {
  const appData = "C:\\Users\\ann\\AppData\\Local\\Finisher";

  test("Unix: run/fin.sock inside server-data", () => {
    expect(socketPath("/srv/sd", "linux", "/home/ann/.local/share/finisher", "ann")).toBe(join("/srv/sd", "run", "fin.sock"));
  });

  test("Windows: the default server-data owns finisher-<username>, others get a hashed pipe", () => {
    expect(socketPath(appData, "win32", appData, "ann")).toBe("\\\\.\\pipe\\finisher-ann");
    // Case and a trailing separator do not make a different server-data on Windows.
    expect(socketPath(`${appData.toUpperCase()}\\`, "win32", appData, "ann")).toBe("\\\\.\\pipe\\finisher-ann");

    const temp = socketPath("C:\\Temp\\sd-1", "win32", appData, "ann");
    expect(temp).toMatch(/^\\\\\.\\pipe\\finisher-ann-[0-9a-f]{12}$/);
    expect(socketPath("c:\\temp\\SD-1", "win32", appData, "ann")).toBe(temp);
    expect(socketPath("C:\\Temp\\sd-2", "win32", appData, "ann")).not.toBe(temp);
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
