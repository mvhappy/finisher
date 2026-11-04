#!/usr/bin/env node
// End-to-end check: fin → finisherd → WebSocket subscriber.
//
// Builds `fin` (release) and fin-node, starts finisherd in a temp server-data dir, opens an
// `ops.subscribe` stream from rev 0 over the bearer-token WebSocket, runs the editing flow
// through the `fin` binary, and asserts the subscriber saw every committed rev in order and
// that its last rev equals `snapshot.get`'s. The server and the temp dir are always cleaned up.
//
// Usage: node scripts/e2e.mjs [--skip-build]
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const WIN = process.platform === "win32";
const FIN = join(ROOT, "target", "release", WIN ? "fin.exe" : "fin");

/** Splits WebSocket message text into NDJSON frames (a message may carry several lines). */
export function parseFrames(text) {
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line));
}

/**
 * Checks that `seen` (the subscriber's revs, in arrival order) is exactly `committed`
 * (the revs `fin` reported, in commit order). Returns an error message or `undefined`.
 */
export function checkRevs(seen, committed) {
  const want = [...committed].sort((a, b) => a - b);
  for (let i = 1; i < want.length; i++) {
    if (want[i] !== want[i - 1] + 1) return `fin's revs are not contiguous: ${JSON.stringify(committed)}`;
  }
  if (JSON.stringify(seen) !== JSON.stringify(want)) {
    return `subscriber saw revs ${JSON.stringify(seen)}, expected ${JSON.stringify(want)} in order`;
  }
  return undefined;
}

function log(message) {
  process.stdout.write(`e2e: ${message}\n`);
}

function build(cmd, args) {
  log(`$ ${cmd} ${args.join(" ")}`);
  // `pnpm` is a .cmd shim on Windows, which only a shell can run.
  const { status } = spawnSync(cmd, args, { cwd: ROOT, stdio: "inherit", shell: WIN && cmd === "pnpm" });
  if (status !== 0) throw new Error(`${cmd} ${args.join(" ")} failed (exit ${status})`);
}

/** Runs `fin` and returns its single JSON document; throws on a non-zero exit. */
function fin(serverData, cwd, args) {
  const env = { ...process.env, FIN_SERVER_DATA: serverData };
  delete env.FIN_SERVER; // find the server through <server-data>/finisherd.json
  const out = spawnSync(FIN, args, { cwd, encoding: "utf8", env });
  if (out.error) throw out.error;
  const text = out.stdout.trim();
  if (out.status !== 0) throw new Error(`fin ${args.join(" ")} exited ${out.status}: ${text} ${out.stderr}`);
  log(`fin ${args.join(" ")} → ${text.length > 160 ? `${text.slice(0, 160)}…` : text}`);
  return JSON.parse(text);
}

async function waitFor(what, predicate, timeoutMs, onTimeout = () => "") {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = predicate();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}${onTimeout()}`);
    await new Promise((r) => setTimeout(r, 25));
  }
}

/** Starts `finisherd --server-data <dir>` and waits for its `finisherd.json`. */
async function startServer(serverData) {
  const child = spawn(process.execPath, [join(ROOT, "apps", "finisherd", "src", "main.ts"), "--server-data", serverData], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8").on("data", (d) => (stderr += d));
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve(code ?? signal)));
  const file = join(serverData, "finisherd.json");
  const server = { child, exited, stderr: () => stderr };
  server.info = await waitFor(
    "finisherd.json",
    () => {
      if (child.exitCode !== null) throw new Error(`finisherd exited ${child.exitCode}: ${stderr}`);
      if (!existsSync(file)) return undefined;
      try {
        return JSON.parse(readFileSync(file, "utf8"));
      } catch {
        return undefined; // mid-write
      }
    },
    30_000,
    () => `: ${stderr}`,
  );
  log(`finisherd pid=${server.info.pid} wsPort=${server.info.wsPort} socket=${server.info.socket}`);
  return server;
}

async function stopServer(server) {
  if (server.child.exitCode !== null || server.child.signalCode !== null) return;
  server.child.kill("SIGTERM"); // TerminateProcess on Windows
  const result = await Promise.race([server.exited, new Promise((r) => setTimeout(() => r("timeout"), 10_000))]);
  if (result === "timeout") {
    server.child.kill("SIGKILL");
    await server.exited;
  }
  log(`finisherd stopped (${result})`);
}

/** A minimal Effect-RPC client over the bearer-token WebSocket (NDJSON frames). */
async function connectWs({ wsPort, token }) {
  // Node's WebSocket (undici) accepts `headers` in its options object.
  const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`, { headers: { Authorization: `Bearer ${token}` } });
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error("websocket failed to open"));
  });
  let nextId = 1;
  const exits = new Map();
  const chunks = new Map();
  ws.onmessage = (event) => {
    for (const frame of parseFrames(String(event.data))) {
      if (frame._tag === "Chunk") {
        chunks.get(frame.requestId)?.(frame.values);
        ws.send(`${JSON.stringify({ _tag: "Ack", requestId: frame.requestId })}\n`);
      } else if (frame._tag === "Exit") {
        chunks.delete(frame.requestId);
        exits.get(frame.requestId)?.(frame.exit);
        exits.delete(frame.requestId);
      }
    }
  };
  const request = (tag, payload, onValues) => {
    const id = String(nextId++);
    if (onValues) chunks.set(id, onValues);
    const done = new Promise((resolve) => exits.set(id, resolve));
    ws.send(`${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`);
    return { id, done };
  };
  return {
    async call(tag, payload) {
      const exit = await request(tag, payload).done;
      if (exit._tag !== "Success") throw new Error(`${tag} failed: ${JSON.stringify(exit)}`);
      return exit.value;
    },
    subscribe(payload, onValue) {
      return request("ops.subscribe", payload, (values) => values.forEach(onValue));
    },
    interrupt(id) {
      ws.send(`${JSON.stringify({ _tag: "Interrupt", requestId: id })}\n`);
    },
    close() {
      ws.close();
    },
  };
}

/** Writes a real 10 s MP4 with fin-probe's fixture writer (fin-node `test-hooks` build). */
async function writeFixture(path) {
  process.env.FIN_NODE_BINARY_DIR = join(ROOT, "packages", "fin-node", "test", "native");
  const { testHooks } = await import(pathToFileURL(join(ROOT, "packages", "fin-node", "index.js")).href);
  if (!testHooks) throw new Error("fin-node test-hooks build is missing (pnpm --filter @finisher/fin-node build:test)");
  testHooks.writeFixtureMp4(path, { seconds: 10, fps: 30, audio: true, sampleRate: 48000 });
}

function assertEqual(actual, expected, what) {
  if (!isDeepStrictEqual(actual, expected)) {
    throw new Error(`${what}: got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
  }
}

async function main(argv) {
  const started = Date.now();
  if (!argv.includes("--skip-build")) {
    build("cargo", ["build", "--release", "-p", "fin"]);
    build("pnpm", ["--filter", "@finisher/fin-node", "build"]);
    build("pnpm", ["--filter", "@finisher/fin-node", "build:test"]);
  }

  // realpath: macOS tmp is a symlink (/var → /private/var), and fin echoes resolved paths.
  const tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisher-e2e-")));
  const serverData = join(tmp, "server-data");
  let server;
  let ws;
  try {
    server = await startServer(serverData);
    const project = join(tmp, "мій фільм");
    const created = fin(serverData, tmp, ["project", "create", project, "--name", "E2E"]);
    const projectId = created.project.projectId;
    assertEqual(created.project.rev, 0, "new project rev");

    ws = await connectWs(server.info);
    const seen = [];
    const sub = ws.subscribe({ projectId, branch: "main", fromRev: 0 }, (tx) => seen.push(tx));
    let subExit;
    sub.done.then((exit) => (subExit = exit));

    await writeFixture(join(project, "кліп 1.mp4"));
    const writes = [
      ["asset", "add", "кліп 1.mp4"],
      ["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"],
      ["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur", "3s"],
      ["clip", "trim", "c1", "--end", "-1s"],
      ["clip", "delete", "c1"],
    ];
    const committed = writes.map((args) => fin(serverData, project, args));
    const view = fin(serverData, project, ["view"]);
    assertEqual(
      view.tracks,
      [
        { id: "t1", kind: "video", clips: [{ id: "c2", start: "0", end: "3", asset: "a1", sourceIn: "4" }] },
        { id: "t2", kind: "audio", clips: [] },
      ],
      "fin view tracks",
    );

    const revs = committed.map((w) => w.rev);
    await waitFor(
      `${revs.length} revs on the subscriber`,
      () => {
        if (subExit) throw new Error(`subscription ended early: ${JSON.stringify(subExit)}`);
        return seen.length >= revs.length;
      },
      15_000,
      () => ` (saw ${JSON.stringify(seen.map((t) => t.rev))})`,
    );
    // Anything duplicated or late would arrive within a short grace period.
    await new Promise((r) => setTimeout(r, 300));
    const problem = checkRevs(
      seen.map((t) => t.rev),
      revs,
    );
    if (problem) throw new Error(problem);
    assertEqual(
      seen.map((t) => t.tx.txId),
      committed.map((w) => w.txId),
      "subscriber txIds",
    );

    const snapshot = await ws.call("snapshot.get", { projectId, branch: "main" });
    assertEqual(snapshot.rev, seen.at(-1).rev, "snapshot.get rev vs last CommittedTx.rev");

    ws.interrupt(sub.id);
    await sub.done;
    log(`subscriber saw revs ${JSON.stringify(seen.map((t) => t.rev))}; snapshot.get rev ${snapshot.rev}`);
    log(`PASS in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    ws?.close();
    if (server) await stopServer(server);
    rmSync(tmp, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main(process.argv.slice(2)).catch((e) => {
    process.stderr.write(`e2e: FAIL: ${e instanceof Error ? e.message : String(e)}\n`);
    process.exitCode = 1;
  });
}
