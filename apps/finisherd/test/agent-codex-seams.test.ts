// The sandbox seams Codex needs (plan 7E Task 5): the session's own `HOME` and `SHELL`
// (Decision 5), Codex's and OpenAI's secret and internal variables never in a session, the one
// name a Codex process adds (`NO_COLOR`), provider directory hygiene (Decision 6, with the four
// names Task 0 added), and the host sandboxes: detection reads the sign-in store and never writes
// it, the probe has none.
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { childEnv, passEnv, secretName } from "../src/env.ts";
import { assertSessionEnv, openHostSandbox, openSessionSandbox, type SessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { assertSpawnEnv, harnessExtra, NEVER_IN_SESSION, SESSION_SHELLS, sessionEnv } from "../src/agent/sandbox-env.ts";
import { ensureSessionDirs, plantedName, providerDirClean, sessionDirs } from "../src/agent/session-dirs.ts";
import { resolveSandboxExecPath } from "../src/sandbox/exec.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const require = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "codex-seams-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const withAgent = (agent: SandboxStatus["agent"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), agent });
const NOT_READY = withAgent({ kind: "disabled", reason: "profileNotReady" });
const ENFORCED_STUB = withAgent({ kind: "enforced", detail: "stub" });
const SESSION_ID = "01S0000000000000000000000C";
const CODEX = { id: SESSION_ID, harness: "codex" as const };

/** Decision 5's names, each set in the server's environment. */
const CODEX_SECRETS: Readonly<Record<string, string>> = {
  CODEX_ACCESS_TOKEN: "t",
  CODEX_API_KEY: "k",
  OPENAI_API_KEY: "k",
  OPENAI_BASE_URL: "https://example.invalid/v1",
  OPENAI_ORG_ID: "org",
  OPENAI_PROJECT_ID: "proj",
  CODEX_SANDBOX: "seatbelt",
  CODEX_SANDBOX_NETWORK_DISABLED: "1",
  CODEX_INTERNAL_ORIGINATOR_OVERRIDE: "x",
  RUST_LOG: "trace",
};
const parent = WIN
  ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", USERPROFILE: "C:\\Users\\u", ...CODEX_SECRETS }
  : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", SHELL: "/usr/bin/zsh", LANG: "C.UTF-8", ...CODEX_SECRETS };

function deps(over: Partial<SessionSandboxDeps> = {}): SessionSandboxDeps {
  const data = join(tmp, "data");
  const project = join(tmp, "P");
  mkdirSync(data, { recursive: true, mode: 0o700 });
  mkdirSync(project, { recursive: true });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data, projectDirOf: () => project, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts"), ...over };
}

async function within<A>(open: Effect.Effect<SessionSandbox, unknown, Scope.Scope>, f: (box: SessionSandbox) => Promise<A>): Promise<A> {
  const scope = await Effect.runPromise(Scope.make());
  try {
    const box = await Effect.runPromise(Scope.provide(scope)(open) as Effect.Effect<SessionSandbox>);
    return await f(box);
  } finally {
    await Effect.runPromise(Scope.close(scope, Exit.void));
  }
}

const session = () => ({ id: SESSION_ID, projectId: "01P0000000000000000000000A", harness: "codex", mode: "draft" }) as unknown as Session;

async function run(box: SessionSandbox, argv: readonly string[], env?: Record<string, string>) {
  const child = await Effect.runPromise(box.spawn(argv, { cwd: box.workspace, stdio: "pipe", ...(env === undefined ? {} : { env }) }));
  child.stdin.end();
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (b) => (stdout += String(b)));
  child.stderr.on("data", (b) => (stderr += String(b)));
  const exit = await child.exited;
  return { code: exit.code, stdout, stderr };
}

async function refusal(box: SessionSandbox, env: Record<string, string>) {
  const exit = await Effect.runPromiseExit(box.spawn([process.execPath, "-e", ""], { cwd: box.workspace, stdio: "pipe", env }));
  expect(Exit.isFailure(exit), JSON.stringify(env)).toBe(true);
  const s = JSON.stringify(Exit.isFailure(exit) ? exit.cause : null);
  return { code: /"code":"([A-Z_]+)"/.exec(s)?.[1], name: /"name":"([^"]+)"/.exec(s)?.[1], reason: /"reason":"([^"]+)"/.exec(s)?.[1] };
}

const printEnv = (...names: string[]) => [process.execPath, "-e", `for (const k of ${JSON.stringify(names)}) console.log(k + "=" + (process.env[k] ?? "-"))`];
const lines = (s: string) => Object.fromEntries(s.trim().split(/\r?\n/).map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]));

describe("the Codex session environment (Decision 5)", () => {
  test("codex_session_home_is_not_user_home", async () => {
    const d = deps();
    await within(openSessionSandbox(session(), "codex", d), async (box) => {
      const home = join(d.serverData, "agents", SESSION_ID, "tmp", "home");
      expect(box.env.HOME).toBe(home);
      expect(readdirSync(home)).toEqual([]);
      if (WIN) expect(box.env.USERPROFILE).toBe(home);
      else expect(SESSION_SHELLS).toContain(box.env.SHELL);
      const seen = lines((await run(box, printEnv("HOME", "SHELL", "CODEX_HOME"))).stdout);
      expect(seen.HOME).toBe(home);
      expect(seen.CODEX_HOME).toBe(join(d.serverData, "agents", SESSION_ID, "provider"));
      if (!WIN) expect(seen.SHELL).toBe(box.env.SHELL);
      // The contract requires both for Codex, as for Claude Code.
      expect(() => assertSessionEnv(box.env, CODEX, { expect: box.env })).not.toThrow();
      expect(() => assertSessionEnv({ ...box.env, HOME: "/home/u" }, CODEX)).toThrow(/not the session's own path/);
      const { HOME: _h, ...homeless } = box.env;
      expect(() => assertSessionEnv(homeless, CODEX)).toThrow(/a missing variable/);
      if (!WIN) {
        expect(() => assertSessionEnv({ ...box.env, SHELL: "/usr/bin/zsh" }, CODEX)).toThrow(/not a session shell/);
        const { SHELL: _s, ...shell } = box.env;
        expect(() => assertSessionEnv(shell, CODEX)).toThrow(/a missing variable/);
      }
      // A process cannot be handed the user's home back.
      expect(await refusal(box, { HOME: "/home/u" })).toMatchObject({ code: "SANDBOX_ENV", name: "HOME", reason: "a session variable" });
    });
    // Spelled for each platform, whatever the host.
    const linux = sessionEnv({ session: CODEX, server: "tcp:127.0.0.1:4100", projectDir: "/p/P", dirs: sessionDirs("/data/agents", SESSION_ID, "linux"), parent, platform: "linux" });
    expect([linux.HOME, linux.SHELL, linux.CODEX_HOME]).toEqual([`/data/agents/${SESSION_ID}/tmp/home`, expect.stringMatching(/^\/bin\/(ba)?sh$/), `/data/agents/${SESSION_ID}/provider`]);
    expect(() => assertSessionEnv(linux, CODEX, { expect: linux, platform: "linux" })).not.toThrow();
    const win = sessionEnv({ session: CODEX, server: "pipe", projectDir: "C:\\p\\P", dirs: sessionDirs("C:\\data\\agents", SESSION_ID, "win32"), parent: { Path: "C:\\Windows", USERPROFILE: "C:\\Users\\u" }, platform: "win32" });
    expect([win.HOME, win.USERPROFILE, win.SHELL]).toEqual([`C:\\data\\agents\\${SESSION_ID}\\tmp\\home`, `C:\\data\\agents\\${SESSION_ID}\\tmp\\home`, undefined]);
    expect(() => assertSessionEnv(win, CODEX, { expect: win, platform: "win32" })).not.toThrow();
    expect(() => assertSessionEnv({ ...win, USERPROFILE: "C:\\Users\\u" }, CODEX, { platform: "win32" })).toThrow(/not the session's own path/);
  });

  test("codex_secret_names_never_pass", async () => {
    // Every one is named: the list, `secretName` or the prefix rule holds it, not the allow-list alone.
    for (const k of ["CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_ORG_ID", "OPENAI_PROJECT_ID", "RUST_LOG"]) expect(NEVER_IN_SESSION, k).toContain(k);
    for (const k of ["CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_ORG_ID", "OPENAI_PROJECT_ID", "OPENAI_WHATEVER", "openai_whatever", "Codex_Access_Token", "codex_api_key"]) expect(secretName(k), k).toBe(true);
    expect(secretName("CODEX_HOME")).toBe(false);
    // Dropped by the child environment itself, and by a family's pass-through.
    const child = childEnv({}, parent, process.platform);
    const passed = passEnv(Object.keys(CODEX_SECRETS), parent);
    for (const k of ["CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_ORG_ID", "OPENAI_PROJECT_ID"]) {
      expect(child[k], k).toBeUndefined();
      expect(passed[k], k).toBeUndefined();
    }
    const d = deps();
    await within(openSessionSandbox(session(), "codex", d), async (box) => {
      for (const k of Object.keys(CODEX_SECRETS)) expect(box.env[k], k).toBeUndefined();
      const r = await run(box, printEnv(...Object.keys(CODEX_SECRETS)));
      expect(r.code, r.stderr).toBe(0);
      expect(lines(r.stdout)).toEqual(Object.fromEntries(Object.keys(CODEX_SECRETS).map((k) => [k, "-"])));
      for (const [k, v] of Object.entries(CODEX_SECRETS)) {
        // Refused even where an adapter declares the name (any case), and never addable to a process.
        for (const name of [k, k.toLowerCase()]) {
          expect(() => assertSessionEnv({ ...box.env, [name]: v }, CODEX, { extra: [name] }), name).toThrow(/a variable that never passes|a secret/);
          expect(() => assertSpawnEnv({ [name]: v }, CODEX), name).toThrow(/a variable that never passes|a secret/);
        }
        expect((await refusal(box, { [k]: v })).reason, k).toMatch(/a variable that never passes|a secret/);
      }
    });
  });

  test("codex_extra_names_only", async () => {
    expect(harnessExtra("codex")).toEqual(["NO_COLOR", "FIN_CALL"]);
    // No other harness gains it.
    expect(harnessExtra("fake")).toEqual(["FIN_CALL"]);
    expect(harnessExtra("claudeCode")).not.toContain("NO_COLOR");
    const d = deps();
    await within(openSessionSandbox(session(), "codex", d), async (box) => {
      const r = await run(box, printEnv("NO_COLOR", "FIN_CALL"), { NO_COLOR: "1", FIN_CALL: "01C0000000000000000000000A" });
      expect(r.code, r.stderr).toBe(0);
      expect(lines(r.stdout)).toEqual({ NO_COLOR: "1", FIN_CALL: "01C0000000000000000000000A" });
      expect(box.env.NO_COLOR).toBeUndefined();
      expect(await refusal(box, { NO_COLOR: "0" })).toMatchObject({ code: "SANDBOX_ENV", name: "NO_COLOR", reason: "a wrong value" });
      for (const k of ["CLAUDE_CODE_TMPDIR", "DISABLE_AUTOUPDATER", "TERM", "CODEX_HOME", "EDITOR"]) {
        expect((await refusal(box, { [k]: "1" })).code, k).toBe("SANDBOX_ENV");
      }
    });
    await within(openSessionSandbox({ ...session(), harness: "fake" } as Session, "fake", d), async (box) => {
      expect(await refusal(box, { NO_COLOR: "1" })).toMatchObject({ code: "SANDBOX_ENV", name: "NO_COLOR", reason: "a variable outside the harness's" });
    });
  });
});

describe("provider directory hygiene (Decision 6)", () => {
  /** Decision 6's list as the plan names it (spelled out here, not imported). */
  const FILES = ["config.toml", "work.config.toml", "hooks.json", "AGENTS.md", "AGENTS.override.md", "AGENTS.local.md", "requirements.toml", ".env", "environments.toml", "managed_config.toml"];
  const DIRS = ["rules", "skills", "prompts", "plugins", "agents"];

  const refused = async (provider: string) => {
    const exit = await Effect.runPromiseExit(Effect.tryPromise({ try: () => providerDirClean({ provider }), catch: (e) => e }));
    if (Exit.isSuccess(exit)) return undefined;
    const s = JSON.stringify(exit.cause);
    return { code: /"code":"([A-Z_]+)"/.exec(s)?.[1], missing: /"missing":"([^"]+)"/.exec(s)?.[1], item: /"item":"([^"]+)"/.exec(s)?.[1] };
  };

  test("provider_dir_planted_config_refused", async () => {
    const dirs = await ensureSessionDirs(join(tmp, "data", "agents"), SESSION_ID);
    const p = dirs.provider;
    // What Codex itself keeps there (Task 0) and the store link are not planted configuration.
    const target = join(tmp, "data", "providers", "codex", "home", "auth.json");
    symlinkSync(target, join(p, "auth.json"));
    for (const f of ["installation_id", ".sandbox_migration", "state_5.sqlite", "logs_2.sqlite-wal", "models_cache.json", "version.json"]) writeFileSync(join(p, f), "");
    for (const f of ["sessions", "archived_sessions", "tmp", "thread-writer-locks", "log"]) mkdirSync(join(p, f));
    expect(await refused(p)).toBeUndefined();
    // A provider directory not made yet holds nothing.
    expect(await refused(join(tmp, "nowhere"))).toBeUndefined();
    const plant = async (name: string, make: (at: string) => void) => {
      const at = join(p, name);
      make(at);
      try {
        return await refused(p);
      } finally {
        rmSync(at, { recursive: true, force: true });
      }
    };
    for (const name of FILES) {
      expect(await plant(name, (at) => writeFileSync(at, "x")), name).toEqual({ code: "PROVIDER_VERSION", missing: "isolation", item: name });
      // Any kind of entry by that name: a directory too (never looked into).
      expect((await plant(name, (at) => mkdirSync(at)))?.item, `${name}/`).toBe(name);
    }
    for (const name of DIRS) {
      expect(await plant(name, (at) => mkdirSync(at)), name).toEqual({ code: "PROVIDER_VERSION", missing: "isolation", item: name });
      expect((await plant(name, (at) => writeFileSync(at, "")))?.item, `${name} (a file)`).toBe(name);
    }
    // Names in any case (macOS and Windows find `CONFIG.TOML` as `config.toml`).
    expect((await plant("CONFIG.TOML", (at) => writeFileSync(at, "")))?.item).toBe("CONFIG.TOML");
    expect((await plant("Skills", (at) => mkdirSync(at)))?.item).toBe("Skills");
    // Windows drops trailing dots and spaces: `config.toml.` opens `config.toml`.
    expect((await plant("config.toml.", (at) => writeFileSync(at, "")))?.item).toBe("config.toml.");
    expect((await plant("skills ", (at) => mkdirSync(at)))?.item).toBe("skills ");
    expect((await plant("hooks.json. .", (at) => writeFileSync(at, "")))?.item).toBe("hooks.json. .");
    // Unicode case folding (APFS, NTFS) maps `ſ` (U+017F) to `s` and `ı` (U+0131) to `i`, which
    // `toLowerCase` does not: any name that is not printable ASCII is refused outright.
    for (const name of ["ſkills", "hookſ.json", "skılls", "AGENTſ.md", "conﬁg.toml", "notes-é"]) {
      expect((await plant(name, (at) => writeFileSync(at, "")))?.item, name).toBe(name);
    }
    expect(plantedName("ſkills")).toBe(true);
    expect(plantedName("models_cache.json")).toBe(false);
    // A link named like one is refused without being followed: dangling, to a directory, to a file.
    expect(await plant("config.toml", (at) => symlinkSync(join(tmp, "nothing-here"), at))).toEqual({ code: "PROVIDER_VERSION", missing: "isolation", item: "config.toml" });
    const elsewhere = join(tmp, "elsewhere");
    mkdirSync(join(elsewhere, "skills", "x"), { recursive: true });
    writeFileSync(join(elsewhere, "config.toml"), "model = 'x'");
    expect((await plant("skills", (at) => symlinkSync(join(elsewhere, "skills"), at, "dir")))?.item).toBe("skills");
    expect((await plant(".env", (at) => symlinkSync(join(elsewhere, "config.toml"), at)))?.item).toBe(".env");
    // Only names count: a link to a directory full of configuration is not looked into.
    expect(await plant("notes", (at) => symlinkSync(elsewhere, at, "dir"))).toBeUndefined();
    expect(lstatSync(join(p, "auth.json")).isSymbolicLink()).toBe(true);
    // The store was never made or opened.
    expect(existsSync(join(tmp, "data", "providers"))).toBe(false);
  });
});

describe("Codex host sandboxes (Decisions 8, 22)", () => {
  const stubbed = () => {
    const stub = join(tmp, "launcher.sh");
    const request = join(tmp, "request.json");
    writeFileSync(stub, `#!/bin/sh\ncat <&3 > '${request}'\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    return { d: deps({ status: ENFORCED_STUB, launcher: () => stub }), input: () => (JSON.parse(readFileSync(request, "utf8")) as { input: Record<string, unknown> }).input };
  };
  const realLauncher = () => {
    const m = sandboxStatus().agent;
    if (m.kind === "enforced" && existsSync(resolveSandboxExecPath())) return true;
    expect(require, `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
    return false;
  };

  test.skipIf(WIN)("detect_host_reads_store_never_writes", async () => {
    const { d, input } = stubbed();
    const home = join(d.serverData, "providers", "codex", "home");
    const store = join(home, "auth.json");
    await within(openHostSandbox("detect", "codex", d), async (box) => {
      expect((await run(box, ["/bin/sh", "-c", "true"])).code).toBe(0);
      const i = input();
      const id = String(i.sessionId);
      expect(id).toMatch(/^host-[0-9A-Z]{26}$/);
      // The store, by the link in its own `CODEX_HOME`; never the sign-in home as a whole.
      expect(i.signInStore).toBe(store);
      expect(i.signInHome).toBeUndefined();
      const provider = join(d.serverData, "agents", id, "provider");
      expect(box.env.CODEX_HOME).toBe(provider);
      expect(lstatSync(join(provider, "auth.json")).isSymbolicLink()).toBe(true);
      expect(box.env.HOME).toBe(join(d.serverData, "agents", id, "tmp", "home"));
      expect(() => assertSessionEnv(box.env, { id, harness: "codex" }, { host: true, expect: box.env })).not.toThrow();
    });
    expect(existsSync(join(d.serverData, "providers"))).toBe(false);
    if (!realLauncher()) return;
    const { status: _s, launcher: _l, ...real } = d;
    mkdirSync(home, { recursive: true, mode: 0o700 });
    writeFileSync(store, '{"tokens":{}}');
    await within(openHostSandbox("detect", "codex", real), async (box) => {
      expect(box.mode).toBe("launcher");
      const read = await run(box, ["/bin/sh", "-c", 'cat "$CODEX_HOME/auth.json"']);
      expect(read.code, read.stderr).toBe(0);
      expect(read.stdout).toBe('{"tokens":{}}');
      for (const write of ['echo x > "$CODEX_HOME/auth.json"', `echo x >> '${store}'`]) {
        expect((await run(box, ["/bin/sh", "-c", write])).code, write).not.toBe(0);
      }
      // Its own state is written as Codex writes it.
      expect((await run(box, ["/bin/sh", "-c", 'echo id > "$CODEX_HOME/installation_id"'])).code).toBe(0);
    });
    expect(readFileSync(store, "utf8")).toBe('{"tokens":{}}');
    // A session's Codex refreshes the store in place: read and write.
    await within(openSessionSandbox(session(), "codex", { ...real, agentListener: async () => "tcp:127.0.0.1:4100" }), async (box) => {
      expect(box.mode).toBe("launcher");
      const w = await run(box, ["/bin/sh", "-c", 'printf "{}" > "$CODEX_HOME/auth.json"']);
      expect(w.code, w.stderr).toBe(0);
    });
    expect(readFileSync(store, "utf8")).toBe("{}");
  });

  test.skipIf(WIN)("probe_host_has_no_store", async () => {
    const { d, input } = stubbed();
    await within(openHostSandbox("probe", "codex", d), async (box) => {
      expect((await run(box, ["/bin/sh", "-c", "true"])).code).toBe(0);
      const i = input();
      const id = String(i.sessionId);
      expect(i.signInStore).toBeUndefined();
      expect(i.signInHome).toBeUndefined();
      // A throwaway `CODEX_HOME` with no link in it.
      expect(box.env.CODEX_HOME).toBe(join(d.serverData, "agents", id, "provider"));
      expect(readdirSync(box.env.CODEX_HOME!)).toEqual([]);
      expect(() => assertSessionEnv(box.env, { id, harness: "codex" }, { host: true, expect: box.env })).not.toThrow();
    });
    expect(existsSync(join(d.serverData, "providers"))).toBe(false);
    if (!realLauncher()) return;
    const { status: _s, launcher: _l, ...real } = d;
    const home = join(real.serverData, "providers", "codex", "home");
    mkdirSync(home, { recursive: true, mode: 0o700 });
    writeFileSync(join(home, "auth.json"), "{}");
    await within(openHostSandbox("probe", "codex", real), async (box) => {
      expect(box.mode).toBe("launcher");
      expect(existsSync(join(box.env.CODEX_HOME!, "auth.json"))).toBe(false);
      expect((await run(box, ["/bin/cat", join(home, "auth.json")])).code).not.toBe(0);
    });
  });
});
