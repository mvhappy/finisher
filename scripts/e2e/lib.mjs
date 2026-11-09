// Shared helpers of the e2e scenarios: running `fin`, starting finisherd, a WebSocket RPC client.
import { spawn, spawnSync } from "node:child_process";
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";

export const ROOT = fileURLToPath(new URL("../..", import.meta.url));
export const WIN = process.platform === "win32";
/** Where cargo builds: `CARGO_TARGET_DIR` when set (relative to the repository, as the builds below run there), else `target/`. */
export const TARGET_DIR = resolve(ROOT, process.env.CARGO_TARGET_DIR || "target");
export const FIN = join(TARGET_DIR, "release", WIN ? "fin.exe" : "fin");
/** The publisher tooling (`cargo build --release -p fin-plugin-cli`): not part of the app. */
export const FIN_PLUGIN = join(TARGET_DIR, "release", WIN ? "fin-plugin.exe" : "fin-plugin");
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

/**
 * A fresh temp dir for a scenario, as a real path. The server's socket lives in it
 * (`<dir>/<server-data>/run/fin.sock`) and a unix socket path may hold 104 bytes (macOS; Linux
 * 108): when `os.tmpdir()` is deep (macOS's `/var/folders/…/T`) the dir goes under `/tmp`
 * instead, so no scenario depends on how long the user's `TMPDIR` is. Names under it must stay
 * short (`prefix` ≤ 20 characters, server-data `s`).
 */
export function scenarioTmp(prefix) {
  const probe = `${prefix}XXXXXX/server-data/run/fin.sock`;
  let base = realpathSync(tmpdir());
  if (!WIN && Buffer.byteLength(join(base, probe)) > 100 && existsSync("/tmp")) base = realpathSync("/tmp");
  return realpathSync(mkdtempSync(join(base, prefix)));
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

/**
 * The release `fin` (built with `-p fin` alone, so no dev-dependency turns `test-hooks` on)
 * ignores its test hooks: with `FIN_OFFLINE_SNAPSHOT` naming a missing file it still looks for
 * a server instead of failing to read the snapshot (5A Decision 37).
 */
export function assertNoTestHooks() {
  const r = spawnSync(FIN, ["view", "--project", ROOT], {
    encoding: "utf8",
    env: { ...process.env, FIN_SERVER: join(ROOT, "target", "no-such.sock"), FIN_OFFLINE_SNAPSHOT: join(ROOT, "target", "no-such-snapshot.json"), FIN_TEST_COMMIT_GATE: "x" },
  });
  const code = JSON.parse(r.stdout || "{}").error?.code;
  if (code !== "SERVER_UNAVAILABLE") throw new Error(`the release fin reads a test hook: ${r.stdout}${r.stderr}`);
  log("release fin ignores its test hooks");
}

/** Runs `fin` and returns `{ status, json }`, whatever the exit code. */
export function finRaw(serverData, cwd, args, extraEnv = {}) {
  // Find the server through <server-data>/finisherd.json, unless the caller names it (a
  // credential's `fin` takes the endpoint from FIN_SERVER alone: 7A Decision 14b).
  const env = { ...process.env, FIN_SERVER_DATA: serverData };
  delete env.FIN_SERVER;
  Object.assign(env, extraEnv);
  // fin has its own 30 s server timeouts; this bounds a hang the watchdog cannot interrupt
  // (spawnSync blocks the event loop).
  const out = spawnSync(FIN, args, { cwd, encoding: "utf8", env, timeout: 60_000 });
  if (out.error) throw out.error;
  const text = out.stdout.trim();
  outputs.push({ name: `fin ${args.join(" ")}`, text: `${out.stdout}\n${out.stderr}` });
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
export function fin(serverData, cwd, args, extraEnv = {}) {
  const r = finRaw(serverData, cwd, args, extraEnv);
  if (r.status !== 0) throw new Error(`fin ${args.join(" ")} exited ${r.status}: ${r.text} ${r.stderr}`);
  return r.json;
}

export async function waitFor(what, predicate, timeoutMs, onTimeout = () => "", intervalMs = 25) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = predicate();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}${onTimeout()}`);
    await new Promise((r) => setTimeout(r, intervalMs));
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
 * Starts `finisherd --server-data <dir>` (with `env` added to this process's environment) and
 * waits for its `finisherd.json`. If it never becomes ready, the child is stopped before the
 * error propagates.
 */
export async function startServer(serverData, env = {}) {
  const [cmd, args] = serverCommand(serverData);
  rmSync(join(serverData, "finisherd.json"), { force: true }); // a stale file from an earlier run
  const child = spawn(cmd, args, { cwd: ROOT, stdio: ["ignore", "ignore", "pipe"], env: { ...process.env, ...env } });
  live.children.add(child);
  child.once("exit", () => live.children.delete(child));
  let stderr = "";
  child.stderr.setEncoding("utf8").on("data", (d) => (stderr += d));
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve(code ?? signal)));
  const file = join(serverData, "finisherd.json");
  const server = { child, exited, serverData, stderr: () => stderr };
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

/** HMAC-SHA-256 over the handshake's input: the label, then each field (strings length-prefixed). */
export function handshakeMac(key, label, fields) {
  const parts = [Buffer.from(label, "ascii")];
  for (const f of fields) {
    if (typeof f !== "string") {
      parts.push(f);
      continue;
    }
    const b = Buffer.from(f, "utf8");
    const len = Buffer.alloc(2);
    len.writeUInt16BE(b.length);
    parts.push(len, b);
  }
  return createHmac("sha256", key).update(Buffer.concat(parts)).digest();
}

/** Secret strings this run saw (keys, nonces, proofs, MACs, tickets, challenge codes): the `auth` scenario's leak grep looks for them. */
export const observed = new Set();
const note = (v) => {
  if (typeof v === "string" && v.length > 0) observed.add(v);
};
/** Every `fin` and console output of the run, named by its command line: the leak grep reads them too. */
export const outputs = [];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Connects to the server's socket (Windows: its pipe, waiting out `ERROR_PIPE_BUSY` while the
 * listener has no free instance for a moment, which libuv reports as `EBUSY` or `EAGAIN`).
 */
async function connectLocal(path) {
  const deadline = Date.now() + (WIN ? 5_000 : 0);
  for (;;) {
    try {
      return await new Promise((resolve, reject) => {
        const sock = connect({ path });
        sock.once("connect", () => resolve(sock));
        sock.once("error", reject);
      });
    } catch (e) {
      if (!WIN || !["EBUSY", "EAGAIN"].includes(e?.code) || Date.now() > deadline) throw e;
      await sleep(25);
    }
  }
}

/** The first error code in an Exit, an error or a message: `"code":"X"`. */
export const errorCode = (x) => /"code":"([A-Z_]+)"/.exec(typeof x === "string" ? x : x instanceof Error ? x.message : JSON.stringify(x))?.[1];

/**
 * A raw NDJSON connection to the server's socket with nothing proved (5A Task 12, step 1): `call`
 * resolves with the answer's Exit frame, whatever it holds (`{_tag: "Closed"}` when the connection
 * ends first); `closed` resolves when it ends.
 */
export async function rawConnect(server) {
  const sock = await connectLocal(server.info.socket);
  sock.setEncoding("utf8");
  let buf = "";
  let nextId = 1;
  const waiting = new Map();
  const chunks = new Map();
  const closed = new Promise((resolve) => {
    sock.once("close", () => {
      for (const done of waiting.values()) done({ _tag: "Closed" });
      waiting.clear();
      resolve();
    });
  });
  sock.on("error", () => {}); // a reset ends it; `closed` says so
  sock.on("data", (d) => {
    buf += d;
    let nl;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const frame = JSON.parse(buf.slice(0, nl));
      buf = buf.slice(nl + 1);
      if (frame._tag === "Chunk" && chunks.has(frame.requestId)) {
        chunks.get(frame.requestId)(frame.values);
        sock.write(`${JSON.stringify({ _tag: "Ack", requestId: frame.requestId })}\n`);
      } else if (frame._tag === "Exit") {
        chunks.delete(frame.requestId);
        waiting.get(frame.requestId)?.(frame.exit);
      }
    }
  });
  return {
    /** A streaming call: each chunk's values to `onValues`; resolves with its Exit. */
    stream: (tag, payload, onValues) =>
      new Promise((done) => {
        const id = String(nextId++);
        chunks.set(id, onValues);
        waiting.set(id, done);
        sock.write(`${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`);
      }),
    call: (tag, payload) =>
      new Promise((done) => {
        const id = String(nextId++);
        waiting.set(id, done);
        sock.write(`${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`);
      }),
    close: () => sock.end(),
    closed,
  };
}

const success = (exit, what) => {
  if (exit._tag !== "Success") throw new Error(`${what} failed: ${JSON.stringify(exit)}`);
  return exit.value;
};

/**
 * Connects and runs the handshake `fin` runs (5A Decisions 6–9), checking the server's proof first.
 * `credential` is `local` or `console` (`<server-data>/auth/<name>.key`, or `keyFile`) or a
 * credential file's `{ kind, id, key }`. The connection's `call` answers Exit frames; `close` ends it.
 */
export async function proveConnection(server, credential = "local", { keyFile } = {}) {
  const named = typeof credential === "string";
  const key = Buffer.from(named ? readFileSync(keyFile ?? join(server.serverData, "auth", `${credential}.key`), "utf8").trim() : credential.key, "base64url");
  const who = named ? { kind: credential } : { kind: credential.kind, id: credential.id };
  const conn = await rawConnect(server);
  try {
    const cn = randomBytes(32);
    const hello = success(await conn.call("server.hello", { clientNonce: cn.toString("base64url"), credential: who }), "server.hello");
    const sn = Buffer.from(hello.serverNonce, "base64url");
    note(hello.serverNonce);
    note(hello.proof);
    note(cn.toString("base64url"));
    const proof = Buffer.from(hello.proof ?? "", "base64url");
    const want = handshakeMac(key, "finisherd/hello/v2", [hello.envId, hello.endpoint, cn, sn]);
    if (proof.length !== want.length || !timingSafeEqual(proof, want)) throw new Error("the server did not prove itself");
    const mac = handshakeMac(key, "fin/prove/v2", [hello.envId, hello.endpoint, sn, cn]).toString("base64url");
    note(mac);
    const proved = success(await conn.call("auth.prove", { mac }), "auth.prove");
    return { ...conn, principal: proved.principal };
  } catch (e) {
    conn.close();
    throw e;
  }
}

/**
 * Proves `credential` over the server's socket and calls `tag` once (5A Decision 6): the same
 * handshake `fin` runs. With `confirm`, a console asks for a `console.challenge` first and sends
 * its code in `challenge`, as `finisherd` does once the person has confirmed (5A Decision 30, owner
 * decision of 2026-11-08: the mutating human-only calls).
 */
export async function socketCall(server, tag, payload, credential = "local", { confirm = false, keyFile } = {}) {
  const conn = await proveConnection(server, credential, { keyFile });
  try {
    let challenge = {};
    if (confirm) {
      const code = success(await conn.call("console.challenge", {}), "console.challenge").code;
      note(code);
      challenge = { challenge: code };
    }
    const value = success(await conn.call(tag, { ...payload, ...challenge }), tag);
    if (tag === "auth.ticket") note(value.ticket);
    return value;
  } finally {
    conn.close();
  }
}

/**
 * An RPC as the server's console makes it: proved as `console`, and for a mutation (the default)
 * with a fresh `console.challenge` code of its own connection; `{ confirm: false }` for a read.
 */
export function consoleRpc(server, tag, payload, { confirm = true } = {}) {
  return socketCall(server, tag, payload, "console", { confirm });
}

/** A mutating human-only call as the server's console makes it. */
export const consoleCall = (server, tag, payload) => consoleRpc(server, tag, payload);

/**
 * `finisherd library add <root>` as the console runs it (5D Task 14): `library.add` with the
 * console's typed code. Answers `{ library, job? }`, `job` being the first full scan.
 */
export const consoleLibraryAdd = (server, root, options = {}) => consoleCall(server, "library.add", { root, ...options });

/** Every entry the journal stored (content included: the console is a person), oldest first. */
export async function storedEntries(server, filter = {}) {
  const out = [];
  let cursor;
  for (;;) {
    const page = await socketCall(server, "diagnostics.export", { filter, content: true, ...(cursor ? { cursor } : {}) }, "console");
    out.push(...page.lines);
    if (page.next === undefined) return out;
    cursor = page.next;
  }
}

/**
 * A minimal Effect-RPC client over the WebSocket (NDJSON frames). Since 5A Task 6 it mints a
 * one-time ticket over the socket (`auth.ticket`) and offers it in `Sec-WebSocket-Protocol`
 * beside `finisher.v2`; the connection then runs as the ticket's principal.
 */
export async function connectWs(server, credential = "local") {
  const { ticket } = await socketCall(server, "auth.ticket", {}, credential);
  const ws = new WebSocket(`ws://127.0.0.1:${server.info.wsPort}/rpc`, ["finisher.v2", `finisher.ticket.${ticket}`]);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = () => reject(new Error("websocket failed to open"));
  });
  if (ws.protocol !== "finisher.v2") throw new Error(`the server chose ${ws.protocol}`);
  let nextId = 1;
  const exits = new Map();
  const chunks = new Map();
  const closed = new Promise((resolve) => {
    ws.onclose = () => {
      for (const done of exits.values()) done({ _tag: "Closed" });
      exits.clear();
      resolve();
    };
  });
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
    /** Resolves when the connection ends (a revocation closes it); pending requests then answer `{ _tag: "Closed" }`. */
    closed,
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

