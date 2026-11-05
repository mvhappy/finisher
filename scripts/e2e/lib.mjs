// Shared helpers of the e2e scenarios: running `fin`, starting finisherd, a WebSocket RPC client.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";

export const ROOT = fileURLToPath(new URL("../..", import.meta.url));
export const WIN = process.platform === "win32";
export const FIN = join(ROOT, "target", "release", WIN ? "fin.exe" : "fin");
export const START_TIMEOUT_MS = Number(process.env.E2E_START_TIMEOUT_MS ?? 30_000);

/** What the watchdog must clean up: the spawned servers and the temp dir, once they exist. */
export const live = { children: new Set(), tmp: undefined };

/** Splits WebSocket message text into NDJSON frames (a message may carry several lines). */
export function parseFrames(text) {
  return text
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line));
}

export function log(message) {
  process.stdout.write(`e2e: ${message}\n`);
}

export function build(cmd, args) {
  log(`$ ${cmd} ${args.join(" ")}`);
  // `pnpm` is a .cmd shim on Windows, which only a shell can run; the args are fixed words,
  // passed as one command line (Node deprecates separate args with `shell`).
  const shell = WIN && cmd === "pnpm";
  const { status } = shell
    ? spawnSync(`${cmd} ${args.join(" ")}`, { cwd: ROOT, stdio: "inherit", shell })
    : spawnSync(cmd, args, { cwd: ROOT, stdio: "inherit" });
  if (status !== 0) throw new Error(`${cmd} ${args.join(" ")} failed (exit ${status})`);
}

/** Runs `fin` and returns `{ status, json }`, whatever the exit code. */
export function finRaw(serverData, cwd, args) {
  const env = { ...process.env, FIN_SERVER_DATA: serverData };
  delete env.FIN_SERVER; // find the server through <server-data>/finisherd.json
  // fin has its own 30 s server timeouts; this bounds a hang the watchdog cannot interrupt
  // (spawnSync blocks the event loop).
  const out = spawnSync(FIN, args, { cwd, encoding: "utf8", env, timeout: 60_000 });
  if (out.error) throw out.error;
  const text = out.stdout.trim();
  log(`fin ${args.join(" ")} (exit ${out.status}) → ${text.length > 160 ? `${text.slice(0, 160)}…` : text}`);
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = undefined;
  }
  return { status: out.status, json, text, stderr: out.stderr };
}

/** Runs `fin` and returns its single JSON document; throws on a non-zero exit. */
export function fin(serverData, cwd, args) {
  const r = finRaw(serverData, cwd, args);
  if (r.status !== 0) throw new Error(`fin ${args.join(" ")} exited ${r.status}: ${r.text} ${r.stderr}`);
  return r.json;
}

export async function waitFor(what, predicate, timeoutMs, onTimeout = () => "") {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = predicate();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}${onTimeout()}`);
    await new Promise((r) => setTimeout(r, 25));
  }
}

export function serverCommand(serverData) {
  if (process.env.E2E_SERVER_CMD) {
    const [cmd, ...args] = JSON.parse(process.env.E2E_SERVER_CMD).map((a) => a.replaceAll("{serverData}", serverData));
    return [cmd, args];
  }
  return [process.execPath, [join(ROOT, "apps", "finisherd", "src", "main.ts"), "--server-data", serverData]];
}

/**
 * Starts `finisherd --server-data <dir>` and waits for its `finisherd.json`. If it never
 * becomes ready, the child is stopped before the error propagates.
 */
export async function startServer(serverData) {
  const [cmd, args] = serverCommand(serverData);
  rmSync(join(serverData, "finisherd.json"), { force: true }); // a stale file from an earlier run
  const child = spawn(cmd, args, { cwd: ROOT, stdio: ["ignore", "ignore", "pipe"] });
  live.children.add(child);
  child.once("exit", () => live.children.delete(child));
  let stderr = "";
  child.stderr.setEncoding("utf8").on("data", (d) => (stderr += d));
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve(code ?? signal)));
  const file = join(serverData, "finisherd.json");
  const server = { child, exited, stderr: () => stderr };
  try {
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
      START_TIMEOUT_MS,
      () => `: ${stderr}`,
    );
  } catch (e) {
    await stopServer(server);
    throw e;
  }
  log(`finisherd pid=${server.info.pid} wsPort=${server.info.wsPort} socket=${server.info.socket}`);
  return server;
}

export async function stopServer(server) {
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
export async function connectWs({ wsPort, token }) {
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

export function assertEqual(actual, expected, what) {
  if (!isDeepStrictEqual(actual, expected)) {
    throw new Error(`${what}: got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
  }
}

/** Removes the temp dir; a failure (a Windows file lock) is logged, never thrown. */
export function removeTmp(tmp) {
  try {
    rmSync(tmp, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  } catch (e) {
    process.stderr.write(`e2e: cleanup: cannot remove ${tmp}: ${e instanceof Error ? e.message : String(e)}\n`);
  }
}

