// Helpers of the "claude" e2e scenario (7D Task 16): the fake CLI's install, what it observed, a
// person answering approval cards from the console, and a listing that proves nothing was written.
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { TARGET_DIR, consoleRpc, log } from "./lib.mjs";

export const FAKE_CLAUDE = join(TARGET_DIR, "release", "fake-claude");

/**
 * Installs the fake CLI as `<dir>/claude`: a wrapper that logs each invocation (the host's own
 * detection and probe runs included) and runs `<dir>/fake-claude` with its sidecar: the script
 * file (rewritten between processes by `setScript`) and the observation file, which each session
 * keeps in its own config dir (`agents/<id>/provider/observed.json`: the only place a sandboxed
 * CLI may write), one record per CLI process of the session.
 */
export function installFake(dir, { script }) {
  mkdirSync(dir, { recursive: true });
  copyFileSync(FAKE_CLAUDE, join(dir, "fake-claude"));
  chmodSync(join(dir, "fake-claude"), 0o755);
  const log = join(dir, "invocations.log");
  const binary = join(dir, "claude");
  writeFileSync(binary, `#!/bin/sh\nprintf '%s\\n' "$*" >> '${log}' 2>/dev/null\nexec '${join(dir, "fake-claude")}' "$@"\n`, { mode: 0o755 });
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json" }));
  const setScript = (s) => writeFileSync(join(dir, "script.json"), JSON.stringify(s));
  setScript(script);
  return {
    binary,
    setScript,
    /** How many times the CLI was started by anyone so far. */
    invocations: () => (existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean).length : 0),
    /** One record per CLI process of the session, in start order (`[]` before the first). */
    records: (serverData, sessionId) => {
      const file = join(serverData, "agents", sessionId, "provider", "observed.json");
      return existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : [];
    },
  };
}

/** The user lines the fake received, as texts, over all its processes. */
export const userTexts = (records) =>
  records.flatMap((r) => (r.userLines ?? []).map((l) => (typeof l.content === "string" ? l.content : (l.content ?? []).map((p) => p?.text ?? "").join("\n"))));

/** Every file under `dir` as `path size` lines, sorted (`[]` when the folder is missing). */
export function listing(dir) {
  if (!existsSync(dir)) return [];
  const walk = (d) =>
    readdirSync(d).flatMap((name) => {
      const p = join(d, name);
      return statSync(p).isDirectory() ? [`${p}/`, ...walk(p)] : [`${p} ${statSync(p).size}`];
    });
  return walk(dir).sort();
}

/** The session's approval cards with this status (`undefined`: all), oldest first. */
export async function cardsOf(server, sessionId, status) {
  const r = await consoleRpc(server, "agent.approval.list", status === undefined ? {} : { status }, { confirm: false });
  return r.approvals.filter((a) => a.sessionId === sessionId);
}

/** The text a card's subject names (`tool:Bash(...)` for a provider's permission prompt), whatever its kind. */
export const subjectText = (card) => JSON.stringify(card.subject ?? {});

/**
 * A person at the console: answers every open card of `sessionId` that `decide` takes with allow
 * (a card it returns false for stays open), as soon as it opens. `answered` lists the cards the
 * person answered, in order; `stop()` ends the loop.
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
          if (seen.has(card.id) || !decide(card)) continue;
          seen.add(card.id);
          await consoleRpc(server, "agent.approval.answer", { id: card.id, decision: "allow" });
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
      if (failure !== undefined) throw failure;
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
