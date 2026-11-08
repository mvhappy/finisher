// Helpers of the "auth" e2e scenario (5A Task 12): the console as a program, raw WebSocket
// upgrades, credential files and the leak grep.
import { spawn, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { request } from "node:http";
import { randomBytes } from "node:crypto";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { ROOT, WIN, errorCode, log, outputs } from "./lib.mjs";
import { filesUnder } from "./diagnostics-lib.mjs";

const MAIN = join(ROOT, "apps", "finisherd", "src", "main.ts");

/**
 * `finisherd <args>` as a program, the way a person's shell runs the console, except that its
 * stdin and stdout are pipes: no terminal. Returns `{ status, stdout, stderr, code }` (`code`:
 * the error code on the last `finisherd: CODE:` line of stderr, if any).
 */
export function consoleCli(serverData, args, extraEnv = {}) {
  const out = spawnSync(process.execPath, [MAIN, ...args, "--server-data", serverData], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, ...extraEnv },
    timeout: 60_000,
    input: "",
  });
  if (out.error) throw out.error;
  const code = /finisherd: ([A-Z_]+):/.exec(out.stderr)?.[1];
  outputs.push({ name: `finisherd ${args.join(" ")}`, text: `${out.stdout}\n${out.stderr}` });
  log(`finisherd ${args.join(" ")} (exit ${out.status})${code ? ` → ${code}` : ""}`);
  return { status: out.status, stdout: out.stdout, stderr: out.stderr, code };
}

/**
 * A WebSocket upgrade by hand, so that the request line's headers can be any: resolves with the
 * HTTP status (101 when the server upgraded; the socket is then dropped), the chosen
 * sub-protocol and the body of a refusal. `host` and `origin` replace the defaults (none sent
 * for `origin: null`).
 */
export function wsUpgrade(server, { protocols = [], host, origin = null, path = "/rpc" }) {
  return new Promise((resolve, reject) => {
    const headers = {
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Version": "13",
      "Sec-WebSocket-Key": randomBytes(16).toString("base64"),
    };
    if (protocols.length > 0) headers["Sec-WebSocket-Protocol"] = protocols.join(", ");
    if (host !== undefined) headers.Host = host;
    if (origin !== null) headers.Origin = origin;
    const req = request({ host: "127.0.0.1", port: server.info.wsPort, path, headers, timeout: 10_000 });
    req.on("upgrade", (res, socket) => {
      socket.destroy();
      resolve({ status: 101, protocol: res.headers["sec-websocket-protocol"], body: "" });
    });
    req.on("response", (res) => {
      let body = "";
      res.setEncoding("utf8").on("data", (d) => (body += d));
      res.on("end", () => resolve({ status: res.statusCode, protocol: undefined, body }));
    });
    req.on("timeout", () => req.destroy(new Error("the upgrade timed out")));
    req.on("error", reject);
    req.end();
  });
}

/** Writes `text` as a new owner-only file (0600). */
export function writePrivate(path, text) {
  writeFileSync(path, text, { mode: 0o600 });
  if (!WIN) chmodSync(path, 0o600);
}

/** Writes a minted credential's file (`auth.credential.mint`'s `file`) 0600, as `agent mint --out` does. */
export function writeCredentialFile(path, file) {
  writeFileSync(path, `${JSON.stringify(file)}\n`, { mode: 0o600, flag: "wx" });
  if (!WIN) chmodSync(path, 0o600);
  return path;
}

/**
 * Windows: a folder only this user can use, for the run's TEMP and TMP (the pipe and the key
 * files must not sit in a folder others can write). Elsewhere `dir` as it is.
 */
export function privateTemp(dir) {
  mkdirSync(dir, { recursive: true });
  if (!WIN) return dir;
  const user = `${process.env.USERDOMAIN ?? "."}\\${process.env.USERNAME}`;
  const r = spawnSync("icacls", [dir, "/inheritance:r", "/grant:r", `${user}:(OI)(CI)F`], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`icacls ${dir} failed: ${r.stdout}${r.stderr}`);
  return dir;
}

/**
 * Runs `fin` asynchronously (the event loop stays free to restart a server meanwhile) and
 * resolves with `{ status, json, text, stderr }`.
 */
export function finAsync(fin, serverData, cwd, args, extraEnv = {}) {
  const env = { ...process.env, FIN_SERVER_DATA: serverData, ...extraEnv };
  delete env.FIN_SERVER;
  return new Promise((resolve) => {
    const child = spawn(fin, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
    let text = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (d) => (text += d));
    child.stderr.setEncoding("utf8").on("data", (d) => (stderr += d));
    const timer = setTimeout(() => child.kill("SIGKILL"), 60_000);
    child.once("close", (status) => {
      clearTimeout(timer);
      let json;
      try {
        json = JSON.parse(text.trim());
      } catch {
        json = undefined;
      }
      outputs.push({ name: `fin ${args.join(" ")}`, text: `${text}\n${stderr}` });
      resolve({ status, json, text: text.trim(), stderr });
    });
  });
}

/**
 * Moves a stopped server's credential into the past (the e2e cannot wait out a 60 s minimum
 * expiry): its absolute expiry becomes `at`. The server must not be running.
 */
export function expireCredentialOffline(serverData, id, at) {
  const db = new DatabaseSync(join(serverData, "control.db"));
  try {
    const r = db.prepare("UPDATE credentials SET absoluteExpiryAt = ? WHERE id = ?").run(at, id);
    if (Number(r.changes) !== 1) throw new Error(`credential ${id} is not in control.db`);
    db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  } finally {
    db.close();
  }
}

/**
 * The needles found in `sources` (`{ name, text }`), as `{ source, needle }`. A needle shorter
 * than 16 characters (a challenge code) must stand alone, not inside a longer word.
 */
export function secretHits(sources, needles) {
  const hits = [];
  for (const { name, text } of sources) {
    for (const needle of needles) {
      if (!needle) continue;
      const found =
        needle.length >= 16 ? text.includes(needle) : new RegExp(`(?<![A-Za-z0-9])${needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![A-Za-z0-9])`).test(text);
      if (found) hits.push({ source: name, needle: `${needle.slice(0, 4)}…(${needle.length})` });
    }
  }
  return hits;
}

/** Every file under the server's `events/` and `finisherd.json`, as grep sources (bytes read as latin1). */
export function serverFiles(serverData) {
  const files = filesUnder(join(serverData, "events")).map(([path, bytes]) => ({ name: path.slice(serverData.length), text: bytes.toString("latin1") }));
  const info = join(serverData, "finisherd.json");
  if (existsSync(info)) files.push({ name: "finisherd.json", text: readFileSync(info, "latin1") });
  return files;
}

export { errorCode };
