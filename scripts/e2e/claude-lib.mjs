// Helpers of the "claude" e2e scenario (7D Task 16): the fake CLI's install, what it observed, a
// person answering approval cards from the console, a listing that proves nothing was written, the
// console answers the leak grep reads, and the reaping of a CLI a killed server left behind.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { TARGET_DIR, consoleRpc, log } from "./lib.mjs";

export const FAKE_CLAUDE = join(TARGET_DIR, "release", "fake-claude");
export const VERSION = "2.1.321";

/** Every console answer of the run (and every refusal), named by its tag: the leak grep reads them. */
export const rpcLog = [];

/** `consoleRpc`, its answer (or refusal) kept in `rpcLog`. */
export async function rpc(server, tag, payload, opts) {
  try {
    const r = await consoleRpc(server, tag, payload, opts);
    rpcLog.push({ name: `rpc ${tag}`, text: JSON.stringify(r) });
    return r;
  } catch (e) {
    rpcLog.push({ name: `rpc ${tag} (refused)`, text: String(e?.message ?? e) });
    throw e;
  }
}

/**
 * Installs the fake CLI as Claude Code's native build lays itself out: the fake binary itself is
 * `<dir>/claude/versions/<version>`, and its sidecar (`fake-claude.json`) and the script file
 * (rewritten between processes by `setScript`) sit beside it, inside the one directory the
 * launcher grants the CLI's install (`installTreeRead`, 7D Task 14). Each session keeps its records
 * in its own config dir (`agents/<id>/provider/observed.json`), one per CLI process. Every process
 * (the host's detection runs too) appends `{pid, argv}` to the invocation log: directly, a file
 * beside the install; under the launcher, where a host run may write only its sign-in store,
 * `<sign-in home>/store/fake-invocations.jsonl` (a session's CLI may write the store too).
 */
export function installFake(dir, { script, launcher, serverData }) {
  const versions = join(dir, "claude", "versions");
  mkdirSync(versions, { recursive: true });
  const binary = join(versions, VERSION);
  copyFileSync(FAKE_CLAUDE, binary);
  const invocationLog = launcher ? join(serverData, "providers", "claudeCode", "home", "store", "fake-invocations.jsonl") : join(dir, "invocations.jsonl");
  const spec = launcher ? "@store/fake-invocations.jsonl" : invocationLog;
  writeFileSync(join(versions, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION, invocations: [spec] }));
  const setScript = (s) => writeFileSync(join(versions, "script.json"), JSON.stringify(s));
  setScript(script);
  return {
    binary,
    setScript,
    invocationLog,
    /** Every CLI process started so far, by anyone: `{pid, argv}` in start order. */
    invocations: () =>
      existsSync(invocationLog)
        ? readFileSync(invocationLog, "utf8")
            .split("\n")
            .filter(Boolean)
            .map((l) => JSON.parse(l))
        : [],
    /** One record per CLI process of the session, in start order (`[]` before the first). */
    records: (serverData, sessionId) => {
      const file = join(serverData, "agents", sessionId, "provider", "observed.json");
      return existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : [];
    },
  };
}

/**
 * Kills what is left of the fake CLI processes `pids` (a server killed mid-call leaves its CLI and
 * the call's shell running): each one's process group, then the process. Only a process that still
 * runs `binary` is touched (not a pid recycled since, nor one a PID namespace numbered).
 */
export function reapFakes(pids, binary) {
  for (const pid of new Set(pids)) {
    if (!Number.isInteger(pid) || pid <= 1) continue;
    let out;
    try {
      out = execFileSync("ps", ["-o", "pgid=,command=", "-p", String(pid)], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
      continue; // gone
    }
    const m = /^(\d+)\s+(.*)$/.exec(out);
    if (m === null || !m[2].includes(binary)) continue;
    for (const target of [-Number(m[1]), pid]) {
      try {
        process.kill(target, "SIGKILL");
      } catch {
        // already gone
      }
    }
    log(`reaped the fake CLI ${pid} (group ${m[1]})`);
  }
}

/** The user lines the fake received, as texts, over all its processes. */
export const userTexts = (records) =>
  records.flatMap((r) => (r.userLines ?? []).map((l) => (typeof l.content === "string" ? l.content : (l.content ?? []).map((p) => p?.text ?? "").join("\n"))));

/** Every file under `dir` as `path sha256` lines (folders as `path/`), sorted (`[]` when the folder is missing). */
export function listing(dir) {
  if (!existsSync(dir)) return [];
  const walk = (d) =>
    readdirSync(d).flatMap((name) => {
      const p = join(d, name);
      const st = lstatSync(p);
      if (st.isDirectory()) return [`${p}/`, ...walk(p)];
      return [st.isFile() ? `${p} ${createHash("sha256").update(readFileSync(p)).digest("hex")}` : `${p} (not a file)`];
    });
  return walk(dir).sort();
}

/** The session's approval cards with this status (`undefined`: all), oldest first. */
export async function cardsOf(server, sessionId, status) {
  const r = await rpc(server, "agent.approval.list", status === undefined ? {} : { status }, { confirm: false });
  return r.approvals.filter((a) => a.sessionId === sessionId);
}

/** The text a card's subject names (`tool:Bash(...)` for a provider's permission prompt), whatever its kind. */
export const subjectText = (card) => JSON.stringify(card.subject ?? {});

/**
 * A person at the console: answers every open card of `sessionId` that `decide` takes with allow
 * (a card it returns false for stays open), as soon as it opens. `answered` lists the cards the
 * person answered, in order; `stop()` ends the loop (again: a no-op) and throws what failed it.
 */
export function attend(server, sessionId, decide = () => true) {
  const answered = [];
  const seen = new Set();
  let running = true;
  let failure;
  const loop = (async () => {
    while (running) {
      try {
        for (const card of await cardsOf(server, sessionId, "open")) {
          if (!running) return;
          if (seen.has(card.id) || !decide(card)) continue;
          seen.add(card.id);
          await rpc(server, "agent.approval.answer", { id: card.id, decision: "allow" });
          answered.push(card);
          log(`the person allowed card ${card.kind} ${subjectText(card).slice(0, 100)}`);
        }
      } catch (e) {
        failure = e;
        return;
      }
      await new Promise((r) => setTimeout(r, 40));
    }
  })();
  return {
    answered,
    stop: async () => {
      running = false;
      await loop;
      const f = failure;
      failure = undefined;
      if (f !== undefined) throw f;
    },
  };
}

/** Polls the async `f` until it answers something, up to `ms`. */
export async function until(what, f, ms) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 40));
  }
}

/**
 * What a person's sign-in leaves in Claude Code's sign-in home (`<server-data>/providers/claudeCode/home/`):
 * the store folder and `.claude.json` with the account's profile, which `auth status` names the
 * account from. Written by the scenario, never by finisherd (a repeat is the account switching).
 */
export function signIn(serverData, email) {
  const home = join(serverData, "providers", "claudeCode", "home");
  mkdirSync(join(home, "store"), { recursive: true, mode: 0o700 });
  writeFileSync(join(home, ".claude.json"), JSON.stringify({ oauthAccount: { emailAddress: email, organizationUuid: `org-of-${email}` } }), { mode: 0o600 });
}

/**
 * Every regular file under `roots` but the `skip`ped ones (a path, or a folder ending in `/`), as
 * leak-grep sources `{name, text}` (bytes read as latin1).
 */
export function filesForGrep(roots, skip) {
  const skipped = (p) => skip.some((s) => (s.endsWith("/") ? `${p}/`.startsWith(s) : p === s));
  const walk = (d) =>
    readdirSync(d).flatMap((name) => {
      const p = join(d, name);
      if (skipped(p)) return [];
      const st = lstatSync(p);
      if (st.isDirectory()) return walk(p);
      return st.isFile() ? [{ name: p, text: readFileSync(p, "latin1") }] : [];
    });
  return roots.filter((r) => existsSync(r)).flatMap(walk);
}
