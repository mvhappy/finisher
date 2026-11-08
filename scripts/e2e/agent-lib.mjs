// Helpers of the "agent" e2e scenario (7A Task 13): reading the agent tables of `control.db`,
// waiting for a session, the journal stores' files, and the leak grep.
import { spawn } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { ROOT, consoleRpc, log } from "./lib.mjs";

export const MAIN = join(ROOT, "apps", "finisherd", "src", "main.ts");

/** Runs `f` over a read-only handle on the server's `control.db` (the server keeps running: WAL). */
export function controlDb(serverData, f) {
  const db = new DatabaseSync(join(serverData, "control.db"), { readOnly: true });
  try {
    return f(db);
  } finally {
    db.close();
  }
}

/** The session's nodes, oldest first: `{ seq, kind, callId, payload }` (payload parsed when inline). */
export function nodesOf(serverData, sessionId) {
  return controlDb(serverData, (db) =>
    db
      .prepare("SELECT seq, kind, callId, payload FROM nodes WHERE sessionId = ? ORDER BY seq")
      .all(sessionId)
      .map((n) => ({ ...n, payload: n.payload === null ? null : JSON.parse(n.payload) })),
  );
}

/** The session's `session_txs` rows, oldest first. */
export const txsOf = (serverData, sessionId) =>
  controlDb(serverData, (db) => db.prepare("SELECT txId, callId, attribution, outcome FROM session_txs WHERE sessionId = ? ORDER BY at, txId").all(sessionId));

/** The newest outcome per tx of a session, as `session_tx_outcomes` holds them (`via`, `verdict`, `by`). */
export const outcomesOf = (serverData, sessionId) =>
  controlDb(serverData, (db) => db.prepare("SELECT txId, via, verdict, by FROM session_tx_outcomes WHERE sessionId = ? ORDER BY seq").all(sessionId));

/** The session as `agent.session.get` answers it (a read: no typed code). */
export const getSession = async (server, sessionId) => (await consoleRpc(server, "agent.session.get", { sessionId }, { confirm: false })).session;

/** Waits until the session's status is one of `statuses`; returns the session. */
export async function waitSession(server, sessionId, statuses, timeoutMs = 60_000) {
  let seen;
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    seen = await Promise.race([getSession(server, sessionId), new Promise((_, no) => setTimeout(() => no(new Error(`agent.session.get did not answer within 15 s: ${server.stderr().slice(-1500)}`)), 15_000))]);
    if (statuses.includes(seen.status)) return seen;
    if (Date.now() > deadline) throw new Error(`timed out waiting for session ${sessionId} to be ${statuses.join(" or ")} (it is ${seen.status}: ${JSON.stringify(seen.statusReason ?? null)})`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

/** A long-running `finisherd agent watch <id>`: its lines, and when it exits. */
export function startWatch(serverData, sessionId) {
  const child = spawn(process.execPath, [MAIN, "agent", "watch", sessionId, "--server-data", serverData], { cwd: ROOT, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  let err = "";
  child.stdout.setEncoding("utf8").on("data", (d) => (out += d));
  child.stderr.setEncoding("utf8").on("data", (d) => (err += d));
  const exited = new Promise((resolve) => child.once("close", (status) => resolve(status)));
  return { lines: () => out.split("\n").filter(Boolean), stderr: () => err, exited, kill: () => child.kill("SIGKILL") };
}

/** Every file under `dir` as `[path, bytes]`; none when it is missing. */
export function filesIn(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? filesIn(p) : [[p, readFileSync(p)]];
  });
}

/** The needles found in `files` (`[path, bytes]`): `{ file, needle }` each. */
export function grep(files, needles) {
  const hits = [];
  for (const [file, bytes] of files) {
    for (const needle of needles) if (needle && bytes.includes(Buffer.from(needle, "utf8"))) hits.push({ file, needle });
  }
  return hits;
}

/** Every entry of a journal store's `j-*.jsonl` files under `<serverData>/<sub>` (`events` or `agent-traces`). */
export function entriesIn(serverData, sub) {
  const dir = join(serverData, sub);
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((n) => n.startsWith("j-") && n.endsWith(".jsonl"))
    .sort()
    .flatMap((n) =>
      readFileSync(join(dir, n), "utf8")
        .split("\n")
        .filter(Boolean)
        .map((l) => JSON.parse(l)),
    );
}

/** A content field's value from the trace store: inline, or its blob read back. */
export function contentOf(serverData, body) {
  if (body === undefined) return undefined;
  if ("inline" in body) return body.inline;
  const hash = body.blob.hash;
  return JSON.parse(readFileSync(join(serverData, "agent-traces", "blobs", hash.includes(":") ? hash.slice(hash.indexOf(":") + 1) : hash), "utf8"));
}

export { log };
