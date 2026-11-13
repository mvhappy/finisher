// One command hook (7F plan Task 3; Decisions 2, 6, 9, 10, 12; spec §8.10, §9.2): the reply
// read from stdout (one line, or nothing: `allow`), every other outcome a failure, the
// cleared environment, one request line on stdin, a private temp as cwd, the deadline enforced
// by `finisherd` (slot wait included) and the whole tree killed at it, the program resolved
// (`node`, the toolchain base, never the user's `PATH`), argument boundaries kept, and the
// `hook` profile under the real launcher on Linux (`FIN_TEST_SANDBOX_READY=hook`, the
// test-hooks fin-node, as 7C/7D test the `agent` profile): no network, the project read-only,
// no server data, writes only in its temp. Windows (AppContainer) cases are skipped elsewhere;
// the grant and revoke of a launch's table run everywhere through a recording broker.
// `FIN_REQUIRE_LANDLOCK=1` turns a skip of the launcher cases into a failure.
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { AclGrant, HookInput, HookRequest } from "@finisher/core-types";
import { sandboxHookCheck, sandboxHookGrants, sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { ProjectTrust } from "../src/agent/rule-store.ts";
import { type ConfiguredHook, HookConfig, programOf } from "../src/hooks/config.ts";
import { type HookProcessDeps, type HookRun, HookSlots, isCommandHook, type ResolvedHook, runCommandHook } from "../src/hooks/process.ts";
import type { AclBroker } from "../src/sandbox/broker.ts";
import { probeLauncher, type SandboxStatus } from "../src/sandbox/status.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const WIN = process.platform === "win32";
const LINUX = process.platform === "linux";
const require = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const saved = { ready: process.env.FIN_TEST_SANDBOX_READY, secret: process.env.SECRET_X, cred: process.env.FIN_CREDENTIAL };

beforeAll(() => {
  // The `hook` profile is not in `READY` until 7C Task 16: the test-hooks build adds it.
  process.env.FIN_TEST_SANDBOX_READY = "hook";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_SANDBOX_READY", saved.ready], ["SECRET_X", saved.secret], ["FIN_CREDENTIAL", saved.cred]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-process-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

/** The status with the `hook` profile replaced. */
const withHook = (hook: SandboxStatus["hook"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), hook });
const DIRECT = withHook({ kind: "disabled", reason: "profileNotReady" });

/** Whether the real `hook` profile is enforced here; else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
function enforced(name: string): boolean {
  const m = sandboxStatus().hook;
  if (m.kind === "enforced") return true;
  expect(require, `${name}: FIN_REQUIRE_LANDLOCK=1 but the hook profile is ${JSON.stringify(m)}`).toBe(false);
  return false;
}

interface Fixture {
  readonly serverData: string;
  readonly dir: string;
  readonly script: string;
  readonly config: HookConfig;
  readonly trusts: Map<string, ProjectTrust>;
}

/** Server data and project `p1` in `tmp`, the fake hook copied into both (the user layer declares hooks in server data). */
function fixture(): Fixture {
  const serverData = join(tmp, "server-data");
  const dir = join(tmp, "P");
  mkdirSync(serverData, { recursive: true });
  mkdirSync(dir, { recursive: true });
  const script = join(dir, "fake-hook.mjs");
  copyFileSync(FAKE, script);
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  const trusts = new Map<string, ProjectTrust>();
  const config = new HookConfig({ serverData, trustOf: (id) => trusts.get(id), dirOf: (id) => (id === "p1" ? dir : undefined) });
  return { serverData, dir, script, config, trusts };
}

/** The plan's configuration of the fake hook: `run: ["node", "fake-hook.mjs", <args…>], file: "fake-hook.mjs"`. */
const fakeSpec = (id: string, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event: "PreCommand", run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });

/** A user-layer hook running the fake hook (in server data) with `args`. */
function userHook(f: Fixture, args: readonly string[], more: Record<string, unknown> = {}, id = "h"): ResolvedHook {
  writeFileSync(join(f.serverData, "settings.json"), JSON.stringify({ hooks: [fakeSpec(id, args, more)] }));
  return cmd(f.config.hooks("p1").find((x) => x.id === id));
}

/** Trusts the project's hooks file as it is now. */
function trustNow(f: Fixture): void {
  const hooksHash = f.config.list("p1").hooksHash;
  if (hooksHash === undefined) throw new Error(`not trustable: ${JSON.stringify(f.config.list("p1"))}`);
  f.trusts.set("p1", { projectId: "p1", rulesHash: "none", hooksHash, trustedAt: 0, by: "console:owner" });
}

/** A trusted project-layer hook: `specs` written to `finisher.hooks.json`, the one with `id` answered. */
function projectHook(f: Fixture, specs: readonly unknown[], id: string): ResolvedHook {
  writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: specs }));
  trustNow(f);
  return cmd(f.config.hooks("p1").find((x) => x.layer === "project" && x.id === id));
}

/** `h` as a command hook. */
function cmd(h: ConfiguredHook | undefined): ResolvedHook {
  if (h === undefined || !isCommandHook(h)) throw new Error(`not a command hook: ${JSON.stringify(h)}`);
  return h;
}

/** A file in the project the fake hook writes back as is (`--raw-file`: a `run` string is at most 4 KiB). */
function rawFile(f: Fixture, text: string | Buffer): string {
  const path = join(f.dir, `raw-${Math.random().toString(36).slice(2)}.out`);
  writeFileSync(path, text);
  return path;
}

const request = (f: Fixture, payload: unknown = { rpc: "item.trim" }): HookRequest => ({ event: "PreCommand", v: 1, project: { id: "p1", dir: f.dir }, payload });

function deps(f: Fixture, over: Partial<HookProcessDeps> = {}): HookProcessDeps {
  return { serverData: f.serverData, slots: new HookSlots(8), status: DIRECT, ...over };
}

const run = (h: ResolvedHook, req: HookRequest, d: HookProcessDeps) => Effect.runPromise(runCommandHook(h, req, d));
/** What the fake hook observed (its context reply's text). */
function seen(r: HookRun): Record<string, any> {
  if (!("reply" in r) || r.reply !== "context") throw new Error(`not an observation: ${JSON.stringify(r)}`);
  return JSON.parse(r.text);
}

/** Whether a process whose argv holds `marker` is alive (Linux: each process's `cmdline` under `/proc`). */
function markerAlive(marker: string): boolean {
  for (const pid of readdirSync("/proc")) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      if (readFileSync(join("/proc", pid, "cmdline"), "utf8").includes(marker)) return true;
    } catch {
      // Gone meanwhile.
    }
  }
  return false;
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function until<A>(f: () => A | undefined, ms = 5000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() - t0 > ms) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

describe("the reply (Decision 2)", () => {
  test("allow_on_empty_stdout", async () => {
    const f = fixture();
    expect(await run(userHook(f, []), request(f), deps(f))).toEqual({ reply: "allow" });
    expect(await run(userHook(f, ["--reply", "allow"]), request(f), deps(f))).toEqual({ reply: "allow" });
  });

  test("block_and_context_replies", async () => {
    const f = fixture();
    expect(await run(userHook(f, ["--reply", "block:not on main"]), request(f), deps(f))).toEqual({ reply: "block", reason: "not on main" });
    expect(await run(userHook(f, ["--reply", "context:lint is clean"]), request(f), deps(f))).toEqual({ reply: "context", text: "lint is clean" });
    // Control characters but `\n` and `\t` are stripped.
    expect(await run(userHook(f, ["--raw", `${JSON.stringify({ reply: "context", text: "a\u0007b\nc\td\u001b" })}\n`]), request(f), deps(f))).toEqual({ reply: "context", text: "ab\nc\td" });
  });

  test("nonzero_exit_is_failure: an exit, a signal", async () => {
    const f = fixture();
    // Even with a valid reply.
    expect(await run(userHook(f, ["--reply", "allow", "--exit", "3"]), request(f), deps(f))).toMatchObject({ kind: "exit" });
    if (!WIN) expect(await run(userHook(f, ["--kill"]), request(f), deps(f))).toMatchObject({ kind: "signal" });
  });

  test("two_json_values_is_failure: and garbage, modify, an unknown key, invalid UTF-8", async () => {
    const f = fixture();
    const allow = JSON.stringify({ reply: "allow" });
    for (const raw of [`${allow}\n${allow}\n`, `${allow} ${allow}`, "not json\n", '{"reply":"modify","payload":{}}\n', '{"reply":"allow","extra":1}\n', '{"reply":"block"}\n', "[]\n"]) {
      expect(await run(userHook(f, ["--raw", raw]), request(f), deps(f)), raw).toMatchObject({ kind: "invalid" });
    }
    // Invalid UTF-8 (a lone continuation byte).
    expect(await run(userHook(f, ["--raw-file", rawFile(f, Buffer.from([0x7b, 0x80, 0x7d, 0x0a]))]), request(f), deps(f))).toMatchObject({ kind: "invalid" });
  });

  test("one_line_strictly: empty is allow, one trailing newline or CRLF, whitespace-only is invalid", async () => {
    const f = fixture();
    const allow = JSON.stringify({ reply: "allow" });
    expect(await run(userHook(f, ["--raw", `${allow}\r\n`]), request(f), deps(f))).toEqual({ reply: "allow" });
    expect(await run(userHook(f, ["--raw", allow]), request(f), deps(f))).toEqual({ reply: "allow" });
    for (const raw of [" \n", "\n", `${allow}\n\n`, `\n${allow}\n`, `${allow}\r\n\r\n`]) {
      expect(await run(userHook(f, ["--raw", raw]), request(f), deps(f)), JSON.stringify(raw)).toMatchObject({ kind: "invalid" });
    }
  });

  test("stderr_is_capped_in_bytes", async () => {
    const f = fixture();
    const r = await run(userHook(f, ["--stderr", "é".repeat(600), "--exit", "2"]), request(f), deps(f));
    expect(r).toMatchObject({ kind: "exit" });
    expect(Buffer.byteLength((r as { stderr: string }).stderr)).toBeLessThanOrEqual(512);
  });

  test("stdout_over_cap_is_failure_and_killed", async () => {
    const f = fixture();
    const t0 = Date.now();
    // 64 KiB + 1: refused and killed at once (the fake hook would wait a minute).
    const r = await run(userHook(f, ["--oversize", String(64 * 1024 + 1)], { timeoutS: 30 }), request(f), deps(f));
    expect(r).toMatchObject({ kind: "tooLarge" });
    expect(Date.now() - t0).toBeLessThan(15_000);
    // Exactly the cap is not over it (it is not a reply either).
    expect(await run(userHook(f, ["--raw-file", rawFile(f, "x".repeat(64 * 1024))]), request(f), deps(f))).toMatchObject({ kind: "invalid" });
  });

  test("reason_over_1kib_is_failure: and text over 16 KiB, counted after stripping", async () => {
    const f = fixture();
    expect(await run(userHook(f, ["--reply", `block:${"r".repeat(1025)}`]), request(f), deps(f))).toMatchObject({ kind: "tooLarge" });
    expect(await run(userHook(f, ["--reply", `block:${"r".repeat(1024)}`]), request(f), deps(f))).toEqual({ reply: "block", reason: "r".repeat(1024) });
    const text = "t".repeat(16 * 1024);
    expect(await run(userHook(f, ["--raw-file", rawFile(f, JSON.stringify({ reply: "context", text: `${text}x` }))]), request(f), deps(f))).toMatchObject({ kind: "tooLarge" });
    expect(await run(userHook(f, ["--raw-file", rawFile(f, JSON.stringify({ reply: "context", text: `${text}\u0001\u0002` }))]), request(f), deps(f))).toEqual({ reply: "context", text });
  });
});

describe("the process (Decision 9)", () => {
  test("hook_env_is_cleared: only Decision 9's names", async () => {
    const f = fixture();
    process.env.SECRET_X = "s3cret";
    process.env.FIN_CREDENTIAL = join(f.serverData, "auth", "local.key");
    try {
      const env = seen(await run(userHook(f, ["--report", "env", "--report", "cwd"]), request(f), deps(f))).env as Record<string, string>;
      const names = Object.keys(env).filter((k) => !(WIN && /^(SystemRoot|SYSTEMROOT)$/.test(k)));
      expect(names.sort()).toEqual(["FIN_HOOK_EVENT", "HOME", "LANG", "PATH", "TEMP", "TMP", "TMPDIR"]);
      expect(env.FIN_HOOK_EVENT).toBe("PreCommand");
      expect(env.LANG).toBe("C.UTF-8");
      expect(new Set([env.TMPDIR, env.TEMP, env.TMP, env.HOME]).size).toBe(1);
      expect(dirname(env.TMPDIR!)).toBe(join(f.serverData, "hooks", "run"));
      expect(JSON.stringify(env)).not.toMatch(/s3cret|local\.key/);
    } finally {
      delete process.env.SECRET_X;
      delete process.env.FIN_CREDENTIAL;
    }
  });

  test("stdin_is_one_request_line_then_eof: a payload over 1 MiB is omitted", async () => {
    const f = fixture();
    const h = userHook(f, ["--report", "stdin"]);
    const req = request(f, { rpc: "item.trim", n: 1 });
    // The fake hook answers only after reading to the end of stdin.
    expect(seen(await run(h, req, deps(f))).stdin).toBe(`${JSON.stringify(req)}\n`);
    const big = "b".repeat(1024 * 1024);
    const line = seen(await run(h, request(f, { big }), deps(f))).stdin as string;
    expect(line.endsWith("\n") && line.indexOf("\n") === line.length - 1).toBe(true);
    expect(JSON.parse(line).payload).toEqual({ payloadOmitted: true, bytes: Buffer.byteLength(JSON.stringify({ big })) });
  });

  test("cwd_is_private_temp_removed_after", async () => {
    const f = fixture();
    const cwd = seen(await run(userHook(f, ["--report", "cwd"]), request(f), deps(f))).cwd as string;
    expect(dirname(cwd)).toBe(join(f.serverData, "hooks", "run"));
    expect(existsSync(cwd)).toBe(false);
    expect(readdirSync(join(f.serverData, "hooks", "run"))).toEqual([]);
  });

  test.skipIf(WIN)("cwd_is_private_temp: owner-only", async () => {
    const f = fixture();
    const h = userHook(f, ["--fork-sleeper", "owner-only", "--sleep", "60000"], { timeoutS: 2 });
    const pending = run(h, request(f), deps(f));
    const runDir = join(f.serverData, "hooks", "run");
    const one = await until(() => (existsSync(runDir) ? readdirSync(runDir)[0] : undefined));
    const { statSync } = await import("node:fs");
    expect(statSync(join(runDir, one)).mode & 0o777).toBe(0o700);
    await pending;
  });

  test("argument_boundaries_survive: direct mode", async () => {
    const f = fixture();
    // Spaces, quotes, backslashes (a trailing one too), an empty string and shell characters,
    // each its own argument (the path of a `--read`), echoed back byte-exact.
    const odd = [`a b  "c" \\d\\ e\\`, "", "\\\\server\\share\\", `'single' & | ; $HOME %PATH% *`];
    const args = ["--report", "args", ...odd.flatMap((o) => ["--read", o])];
    expect(seen(await run(userHook(f, args), request(f), deps(f))).args).toEqual(args);
  });
});

describe("the deadline and the slots (Decision 10)", () => {
  test("timeout_kills_tree: the hook and a child it forked are gone at the deadline", async () => {
    if (WIN) return;
    const f = fixture();
    const h = userHook(f, ["--fork-sleeper", `sleeper-${Math.random()}`, "--sleep", "60000"], { timeoutS: 1 });
    const t0 = Date.now();
    const pending = run(h, request(f), deps(f));
    const runDir = join(f.serverData, "hooks", "run");
    const pid = await until(() => {
      const d = existsSync(runDir) ? readdirSync(runDir)[0] : undefined;
      const p = d === undefined ? undefined : join(runDir, d, "sleeper.pid");
      return p !== undefined && existsSync(p) && readFileSync(p, "utf8") !== "" ? Number(readFileSync(p, "utf8")) : undefined;
    });
    expect(alive(pid)).toBe(true);
    expect(await pending).toMatchObject({ kind: "timeout" });
    expect(Date.now() - t0).toBeGreaterThanOrEqual(900);
    await until(() => (alive(pid) ? undefined : true));
  });

  test("slot_wait_counts_against_timeout: nine hooks of 1 s at a 1.5 s timeout, the ninth times out", async () => {
    const f = fixture();
    const d = deps(f, { slots: new HookSlots(8) });
    const hooks = Array.from({ length: 9 }, (_, i) => userHook(f, ["--sleep", "1000", "--reply", "allow"], { timeoutS: 1 }, `h${i}`));
    // `timeoutS` is whole seconds: 1.5 s through the deps' override.
    const results = await Promise.all(hooks.map((h, i) => (i === 8 ? new Promise<void>((r) => setTimeout(r, 50)).then(() => run(h, request(f), { ...d, timeoutMs: 1500 })) : run(h, request(f), { ...d, timeoutMs: 1500 }))));
    expect(results.slice(0, 8)).toEqual(Array(8).fill({ reply: "allow" }));
    expect(results[8]).toMatchObject({ kind: "timeout" });
  });

  test("hook_slots_bound_processes", async () => {
    const slots = new HookSlots(2);
    const a = await slots.acquire(Date.now() + 1000);
    const b = await slots.acquire(Date.now() + 1000);
    expect([a, b].every((x) => x !== undefined)).toBe(true);
    // A third waits, and gives up at its deadline.
    expect(await slots.acquire(Date.now() + 100)).toBeUndefined();
    const c = slots.acquire(Date.now() + 2000);
    a!();
    expect(await c).toBeTypeOf("function");
    b!();
  });
});

describe("modes (Decision 9)", () => {
  test("unavailable_profile_is_failure", async () => {
    const f = fixture();
    const r = await run(userHook(f, ["--reply", "allow"]), request(f), deps(f, { status: withHook({ kind: "unavailable", need: "Landlock" }) }));
    expect(r).toMatchObject({ kind: "unavailable" });
    expect(existsSync(join(f.serverData, "hooks", "run")) ? readdirSync(join(f.serverData, "hooks", "run")) : []).toEqual([]);
  });

  test("direct_mode_runs_with_warning", async () => {
    const f = fixture();
    const started: unknown[] = [];
    const r = await run(userHook(f, ["--reply", "allow"]), request(f), deps(f, { onStart: (s) => started.push(s) }));
    expect(r).toEqual({ reply: "allow" });
    expect(started).toEqual([{ mode: "direct", warning: expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "hook", reason: "profileNotReady" }) }) }]);
  });

  test("a_launcher_refusal_is_unavailable", async () => {
    if (WIN) return;
    const f = fixture();
    // A launcher that refuses as the real one does: its line on descriptor 3, exit 78.
    const refusing = join(tmp, "refusing-launcher.sh");
    writeFileSync(refusing, `#!/bin/sh\ncat <&3 >/dev/null\necho '{"code":"SANDBOX_UNAVAILABLE"}' >&3\nexit 78\n`, { mode: 0o755 });
    const r = await run(userHook(f, ["--reply", "allow"]), request(f), deps(f, { status: withHook({ kind: "enforced", detail: "stub" }), launcher: () => refusing }));
    expect(r).toMatchObject({ kind: "unavailable" });
  });

  test("sandbox_hook_check_reports_bad_input", () => {
    const good: HookInput = { projectDir: tmp, hookFile: join(tmp, "h.mjs"), toolchains: [], tmp: join(tmp, "t"), serverData: join(tmp, "sd"), home: join(tmp, "home") };
    expect(sandboxHookCheck(good)).toBeNull();
    expect(sandboxHookCheck({ ...good, hookFile: "relative/h.mjs" })).toMatch(/hookFile/);
    // The project inside server data.
    expect(sandboxHookCheck({ ...good, projectDir: join(tmp, "sd", "p") })).toMatch(/projectDir/);
    expect(sandboxHookCheck({ ...good, extra: 1 } as unknown as HookInput)).toMatch(/does not parse/);
  });
});

describe("the program (Decision 6)", () => {
  test("program_resolution: node, a declaring-folder path, an absolute path, a bare name", () => {
    expect(programOf("node", "/d", "linux")).toEqual({ path: process.execPath, node: true });
    expect(programOf("./bin/check", "/d", "linux")).toEqual({ path: join("/d", "bin", "check") });
    expect(programOf("/usr/bin/git", "/d", "linux")).toEqual({ path: "/usr/bin/git" });
    expect(programOf("git", "/d", "linux")).toEqual({ bare: "git" });
    // Windows: an `.exe`, or finisherd's own Node; nothing a shell runs.
    expect(programOf("node", "C:\\d", "win32")).toEqual({ path: process.execPath, node: true });
    expect(programOf("git.exe", "C:\\d", "win32")).toEqual({ bare: "git.exe" });
    for (const p of ["check.cmd", "check.bat", "check.ps1", "git", "C:\\tools\\run.CMD", ".\\hooks\\lint.js"]) {
      expect(() => programOf(p, "C:\\d", "win32"), p).toThrow(/\.exe/);
    }
  });

  test("program_resolution: a bare name only from the toolchain base, never the user's PATH", async () => {
    if (WIN) return;
    const f = fixture();
    const base = join(tmp, "base");
    const user = join(tmp, "user-bin");
    mkdirSync(base);
    mkdirSync(user);
    const tool = (dir: string, name: string) => {
      writeFileSync(join(dir, name), `#!/bin/sh\ncat >/dev/null\necho '{"reply":"context","text":"${name} from ${dir === base ? "base" : "user"}"}'\n`);
      chmodSync(join(dir, name), 0o755);
    };
    tool(base, "basetool");
    tool(user, "usertool");
    const savedPath = process.env.PATH;
    process.env.PATH = `${user}:${savedPath}`;
    try {
      const d = deps(f, { toolchainBase: () => [base, "/usr/bin", "/bin"] });
      writeFileSync(join(f.serverData, "settings.json"), JSON.stringify({ hooks: [{ id: "b", event: "PreCommand", run: ["basetool"] }, { id: "u", event: "PreCommand", run: ["usertool"] }] }));
      const [b, u] = f.config.hooks("p1").map(cmd);
      expect(b!.program).toEqual({ bare: "basetool" });
      expect(await run(b!, request(f), d)).toEqual({ reply: "context", text: "basetool from base" });
      expect(await run(u!, request(f), d)).toMatchObject({ kind: "unavailable", detail: expect.stringMatching(/toolchain/) });
    } finally {
      process.env.PATH = savedPath;
    }
  });

  test("program_resolution: node is finisherd's own and a declaring-folder path resolves in that folder", async () => {
    const f = fixture();
    const h = userHook(f, ["--reply", "allow"]);
    expect(h.program).toEqual({ path: process.execPath, node: true });
    expect(h.run[0]).toBe("node");
    // A project hook's `run[0]` with a separator is the very path its hash binds.
    writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [{ id: "p", event: "PreCommand", run: ["./hooks/../fake-hook.mjs"] }] }));
    const listed = f.config.list("p1");
    f.trusts.set("p1", { projectId: "p1", rulesHash: "none", hooksHash: listed.hooksHash!, trustedAt: 0, by: "console:owner" });
    const p = cmd(f.config.hooks("p1").find((x) => x.layer === "project"));
    expect(listed.projectHooksFiles).toEqual(["fake-hook.mjs"]);
    expect(p.program).toEqual({ path: join(f.dir, ...listed.projectHooksFiles![0]!.split("/")) });
    expect(p.file).toBe(join(f.dir, "fake-hook.mjs"));
  });

  test.skipIf(!WIN)("program_resolution: Windows refuses a .cmd at load (the layer is unreadable)", () => {
    const f = fixture();
    writeFileSync(join(f.serverData, "settings.json"), JSON.stringify({ hooks: [{ id: "c", event: "PreCommand", run: ["check.cmd"] }] }));
    const hooks = f.config.hooks("p1");
    expect(hooks.every((h) => h.unreadable === true && h.id === "user.unreadable")).toBe(true);
  });

  test("project_hook_is_rechecked_before_it_starts: a script changed while the hook waits for a slot does not run", async () => {
    const f = fixture();
    const h = projectHook(f, [fakeSpec("p", ["--reply", "allow"])], "p");
    const slots = new HookSlots(1);
    const held = await slots.acquire(Date.now() + 1000);
    const d = deps(f, { slots, recheck: () => f.config.applies("p1", h) });
    const pending = run(h, request(f), d);
    await new Promise((r) => setTimeout(r, 100));
    writeFileSync(f.script, `${readFileSync(f.script, "utf8")}\n// changed\n`);
    held!();
    // Not a failure (Decision 3 does not apply): the hook no longer applies.
    expect(await pending).toEqual({ notRun: "changed", detail: expect.stringMatching(/changed/) });
    // Unchanged, it runs.
    writeFileSync(f.script, readFileSync(FAKE));
    const again = projectHook(f, [fakeSpec("p", ["--reply", "allow"])], "p");
    expect(await run(again, request(f), deps(f, { recheck: () => f.config.applies("p1", again) }))).toEqual({ reply: "allow" });
  });

  test("plan_configuration_runs: node fake-hook.mjs with file fake-hook.mjs, in the user and the project layer", async () => {
    const f = fixture();
    const user = userHook(f, ["--reply", "context:user"]);
    expect(user.run).toEqual(["node", "fake-hook.mjs", "--reply", "context:user"]);
    expect(await run(user, request(f), deps(f))).toEqual({ reply: "context", text: "user" });
    // The project's runs from a verified copy in the hook's temp (its argv names the copy).
    const project = projectHook(f, [fakeSpec("p", ["--report", "args"])], "p");
    expect(project.bound).toEqual({ [f.script]: expect.stringMatching(/^sha256:/) });
    const r = seen(await run(project, request(f), deps(f)));
    expect(r.args).toEqual(["--report", "args"]);
    // A relative argument that is not `file` is left as written.
    const other = userHook(f, ["--read", "fake-hook.mjs.not"]);
    expect(other.run.slice(2)).toEqual(["--read", "fake-hook.mjs.not"]);
  });

  test("script_swapped_after_check_is_not_run: the trusted bytes run, or nothing does", async () => {
    const f = fixture();
    const trusted = `${readFileSync(FAKE, "utf8")}\n// trusted\n`;
    const edited = `process.stdin.resume(); process.stdin.on("end", () => console.log(JSON.stringify({ reply: "context", text: "edited" })));\n`;
    writeFileSync(f.script, trusted);
    const h = projectHook(f, [fakeSpec("p", ["--reply", "context:trusted"])], "p");
    const swapAt = (stage: string) => async (s: string) => {
      if (s === stage) writeFileSync(f.script, edited);
    };
    // Swapped after the check, before the copy: the copy's hash differs, the hook does not run.
    expect(await run(h, request(f), deps(f, { recheck: () => f.config.applies("p1", h), onStage: swapAt("checked") }))).toEqual({ notRun: "changed", detail: expect.any(String) });
    // Swapped after the copy, before the start: the verified copy runs.
    writeFileSync(f.script, trusted);
    const h2 = projectHook(f, [fakeSpec("p", ["--reply", "context:trusted"])], "p");
    expect(await run(h2, request(f), deps(f, { recheck: () => f.config.applies("p1", h2), onStage: swapAt("copied") }))).toEqual({ reply: "context", text: "trusted" });
  });

  test("project_run_args_are_refused: an absolute path into the project, a .. segment, a project program with another file", () => {
    const f = fixture();
    mkdirSync(join(tmp, "outside"));
    const cases: Array<[unknown, RegExp]> = [
      [fakeSpec("a", ["--read", join(f.dir, "secret.txt")]), /names the project folder/],
      [fakeSpec("b", ["--read", "sub/../fake-hook.mjs"]), /\.\. segment/],
      [fakeSpec("c", ["--read", "..\\up"]), /\.\. segment/],
      [{ id: "d", event: "PreCommand", run: ["./fake-hook.mjs"], file: "data.json" }, /is the hook's own file/],
    ];
    writeFileSync(join(f.dir, "data.json"), "{}");
    for (const [spec, why] of cases) {
      writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [spec] }));
      const p = f.config.project("p1");
      expect(p.reason, JSON.stringify(spec)).toBe("invalidHooks");
      expect(p.invalid).toMatch(why);
      expect(p.hooksHash).toBeUndefined();
    }
    // An absolute path outside the project is fine (not project content).
    writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [fakeSpec("e", ["--read", join(tmp, "outside", "x")])] }));
    expect(f.config.project("p1").reason).toBeUndefined();
  });
});

describe("Windows grants (Decision 12)", () => {
  /** A broker that records; `active` as on Windows. */
  function recordingBroker() {
    const log: string[] = [];
    const broker: AclBroker = {
      active: true,
      grant: async (g) => void log.push(`grant ${g.owner} ${g.path} ${g.mask.toString(16)}`),
      revoke: async (owner) => (log.push(`revoke ${owner}`), 0),
      sweep: async () => 0,
    };
    return { log, broker };
  }

  test.skipIf(WIN)("windows_hook_grants_owner_call_revoked_at_exit: and on a failed start", async () => {
    const f = fixture();
    // A launcher that runs the program after reading the request (the AppContainer is the real
    // launcher's on Windows; the grants are what this proves).
    const launcher = join(tmp, "pass-launcher.sh");
    writeFileSync(launcher, `#!/bin/sh\ncat <&3 >/dev/null\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    const { log, broker } = recordingBroker();
    const tables: HookInput[] = [];
    const d = deps(f, {
      platform: "win32",
      status: () => ({ ...sandboxStatus(), os: "windows", hook: { kind: "enforced", detail: "stub" } }),
      launcher: () => launcher,
      acl: broker,
      containerSid: () => "S-1-15-2-1",
      hookGrants: (input) => {
        tables.push(input);
        return [
          { path: input.tmp, trustee: { kind: "container", value: "c" }, mask: 0x1301bf, scope: "owner" },
          { path: input.projectDir, trustee: { kind: "container", value: "p" }, mask: 0x1200a9, scope: "static" },
        ] as AclGrant[];
      },
    });
    // The input's limit is what is left of the deadline once a slot is taken (N1).
    const slots = new HookSlots(1);
    const held = await slots.acquire(Date.now() + 1000);
    setTimeout(() => held!(), 400);
    expect(await run(userHook(f, ["--reply", "allow"]), request(f), { ...d, slots })).toEqual({ reply: "allow" });
    expect(tables[0]!.timeoutMs).toBeLessThanOrEqual(10_000 - 350);
    expect(tables[0]!.timeoutMs).toBeGreaterThan(0);
    const owner = `call:${tables[0]!.tmp.split(/[\\/]/).at(-1)}`;
    expect(log).toEqual([`grant ${owner} ${tables[0]!.tmp} 1301bf`, `grant static ${f.dir} 1200a9`, `revoke ${owner}`]);
    // A start that fails after the grants revokes them too.
    log.length = 0;
    const failing = { ...d, launcher: () => join(tmp, "no-such-launcher") };
    expect(await run(userHook(f, ["--reply", "allow"]), request(f), failing)).toMatchObject({ kind: "unavailable" });
    expect(log.filter((l) => l.startsWith("revoke"))).toHaveLength(1);
    expect(log.at(-1)).toMatch(/^revoke call:/);
  });

  test("hook_file_is_granted_read_only: the table names a hook file outside the project with a read mask", () => {
    const sd = join(tmp, "sd");
    const input: HookInput = { projectDir: join(tmp, "P"), hookFile: join(tmp, "user-hooks", "lint.mjs"), toolchains: [], tmp: join(sd, "hooks", "run", "01J"), serverData: sd, home: join(tmp, "home") };
    for (const d of [input.projectDir, dirname(input.hookFile), input.tmp]) mkdirSync(d, { recursive: true });
    writeFileSync(input.hookFile, "");
    const table = sandboxHookGrants(input);
    const file = table.find((g) => g.path === input.hookFile);
    // READ_EXECUTE: no write, no delete.
    expect(file).toMatchObject({ mask: 0x1200a9, scope: "owner" });
    expect(table.find((g) => g.path === input.tmp)).toMatchObject({ scope: "owner" });
  });

  test.skipIf(!WIN)("windows_argument_boundaries_survive_in_appcontainer", async () => {
    if (!enforced("windows_argument_boundaries_survive_in_appcontainer")) return;
    const f = fixture();
    const { aclBroker } = await import("../src/sandbox/broker.ts");
    const odd = `a b  "c" \\d\\ e\\`;
    const r = await run(userHook(f, ["--report", "args", "--read", odd]), request(f), deps(f, { status: sandboxStatus, acl: aclBroker(f.serverData) }));
    expect(seen(r).args).toEqual(["--report", "args", "--read", odd]);
  });

  test.skipIf(!WIN)("windows_hook_reads_project_but_cannot_write_it", async () => {
    if (!enforced("windows_hook_reads_project_but_cannot_write_it")) return;
    const f = fixture();
    const { aclBroker } = await import("../src/sandbox/broker.ts");
    writeFileSync(join(f.dir, "readme.txt"), "hi\n");
    const r = seen(await run(userHook(f, ["--read", join(f.dir, "readme.txt"), "--write", join(f.dir, "planted.txt"), "--write", "own.txt"]), request(f), deps(f, { status: sandboxStatus, acl: aclBroker(f.serverData) })));
    expect(r.read[join(f.dir, "readme.txt")]).toBe("ok");
    expect(r.write[join(f.dir, "planted.txt")]).not.toBe("ok");
    expect(r.write["own.txt"]).toBe("ok");
  });
});

describe("the hook profile under the real launcher (Linux and macOS)", () => {
  const launched = (f: Fixture, over: Partial<HookProcessDeps> = {}) => deps(f, { status: sandboxStatus, ...over });

  test.skipIf(WIN)("hook_cannot_reach_network: TCP to loopback refused, no address", async () => {
    if (!enforced("hook_cannot_reach_network")) return;
    const f = fixture();
    const server = createServer((s) => s.end());
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    try {
      const port = (server.address() as { port: number }).port;
      // The control: unconfined, it connects.
      expect(seen(await run(userHook(f, ["--connect", String(port)]), request(f), deps(f))).connect).toBe("ok");
      const r = seen(await run(userHook(f, ["--connect", String(port), "--report", "net"]), request(f), launched(f)));
      expect(r.connect).not.toBe("ok");
      // No address at all where the launcher has its namespaces (else Landlock's TCP rules
      // alone refuse the connect, and the host warns `SANDBOX_SOCKETS_EXPOSED`).
      if (LINUX && probeLauncher().kind === "available") expect(r.net).toEqual([]);
    } finally {
      server.close();
    }
  });

  test.skipIf(WIN)("hook_reads_project_but_cannot_write_it", async () => {
    if (!enforced("hook_reads_project_but_cannot_write_it")) return;
    const f = fixture();
    writeFileSync(join(f.dir, "readme.txt"), "hi\n");
    const r = seen(await run(userHook(f, ["--read", join(f.dir, "readme.txt"), "--write", join(f.dir, "planted.txt")]), request(f), launched(f)));
    expect(r.read[join(f.dir, "readme.txt")]).toBe("ok");
    expect(r.write[join(f.dir, "planted.txt")]).toMatch(/EACCES|EPERM|EROFS/);
    expect(existsSync(join(f.dir, "planted.txt"))).toBe(false);
  });

  test.skipIf(WIN)("hook_cannot_read_server_data: auth/local.key", async () => {
    if (!enforced("hook_cannot_read_server_data")) return;
    const f = fixture();
    mkdirSync(join(f.serverData, "auth"), { recursive: true });
    const key = join(f.serverData, "auth", "local.key");
    writeFileSync(key, "secret\n", { mode: 0o600 });
    // The control: unconfined, it reads it.
    expect(seen(await run(userHook(f, ["--read", key]), request(f), deps(f))).read[key]).toBe("ok");
    expect(seen(await run(userHook(f, ["--read", key]), request(f), launched(f))).read[key]).toMatch(/EACCES|EPERM|ENOENT/);
  });

  test.skipIf(WIN)("hook_writes_only_its_temp", async () => {
    if (!enforced("hook_writes_only_its_temp")) return;
    const f = fixture();
    const elsewhere = join(tmp, "elsewhere.txt");
    const r = seen(await run(userHook(f, ["--write", "own.txt", "--write", elsewhere, "--write", join(f.serverData, "planted.txt"), "--report", "cwd"]), request(f), launched(f)));
    expect(r.write["own.txt"]).toBe("ok");
    expect(r.write[elsewhere]).toMatch(/EACCES|EPERM|EROFS/);
    expect(r.write[join(f.serverData, "planted.txt")]).toMatch(/EACCES|EPERM|EROFS/);
    expect(existsSync(elsewhere) || existsSync(join(f.serverData, "planted.txt"))).toBe(false);
  });

  test.skipIf(WIN)("hook_file_is_granted_read_only: the script is readable, a sibling outside the project is not", async () => {
    if (!enforced("hook_file_is_granted_read_only")) return;
    const f = fixture();
    const outside = join(tmp, "user-hooks");
    mkdirSync(outside);
    const script = join(outside, "hook.mjs");
    copyFileSync(FAKE, script);
    writeFileSync(join(outside, "sibling.txt"), "no\n");
    writeFileSync(join(f.serverData, "settings.json"), JSON.stringify({ hooks: [{ id: "o", event: "PreCommand", run: ["node", script, "--read", script, "--read", join(outside, "sibling.txt"), "--write", script], file: script }] }));
    const r = seen(await run(cmd(f.config.hooks("p1")[0]), request(f), launched(f)));
    expect(r.read[script]).toBe("ok");
    expect(r.read[join(outside, "sibling.txt")]).toMatch(/EACCES|EPERM/);
    expect(r.write[script]).toMatch(/EACCES|EPERM|EROFS/);
  });

  test.skipIf(WIN)("argument_boundaries_survive: under the launcher", async () => {
    if (!enforced("argument_boundaries_survive_under_the_launcher")) return;
    const f = fixture();
    const odd = `a b  "c" \\d\\ e\\`;
    const r = seen(await run(userHook(f, ["--report", "args", "--read", odd]), request(f), launched(f)));
    expect(r.args).toEqual(["--report", "args", "--read", odd]);
  });

  test.skipIf(WIN)("timeout_kills_tree: under the launcher (hung up)", async () => {
    if (!enforced("timeout_kills_tree_under_the_launcher")) return;
    const f = fixture();
    // A marker in the sleeper's argv: its pid is the launcher's namespace's, so it is found by name.
    const marker = `hook-sleeper-${process.pid}-${Math.random().toString(36).slice(2)}`;
    const h = userHook(f, ["--fork-sleeper", marker, "--sleep", "60000"], { timeoutS: 2 });
    const pending = run(h, request(f), launched(f));
    if (LINUX) await until(() => (markerAlive(marker) ? true : undefined));
    expect(await pending).toMatchObject({ kind: "timeout" });
    if (LINUX) {
      await until(() => (markerAlive(marker) ? undefined : true));
      expect(markerAlive(marker)).toBe(false);
    }
  });

  test.skipIf(WIN)("hook_env_is_cleared: under the launcher", async () => {
    if (!enforced("hook_env_is_cleared_under_the_launcher")) return;
    const f = fixture();
    process.env.SECRET_X = "s3cret";
    try {
      const env = seen(await run(userHook(f, ["--report", "env"]), request(f), launched(f))).env as Record<string, string>;
      expect(Object.keys(env).sort()).toEqual(["FIN_HOOK_EVENT", "HOME", "LANG", "PATH", "TEMP", "TMP", "TMPDIR"]);
    } finally {
      delete process.env.SECRET_X;
    }
  });

  test.skipIf(WIN)("plan_configuration_runs_under_the_launcher: user and project layers, a native project program from its copy", async () => {
    if (!enforced("plan_configuration_runs_under_the_launcher")) return;
    const f = fixture();
    expect(await run(userHook(f, ["--reply", "context:user"]), request(f), launched(f))).toEqual({ reply: "context", text: "user" });
    mkdirSync(join(f.dir, "hooks"));
    writeFileSync(join(f.dir, "hooks", "check.sh"), `#!/bin/sh\ncat >/dev/null\necho '{"reply":"context","text":"native"}'\n`, { mode: 0o755 });
    const specs = [fakeSpec("p", ["--reply", "context:project"]), { id: "n", event: "PreCommand", run: ["./hooks/check.sh"] }];
    expect(await run(projectHook(f, specs, "p"), request(f), launched(f))).toEqual({ reply: "context", text: "project" });
    // Landlock executes the copy only as the hook's own file (the temp is not an execute root).
    expect(await run(projectHook(f, specs, "n"), request(f), launched(f))).toEqual({ reply: "context", text: "native" });
  });
});
