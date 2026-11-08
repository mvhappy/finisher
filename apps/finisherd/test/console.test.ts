// The `finisherd` console (plan 5A Task 8): dispatch, the console's own handshake, the TTY gate,
// typed confirmations, read-only and service commands, the graceful stop, and that nothing
// secret is printed. Commands run in-process through an injected `Prompt`, and spawned with
// piped stdio for the cases a terminal decides.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { answerChallenge, issueChallenge } from "../src/auth/challenges.ts";
import { authDir } from "../src/auth/keys.ts";
import { connectConsole } from "../src/console/client.ts";
import { CONFIRM_TRIES } from "../src/console/confirm.ts";
import { CONFLICT_CODES, DENIED_CODES, JOB_RESULT_CODES, UNAVAILABLE_CODES, USAGE_CODES, VALIDATION_CODES } from "../src/console/exit-codes.ts";
import { isServeCommand, runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import type { Connection } from "../src/connections.ts";
import { connectProved, type Exit, ok, type RunningServer, startServer } from "./support/ndjson-client.ts";

const MAIN = fileURLToPath(new URL("../src/main.ts", import.meta.url));
const REPO = fileURLToPath(new URL("../../../", import.meta.url));

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-console-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

function env(): NodeJS.ProcessEnv {
  const { DO_NOT_TRACK: _d, FIN_DIAGNOSTICS: _f, FIN_TRAINING: _t, FIN_SERVER_DATA: _s, ...rest } = process.env;
  return rest;
}

async function start(): Promise<RunningServer> {
  server = await startServer(serverData, 15_000, env());
  return server;
}

/** A person at a terminal: types what `answer` says for each code the console shows. */
function person(answer: (code: string, attempt: number) => string = (code) => code): Prompt & { shown: string[]; asked: number } {
  const shown: string[] = [];
  let last = "";
  const p = {
    shown,
    asked: 0,
    isInteractive: () => true,
    show: (text: string) => {
      shown.push(text);
      const m = /type this code within 60 seconds: ([0-9A-Z]{6})$/.exec(text);
      if (m) last = m[1]!;
    },
    ask: async () => answer(last, ++p.asked),
  };
  return p;
}

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

/** Runs a console command in-process; resolves with its exit code and what it printed. */
async function run(argv: string[], prompt: Prompt = person()): Promise<{ code: number; out: string; err: string }> {
  let out = "";
  let err = "";
  const code = await runConsole([...argv, "--server-data", serverData], {
    prompt,
    output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
  });
  return { code, out, err };
}

/** Spawns `finisherd <argv>` with piped stdio (not a terminal). */
function spawned(argv: string[]): Promise<{ code: number | null; out: string; err: string }> {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [MAIN, ...argv, "--server-data", serverData], { stdio: ["pipe", "pipe", "pipe"], env: env() });
    let out = "";
    let err = "";
    child.stdout.setEncoding("utf8").on("data", (d: string) => (out += d));
    child.stderr.setEncoding("utf8").on("data", (d: string) => (err += d));
    // What `yes |` would send.
    child.stdin.end("y\ny\ny\n");
    child.once("exit", (code) => resolve({ code, out, err }));
  });
}

const settings = () => (existsSync(join(serverData, "settings.json")) ? readFileSync(join(serverData, "settings.json"), "utf8") : undefined);
const codeOf = (e: Exit) => (e.ok ? undefined : e.error.code);

describe("dispatch and connection", () => {
  test("serve_is_default_and_flags_still_serve", async () => {
    expect([[], ["serve"], ["--server-data", "x"], ["serve", "--server-data", "x"]].map(isServeCommand)).toEqual([true, true, true, true]);
    expect([["status"], ["stop"], ["audit"], ["diagnostics"], ["project"], ["agent"], ["--help"]].map(isServeCommand)).toEqual(Array(7).fill(false));
    // `finisherd serve --server-data <dir>` serves; `startServer` uses the flags-only form.
    const child = spawn(process.execPath, [MAIN, "serve", "--server-data", serverData], { stdio: "ignore", env: env() });
    const exited = new Promise<number | null>((r) => child.once("exit", r));
    for (let i = 0; i < 300 && !existsSync(join(serverData, "finisherd.json")); i++) await new Promise((r) => setTimeout(r, 25));
    expect(existsSync(join(serverData, "finisherd.json"))).toBe(true);
    expect((await run(["stop"])).code).toBe(0);
    expect(await exited).toBe(0);
  });

  test("console_proves_as_console", async () => {
    await start();
    const c = await connectConsole(serverData);
    try {
      expect(ok<{ principal: { kind: string; human: boolean } }>(await c.call("auth.whoami", {})).principal).toMatchObject({ kind: "console", human: true });
    } finally {
      await c.close();
    }
  });

  test("console_without_server_is_server_unavailable", async () => {
    const r = await spawned(["status"]);
    expect(r.code).toBe(5);
    expect(r.err).toMatch(/^finisherd: SERVER_UNAVAILABLE: /);
    expect(r.out).toBe("");
  });

  test("unknown commands are usage errors; agent and mcp check their arguments first", async () => {
    for (const [argv, said] of [
      [["nonsense"], "unknown command"],
      [["agent", "mint"], "agent mint needs --label"],
      [["mcp", "setup"], "mcp setup needs --out"],
      [["agent", "mint", "--label", "x", "--scope", "nope"], "unknown scope: nope"],
      [["agent", "revoke"], "agent revoke needs one credential id"],
    ] as const) {
      const r = await run([...argv]);
      expect(r.code, argv.join(" ")).toBe(2);
      expect(r.err).toContain(`finisherd: USAGE: ${said}`);
    }
    expect((await run(["help"])).out).toContain("needs an interactive terminal");
  });

  test("exit codes mirror fin's (crates/fin/src/output.rs)", () => {
    const rs = readFileSync(join(REPO, "crates/fin/src/output.rs"), "utf8");
    const list = (name: string) => {
      const body = new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`).exec(rs)![1]!.replace(/\/\/.*$/gm, "");
      return [...body.matchAll(/"([A-Z0-9_]+)"/g)].map((m) => m[1]);
    };
    expect(DENIED_CODES).toEqual(list("DENIED_CODES"));
    expect(USAGE_CODES).toEqual(list("USAGE_CODES"));
    expect(CONFLICT_CODES).toEqual(list("CONFLICT_CODES"));
    expect(JOB_RESULT_CODES).toEqual(list("JOB_RESULT_CODES"));
    expect(UNAVAILABLE_CODES).toEqual(list("UNAVAILABLE_CODES"));
    expect(VALIDATION_CODES).toEqual(list("VALIDATION_CODES"));
  });
});

describe("the terminal gate and typed codes", () => {
  const MUTATING = [
    ["audit", "purge", "--all"],
    ["diagnostics", "consent", "diagnostics", "off"],
    ["diagnostics", "off"],
    ["diagnostics", "consent", "training", "off"],
    ["diagnostics", "purge", "--all"],
    ["diagnostics", "project", "default", "01aaaaaaaaaaaaaaaaaaaaaaaa"],
    ["project", "takeover", "01aaaaaaaaaaaaaaaaaaaaaaaa", "--confirm"],
    ["project", "open", "/nowhere/project", "--relocate"],
    ["agent", "mint", "--label", "x"],
    ["agent", "revoke", "01aaaaaaaaaaaaaaaaaaaaaaaa"],
    ["mcp", "setup", "--out", "/nowhere/mcp.json"],
  ];

  test("non_tty_is_console_needs_tty", { timeout: 60_000 }, async () => {
    await start();
    const before = settings();
    for (const argv of MUTATING) {
      const r = await spawned(argv);
      expect(r.code, argv.join(" ")).toBe(7);
      expect(r.err, argv.join(" ")).toMatch(/^finisherd: CONSOLE_NEEDS_TTY: /);
      expect(r.err).not.toMatch(/`|\bfin(isherd)? [a-z]/);
    }
    // In-process, a prompt that is not a terminal is refused the same way.
    expect((await run(["diagnostics", "off"], NOT_A_TERMINAL)).code).toBe(7);
    // Nothing changed: no consent written, nothing purged or confirmed.
    expect(settings()).toBe(before);
    const c = await connectProved(server!, "console");
    const kinds = ok<{ entries: Array<{ kind: string }> }>(await c.call("auth.audit.list", {})).entries.map((e) => e.kind);
    await c.close();
    expect(kinds).not.toContain("consent.change");
    expect(kinds).not.toContain("audit.purge");
    expect(kinds).not.toContain("console.refused");
  });

  test("typed_code_must_match", async () => {
    await start();
    const before = settings();
    const wrong = person(() => "ZZZZZZ");
    const r = await run(["diagnostics", "off"], wrong);
    expect(r.code).toBe(7);
    expect(r.err).toContain("CONFIRMATION_REQUIRED");
    // A new code after each wrong one, then it gives up; nothing changed.
    expect(wrong.asked).toBe(CONFIRM_TRIES);
    expect(wrong.shown.filter((t) => t.startsWith("To go ahead")).length).toBe(CONFIRM_TRIES);
    expect(settings()).toBe(before);
    // Nothing typed: cancelled, nothing changed.
    expect((await run(["diagnostics", "off"], person(() => ""))).code).toBe(7);
    expect(settings()).toBe(before);
    // Wrong once, then right: it goes ahead.
    const second = person((code, attempt) => (attempt === 1 ? "ZZZZZZ" : code.toLowerCase()));
    const ok2 = await run(["diagnostics", "off"], second);
    expect(ok2.code).toBe(0);
    expect(second.shown.some((t) => t.includes("did not match"))).toBe(true);
    expect(JSON.parse(settings()!).privacy.diagnostics).toBe("off");
  });

  test("mutation_without_challenge_is_refused", async () => {
    await start();
    const c = await connectConsole(serverData);
    try {
      for (const [tag, payload] of [
        ["auth.audit.purge", { all: true }],
        ["diagnostics.consent", { purpose: "diagnostics", value: "off" }],
        ["diagnostics.purge", { selector: "all" }],
      ] as const) {
        const e = await c.call(tag, payload);
        expect(codeOf(e), tag).toBe("CONFIRMATION_REQUIRED");
        expect(e.ok ? undefined : e.error.details).toMatchObject({ reason: "missing" });
      }
      // Read-only and service calls need none.
      expect(codeOf(await c.call("auth.audit.list", {}))).toBeUndefined();
      expect(codeOf(await c.call("server.status", {}))).toBeUndefined();
    } finally {
      await c.close();
    }
  });

  test("challenge_is_single_use_and_per_connection", async () => {
    await start();
    const [a, b] = [await connectConsole(serverData), await connectConsole(serverData)];
    try {
      const { code } = ok<{ code: string }>(await a.call("console.challenge", {}));
      // Another connection's code is not this one's.
      expect(codeOf(await b.call("auth.audit.purge", { all: true, challenge: code }))).toBe("CONFIRMATION_REQUIRED");
      expect(codeOf(await a.call("auth.audit.purge", { all: true, challenge: code }))).toBeUndefined();
      // Used up.
      const again = await a.call("auth.audit.purge", { all: true, challenge: code });
      expect(again.ok ? undefined : again.error.details).toMatchObject({ reason: "wrong" });
    } finally {
      await Promise.all([a.close(), b.close()]);
    }
  });

  test("challenge_expires_after_60s", () => {
    const c = {} as Connection;
    const t0 = 1_000_000;
    const { code, expiresAt } = issueChallenge(c, t0);
    expect(expiresAt).toBe(t0 + 60_000);
    expect(answerChallenge(c, code, t0 + 60_000)).toBe("expired");
    const fresh = issueChallenge(c, t0).code;
    expect(answerChallenge(c, fresh, t0 + 59_999)).toBeUndefined();
  });

  test("console_training_on_is_consent_ui_only", async () => {
    await start();
    const before = settings();
    const r = await spawned(["diagnostics", "consent", "training", "on"]);
    expect(r.code).toBe(2);
    expect(r.err).toMatch(/^finisherd: CONSENT_UI_ONLY: /);
    expect(r.err).not.toMatch(/`|\bfin(isherd)? [a-z]/);
    // A person at a terminal is refused too, and never asked for a code.
    const p = person();
    expect((await run(["diagnostics", "consent", "training", "on"], p)).code).toBe(2);
    expect(p.asked).toBe(0);
    expect(settings()).toBe(before);
  });
});

describe("read-only, service and confirmed commands", () => {
  test("read_only_commands_run_without_tty", async () => {
    await start();
    const status = await spawned(["status", "--json"]);
    expect(status.code).toBe(0);
    expect(JSON.parse(status.out)).toMatchObject({ protocol: 2, envId: server!.info.envId, pid: server!.info.pid, journal: { enabled: expect.any(Boolean) } });
    expect((await spawned(["status"])).out).toContain("Finisher server");
    const audit = await spawned(["audit", "--json", "--limit", "5"]);
    expect(audit.code).toBe(0);
    expect(Array.isArray(JSON.parse(audit.out).entries)).toBe(true);
    expect((await spawned(["audit", "--kind", "nonsense"])).code).toBe(2);
  });

  test("stop_shuts_down_gracefully", async () => {
    const s = await start();
    const exited = new Promise<number | null>((r) => (s.child.exitCode !== null ? r(s.child.exitCode) : s.child.once("exit", r)));
    const r = await spawned(["stop"]);
    expect(r.code).toBe(0);
    expect(r.out).toContain("stopped");
    expect(await exited).toBe(0);
    expect(existsSync(join(serverData, "finisherd.json"))).toBe(false);
    // `fin` is not the owner: `server.stop` is human-only for it.
    const again = await start();
    const local = await connectProved(again, "local");
    expect(codeOf(await local.call("server.stop", {}))).toBe("HUMAN_ONLY");
    await local.close();
    expect(again.child.exitCode).toBeNull();
  });

  test("audit_purge_with_a_typed_code_is_audited", async () => {
    await start();
    const r = await run(["audit", "purge", "--all"]);
    expect(r.code).toBe(0);
    expect(r.out).toMatch(/^Removed \d+ audit entries\./);
    const list = await run(["audit", "--json"]);
    const entries = JSON.parse(list.out).entries as Array<{ kind: string; principalKind: string; details: Record<string, unknown> }>;
    expect(entries[0]).toMatchObject({ kind: "audit.purge", principalKind: "console", details: { reason: "all" } });
  });

  test("takeover_confirm_and_relocate_via_console", { timeout: 30_000 }, async () => {
    await start();
    const dir = join(tmp, "project");
    const local = await connectProved(server!, "local");
    const { projectId } = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" }));
    // `fin` may not do either.
    expect(codeOf(await local.call("project.open", { dir, relocate: true }))).toBe("HUMAN_ONLY");
    expect(codeOf(await local.call("project.takeover", { projectId, confirm: true }))).toBe("HUMAN_ONLY");
    await local.close();
    const opened = await run(["project", "open", dir, "--relocate"]);
    expect(opened.code, opened.err).toBe(0);
    expect(opened.out).toContain(projectId);
    // By id and by folder; the server decides what a takeover of its own project means.
    for (const target of [projectId, dir]) {
      const p = person();
      const r = await run(["project", "takeover", target, "--confirm"], p);
      expect(r.err, target).not.toMatch(/HUMAN_ONLY|CONFIRMATION_REQUIRED|CONSOLE_NEEDS_TTY/);
      expect(p.asked).toBe(1);
      if (r.code === 0) expect(JSON.parse(r.out)).toBeTypeOf("object");
      else expect(r.err).toMatch(/LEASE_TAKEOVER_REFUSED/);
    }
    // Without the flag it is a usage error, before anything is asked.
    expect((await run(["project", "takeover", projectId])).code).toBe(2);
  });

  test("agent and mcp credentials through the console (Task 9)", { timeout: 30_000 }, async () => {
    await start();
    const out = join(tmp, "agent.json");
    const minted = await run(["agent", "mint", "--label", "night run", "--scope", "server.read", "--scope", "project.read", "--branch", "own", "--idle", "2h", "--out", out]);
    expect(minted.code, minted.err).toBe(0);
    const file = JSON.parse(readFileSync(out, "utf8")) as { v: number; kind: string; id: string; key: string; env: string };
    expect(file).toMatchObject({ v: 1, kind: "agent", env: server!.info.envId });
    if (process.platform !== "win32") expect(statSync(out).mode & 0o777).toBe(0o600);
    expect(minted.out).toContain(file.id);
    expect(minted.out).not.toContain(file.key);
    // An existing file is refused before anything is minted; a wrong code mints nothing and leaves no file.
    expect((await run(["agent", "mint", "--label", "again", "--out", out])).code).toBe(2);
    const other = join(tmp, "other.json");
    expect((await run(["agent", "mint", "--label", "wrong", "--out", other], person(() => "ZZZZZZ"))).code).toBe(7);
    expect(existsSync(other)).toBe(false);
    // Listing needs no terminal.
    const listed = await run(["agent", "ls", "--json"], NOT_A_TERMINAL);
    expect(listed.code, listed.err).toBe(0);
    const creds = (JSON.parse(listed.out) as { credentials: Array<Record<string, unknown>> }).credentials;
    expect(creds).toHaveLength(1);
    expect(creds[0]).toMatchObject({ id: file.id, label: "night run", scopes: ["server.read", "project.read"], branches: ["own"], idleExpiryS: 7200, status: "live" });
    expect(listed.out).not.toContain(file.key);
    expect((await run(["agent", "ls"], NOT_A_TERMINAL)).out).toContain(file.id);
    // `mcp setup` writes an mcp credential; without --out `agent mint` shows the file once.
    const mcpOut = join(tmp, "mcp.json");
    expect((await run(["mcp", "setup", "--out", mcpOut])).code).toBe(0);
    expect(JSON.parse(readFileSync(mcpOut, "utf8"))).toMatchObject({ v: 1, kind: "mcp" });
    const shown = await run(["agent", "mint", "--label", "shown"]);
    expect(shown.code, shown.err).toBe(0);
    expect(shown.out).toMatch(/"v":1,"kind":"agent"/);
    // Revoking asks for a code; the credential is gone from the live list, kept in --all.
    const p = person();
    const revoked = await run(["agent", "revoke", file.id], p);
    expect(revoked.code, revoked.err).toBe(0);
    expect(revoked.out).toContain(`Revoked credential ${file.id}`);
    expect(p.asked).toBe(1);
    expect((await run(["agent", "ls"], NOT_A_TERMINAL)).out).not.toContain(file.id);
    expect((await run(["agent", "ls", "--all"], NOT_A_TERMINAL)).out).toMatch(new RegExp(`${file.id}  agent  revoked`));
    expect((await run(["agent", "revoke", file.id])).out).toContain("nothing changed");
  });

  test("console_never_prints_secrets", async () => {
    await start();
    const keys = ["local.key", "console.key", "credential.master"]
      .map((f) => join(authDir(serverData), f))
      .filter((f) => existsSync(f))
      .map((f) => readFileSync(f, "utf8").trim());
    expect(keys.length).toBeGreaterThanOrEqual(2);
    const p = person();
    const printed = [
      await run(["status"]),
      await run(["status", "--json"]),
      await run(["diagnostics", "off"], p),
      await run(["audit", "--json"]),
      await run(["audit"]),
      await run(["diagnostics", "consent", "training", "on"]),
    ]
      .map((r) => r.out + r.err)
      .join("\n");
    for (const k of keys) expect(printed).not.toContain(k);
    // The typed code is shown to the person on the prompt only, never in the output or an error.
    const code = /: ([0-9A-Z]{6})$/.exec(p.shown.find((t) => t.startsWith("To go ahead"))!)![1]!;
    expect(printed).not.toContain(code);
    expect(printed).not.toMatch(/serverNonce|clientNonce|"mac"|"proof"|finisher\.ticket\./);
  });
});
