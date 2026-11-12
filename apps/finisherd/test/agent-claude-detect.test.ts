// Claude Code's detection, its `agent.providers` line and the sign-in commands (plan 7D Task 3,
// Decisions 9, 22, 23): the binary from `agents.claudeCode.path` or the absolute `PATH` entries,
// the native build only on Windows, `--version` against the floor, `auth status`'s JSON read
// for `loggedIn` and a one-word method only, nothing opened under `providers/`, and sign-in
// commands printed, never run. Everything runs `fake-claude` (no real CLI).
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { hostname, tmpdir, userInfo } from "node:os";
import { delimiter, join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { AgentProvidersSuccess, type FinError, type ProviderInfo } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Schema } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";
import { AgentAdmin } from "../src/agent/admin.ts";
import {
  CLAUDE_FLOOR,
  type ClaudeDetectDeps,
  claudeProviderInfo,
  compareVersions,
  detectClaude,
  invalidateClaudeDetection,
  parseAuthStatus,
  parseVersion,
  resolveClaudePath,
  signinCommands,
} from "../src/agent/claude/detect.ts";
import type { SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { readAgentSettings } from "../src/agent/settings.ts";
import { runConsole } from "../src/console/main.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

// Every path finisherd's own code hands `node:fs` (first two arguments), for the store spy.
const spy = vi.hoisted(() => {
  const touched: string[] = [];
  const wrap = <T extends object>(m: T): T => {
    const out: Record<string, unknown> = { ...(m as Record<string, unknown>) };
    for (const [k, v] of Object.entries(m)) {
      if (typeof v !== "function" || /^[A-Z]/.test(k)) continue;
      const f = v as (...a: unknown[]) => unknown;
      out[k] = Object.assign((...args: unknown[]) => {
        for (const a of args.slice(0, 2)) if (typeof a === "string" || a instanceof URL) touched.push(String(a));
        return f(...args);
      }, f);
    }
    return out as T;
  };
  return { touched, wrap };
});
vi.mock("node:fs", async (orig) => {
  const m = await orig<typeof import("node:fs")>();
  return { ...spy.wrap(m), default: spy.wrap(m) };
});
vi.mock("node:fs/promises", async (orig) => {
  const m = await orig<typeof import("node:fs/promises")>();
  return { ...spy.wrap(m), default: spy.wrap(m) };
});

const CRED = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_CODE_OAUTH_REFRESH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"];
const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
/** CI (`FIN_REQUIRE_SERVER=1`, as the other Rust-binary tests): a missing fake fails instead of skipping. */
const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";
const NO_FAKE = !existsSync(FAKE) && !REQUIRE_FAKE;
const needFake = () => {
  if (!existsSync(FAKE)) throw new Error(`FIN_REQUIRE_SERVER=1 but ${FAKE} is missing: run \`cargo build -p fin-agent-fakes\``);
};

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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-detect-")));
});
afterEach(() => {
  invalidateClaudeDetection();
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8" };

function sandboxDeps(): SessionSandboxDeps {
  const data = join(tmp, "data");
  mkdirSync(data, { recursive: true, mode: 0o700 });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data, projectDirOf: () => undefined, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts") };
}

/** `fake-claude` installed as `<dir>/claude` with its sidecar: a version and an `auth status` override. */
function installFake(name: string, o: { version?: string; auth?: Record<string, unknown> } = {}): string {
  const dir = join(tmp, name);
  mkdirSync(dir, { recursive: true });
  const bin = join(dir, WIN ? "claude.exe" : "claude");
  copyFileSync(FAKE, bin);
  chmodSync(bin, 0o755);
  writeFileSync(join(dir, "script.json"), JSON.stringify(o.auth === undefined ? {} : { auth: o.auth }));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", ...(o.version === undefined ? {} : { version: o.version }) }));
  return dir;
}

/** A `claude` that records it was run (a file next to it) and prints nothing useful. */
function installMarker(name: string): { dir: string; marker: string } {
  const dir = join(tmp, name);
  mkdirSync(dir, { recursive: true });
  const marker = join(dir, "ran");
  writeFileSync(join(dir, "claude"), `#!/bin/sh\necho "$@" >> '${marker}'\n`, { mode: 0o755 });
  return { dir, marker };
}

const deps = (pathEnv: string, over: Partial<ClaudeDetectDeps> = {}): ClaudeDetectDeps => ({ sandbox: sandboxDeps(), settings: () => ({ enabled: true }), pathEnv, ...over });

async function failure(e: Effect.Effect<unknown, FinError>): Promise<FinError> {
  const exit = await Effect.runPromiseExit(e);
  if (!Exit.isFailure(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit)}`);
  const err = exit.cause.reasons.map((r) => (r._tag === "Fail" ? r.error : undefined)).find((x) => x !== undefined);
  if (err === undefined) throw new Error(`no failure in ${String(exit.cause)}`);
  return err;
}

const thrown = (f: () => unknown): FinError => {
  try {
    f();
  } catch (e) {
    return e as FinError;
  }
  throw new Error("expected a throw");
};

describe("detection", () => {
  test("not_installed_is_unavailable", async () => {
    const empty = join(tmp, "empty");
    mkdirSync(empty);
    const e = await failure(detectClaude(deps(empty)));
    expect(e).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { harness: "claudeCode", reason: "notOnPath" } });
    expect(e.fix).toMatch(/agents\.claudeCode\.path/);
    // A setting naming a missing file is the same answer; the line says not detected.
    const missing = await failure(detectClaude(deps(empty, { settings: () => ({ enabled: true, path: join(tmp, "nope", "claude") }) })));
    expect(missing).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { harness: "claudeCode", reason: "pathMissing" } });
    const info = await Effect.runPromise(claudeProviderInfo(deps(empty)));
    expect(info).toMatchObject({ harness: "claudeCode", detected: false, floor: CLAUDE_FLOOR, enabled: true, problem: { code: "PROVIDER_UNAVAILABLE" } });
  });

  test.skipIf(NO_FAKE)("version_parsed_and_floor_enforced", async () => {
    needFake();
    expect(parseVersion("2.1.321 (Claude Code)\n")).toBe("2.1.321");
    expect(compareVersions("2.1.1000", "2.1.321")).toBeGreaterThan(0);
    expect(compareVersions("2.1.314", "2.1.321")).toBeLessThan(0);
    expect(compareVersions(CLAUDE_FLOOR, "2.1.314")).toBeGreaterThanOrEqual(0);
    const ok = await Effect.runPromise(detectClaude(deps(installFake("ok"))));
    expect(ok).toMatchObject({ path: join(tmp, "ok", WIN ? "claude.exe" : "claude"), version: "2.1.321", floor: CLAUDE_FLOOR });
    const newer = await Effect.runPromise(detectClaude(deps(installFake("newer", { version: "2.2.0" }))));
    expect(newer.version).toBe("2.2.0");
    const old = installFake("old", { version: "2.1.300" });
    const e = await failure(detectClaude(deps(old)));
    expect(e).toMatchObject({ code: "PROVIDER_VERSION", details: { harness: "claudeCode", version: "2.1.300", floor: CLAUDE_FLOOR } });
    const info = await Effect.runPromise(claudeProviderInfo(deps(old)));
    expect(info).toMatchObject({ detected: true, version: "2.1.300", supported: false, floor: CLAUDE_FLOOR, problem: { code: "PROVIDER_VERSION" } });
  });

  test.skipIf(NO_FAKE)("garbled_version_is_protocol", async () => {
    needFake();
    for (const line of ["2.1.321", "Claude Code 2.1.321", "2.1.321 (Claude Code) extra", "v2.1.321 (Claude Code)", ""]) {
      expect(thrown(() => parseVersion(line)).code, line).toBe("PROVIDER_PROTOCOL");
    }
    const e = await failure(detectClaude(deps(installFake("garbled", { version: "two.one (beta)" }))));
    expect(e).toMatchObject({ code: "PROVIDER_PROTOCOL", details: { harness: "claudeCode", reason: "version" } });
    expect(await Effect.runPromise(claudeProviderInfo(deps(join(tmp, "garbled"))))).toMatchObject({ detected: true, problem: { code: "PROVIDER_PROTOCOL" } });
    if (WIN) return;
    // A CLI that does not answer within the timeout is killed and is the same refusal.
    const slow = join(tmp, "slow");
    mkdirSync(slow);
    writeFileSync(join(slow, "claude"), "#!/bin/sh\nexec sleep 30\n", { mode: 0o755 });
    const started = Date.now();
    const t = await failure(detectClaude(deps(slow, { timeoutMs: 300 })));
    expect(t).toMatchObject({ code: "PROVIDER_PROTOCOL", details: { reason: "timeout", step: "--version" } });
    expect(Date.now() - started).toBeLessThan(10_000);
  });

  test.skipIf(NO_FAKE)("detect_never_reaches_sign_in_home", async () => {
    needFake();
    const dir = installFake("out", { auth: { loggedIn: false } });
    const d = deps(dir);
    // A signed-in home with a store: detection hands its store to the CLI, and opens nothing there.
    const store = join(d.sandbox.serverData, "providers", "claudeCode", "home", "store");
    mkdirSync(store, { recursive: true, mode: 0o700 });
    writeFileSync(join(store, "credentials"), "not for finisherd");
    spy.touched.length = 0;
    const r = await Effect.runPromise(detectClaude(d));
    const seen = [...spy.touched];
    expect(r.signedIn).toBe(false);
    const providers = join(d.sandbox.serverData, "providers");
    expect(seen.length, "the fs spy records finisherd's calls").toBeGreaterThan(0);
    expect(seen.some((p) => p.includes(`${sep}agents${sep}host-`))).toBe(true);
    expect(seen.filter((p) => p === providers || p.startsWith(providers + sep))).toEqual([]);
    // The CLI's own config dir was the host sandbox's throwaway one, removed at close.
    const out = await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const { openHostSandbox } = yield* Effect.promise(() => import("../src/agent/sandbox.ts"));
          const box = yield* openHostSandbox("detect", "claudeCode", d.sandbox);
          return { config: box.env.CLAUDE_CONFIG_DIR, store: box.env.CLAUDE_SECURESTORAGE_CONFIG_DIR };
        }),
      ),
    );
    expect(out.config).toMatch(new RegExp(`${join(d.sandbox.serverData, "agents").replaceAll("\\", "\\\\")}.host-[0-9A-Z]{26}.provider$`));
    expect(out.store).toBe(store);
    expect(readdirSync(join(d.sandbox.serverData, "agents")).filter((n) => n.startsWith("host-"))).toEqual([]);
    expect(readdirSync(store)).toEqual(["credentials"]);
  });

  test.skipIf(NO_FAKE)("signed_in_from_auth_status_json", async () => {
    needFake();
    const signedIn = await Effect.runPromise(detectClaude(deps(installFake("in"))));
    expect(signedIn).toMatchObject({ signedIn: true, authMethod: "claude.ai" });
    expect(signedIn.problem).toBeUndefined();
    // Signed out: exit 1 with JSON is an answer, not a failure; no method is kept.
    const out = await Effect.runPromise(detectClaude(deps(installFake("out", { auth: { loggedIn: false } }))));
    expect(out).toMatchObject({ signedIn: false });
    expect(out.authMethod).toBeUndefined();
    // A method outside the allow-list (it could be anything) is "other", never its text.
    const odd = await Effect.runPromise(detectClaude(deps(installFake("odd", { auth: { authMethod: "someone@example.invalid" } }))));
    expect(odd).toMatchObject({ signedIn: true, authMethod: "other" });
    // The parser on its own: exit 0/1 with a boolean `loggedIn`, nothing else.
    expect(parseAuthStatus(0, JSON.stringify({ loggedIn: true, authMethod: "console", email: "x@y.z" }))).toEqual({ signedIn: true, authMethod: "console" });
    expect(parseAuthStatus(1, JSON.stringify({ loggedIn: false, authMethod: "none" }))).toEqual({ signedIn: false });
    for (const [code, body] of [[2, '{"loggedIn": false}'], [0, "Logged in"], [1, '{"loggedIn": "no"}'], [null, "{}"], [0, "[]"]] as const) {
      expect(thrown(() => parseAuthStatus(code, body)).code, body).toBe("PROVIDER_PROTOCOL");
    }
    // An unusable answer leaves the version standing, `signedIn` unknown with the problem.
    if (WIN) return;
    const broken = join(tmp, "broken");
    mkdirSync(broken);
    writeFileSync(join(broken, "claude"), '#!/bin/sh\nif [ "$1" = --version ]; then echo "2.1.321 (Claude Code)"; else echo "Logged in"; fi\n', { mode: 0o755 });
    const b = await Effect.runPromise(detectClaude(deps(broken)));
    expect(b).toMatchObject({ version: "2.1.321", signedIn: null, problem: { code: "PROVIDER_PROTOCOL" } });
  });

  test.skipIf(NO_FAKE || WIN)("detection_cached_and_single_flight", async () => {
    needFake();
    const dir = join(tmp, "counted");
    mkdirSync(dir);
    const log = join(dir, "runs");
    writeFileSync(
      join(dir, "claude"),
      `#!/bin/sh\necho "$1" >> '${log}'\nif [ "$1" = --version ]; then echo "2.1.321 (Claude Code)"; else sleep 0.3; echo '{"loggedIn":true,"authMethod":"console"}'; fi\n`,
      { mode: 0o755 },
    );
    const runs = () => (existsSync(log) ? readFileSync(log, "utf8").trim().split("\n") : []);
    let clock = 1_000_000;
    const d = deps(dir, { now: () => clock });
    // Concurrent callers share one run of each command.
    const all = await Promise.all([1, 2, 3, 4].map(() => Effect.runPromise(detectClaude(d))));
    expect(all.every((r) => r.signedIn === true && r.authMethod === "console")).toBe(true);
    expect(runs()).toEqual(["--version", "auth"]);
    // Within 30 s nothing runs again; after it only `auth status` does (the version is the same file).
    await Effect.runPromise(claudeProviderInfo(d));
    expect(runs()).toHaveLength(2);
    clock += 31_000;
    await Effect.runPromise(detectClaude(d));
    expect(runs()).toEqual(["--version", "auth", "auth"]);
    // Invalidation forgets both.
    invalidateClaudeDetection();
    await Effect.runPromise(detectClaude(d));
    expect(runs()).toEqual(["--version", "auth", "auth", "--version", "auth"]);
  });

  test.skipIf(NO_FAKE || WIN)("cache_invalidated_by_binary_change", async () => {
    needFake();
    const dir = join(tmp, "swap");
    mkdirSync(dir);
    const script = (v: string) => `#!/bin/sh\nif [ "$1" = --version ]; then echo "${v} (Claude Code)"; else echo '{"loggedIn":true}'; fi\n`;
    writeFileSync(join(dir, "claude"), script("2.1.321"), { mode: 0o755 });
    const d = deps(dir);
    expect((await Effect.runPromise(detectClaude(d))).version).toBe("2.1.321");
    // The same path now holds another build (a different size): the cached version is not reused.
    writeFileSync(join(dir, "claude"), script("2.1.400"), { mode: 0o755 });
    expect((await Effect.runPromise(detectClaude(d))).version).toBe("2.1.400");
    // Same size, new mtime.
    writeFileSync(join(dir, "claude"), script("2.1.401"), { mode: 0o755 });
    utimesSync(join(dir, "claude"), new Date(0), new Date(1_000_000));
    expect((await Effect.runPromise(detectClaude(d))).version).toBe("2.1.401");
  });

  test.skipIf(NO_FAKE)("providers_never_carry_label_or_token", async () => {
    needFake();
    const label = "owner-label-7c1@example.invalid";
    const token = "sk-ant-oat01-SECRETTOKEN";
    const dir = installFake("label", { auth: { email: label, orgName: "Label Org 7c1", accessToken: token, authMethod: "claude.ai" } });
    const d = deps(dir);
    const admin = new AgentAdmin({ serverData: d.sandbox.serverData, secrets: undefined, localPresets: {}, audit: () => undefined, claudeCode: () => claudeProviderInfo(d) });
    const answer = await Effect.runPromise(admin.providers(new Map(), () => true));
    // What the RPC sends, through its schema.
    const wire = JSON.stringify(Schema.encodeUnknownSync(AgentProvidersSuccess)(answer));
    for (const s of [label, "Label Org 7c1", token, "example.invalid"]) expect(wire, s).not.toContain(s);
    const line = answer.providers.find((p) => p.harness === "claudeCode") as ProviderInfo;
    expect(line).toMatchObject({ detected: true, version: "2.1.321", supported: true, signedIn: true, authMethod: "claude.ai", floor: CLAUDE_FLOOR, enabled: true });
    expect(line.account).toBeUndefined();
    expect(line.accountHash).toBeUndefined();
    expect(Object.keys(line).sort()).toEqual(["authMethod", "capabilities", "detected", "enabled", "floor", "harness", "signedIn", "supported", "version"]);
    // The schema holds `accountHash` to 8 hex characters.
    expect(() => Schema.decodeUnknownSync(AgentProvidersSuccess)({ ...answer, providers: [{ ...line, accountHash: label }] })).toThrow();
    expect(() => Schema.decodeUnknownSync(AgentProvidersSuccess)({ ...answer, providers: [{ ...line, accountHash: "0a1b2c3d" }] })).not.toThrow();
  });

  test("disabled_by_default", async () => {
    const d = sandboxDeps();
    // No settings, and an unusable value: off.
    expect(readAgentSettings(d.serverData).settings.claudeCode).toEqual({ enabled: false });
    writeFileSync(join(d.serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: "yes", path: "" } } }));
    const bad = readAgentSettings(d.serverData);
    expect(bad.settings.claudeCode).toEqual({ enabled: false });
    expect(bad.error).toMatch(/agents\.claudeCode\.path.*agents\.claudeCode\.enabled/);
    writeFileSync(join(d.serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: "/opt/claude/bin/claude" } } }));
    expect(readAgentSettings(d.serverData).settings.claudeCode).toEqual({ enabled: true, path: "/opt/claude/bin/claude" });
    rmSync(join(d.serverData, "settings.json"));
    if (WIN) return;
    // Off, the line says so, and the CLI is looked for but never run.
    const { dir, marker } = installMarker("off");
    const info = await Effect.runPromise(claudeProviderInfo({ sandbox: d, pathEnv: dir }));
    expect(info).toMatchObject({ harness: "claudeCode", detected: true, enabled: false, floor: CLAUDE_FLOOR, problem: { code: "PROVIDER_UNAVAILABLE", fix: expect.stringMatching(/agents\.claudeCode\.enabled/) } });
    expect(info.version).toBeUndefined();
    expect(existsSync(marker)).toBe(false);
  });
});

describe("where the CLI is", () => {
  test("relative_path_entry_ignored", () => {
    const asked: string[] = [];
    const isFile = (p: string) => (asked.push(p), false);
    const e = thrown(() => resolveClaudePath({ pathEnv: [".", "bin", "", "./x", "/abs/dir"].join(":"), platform: "linux", isFile }));
    expect(e).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { reason: "notOnPath" } });
    expect(asked).toEqual(["/abs/dir/claude"]);
    // A relative entry before the real one is skipped even when it would hold a `claude`.
    const found = resolveClaudePath({ pathEnv: ["bin", "/usr/local/bin"].join(":"), platform: "linux", isFile: (p) => p === "bin/claude" || p === "/usr/local/bin/claude" });
    expect(found).toBe("/usr/local/bin/claude");
    // Windows: a drive-less rooted or relative entry is skipped too; quotes are taken off.
    const winAsked: string[] = [];
    expect(() => resolveClaudePath({ pathEnv: ["\\tools", "tools", '"C:\\Program Files\\Claude"'].join(";"), platform: "win32", isFile: (p) => (winAsked.push(p), false) })).toThrow();
    expect(winAsked.every((p) => p.startsWith("C:\\Program Files\\Claude\\"))).toBe(true);
    // The setting must be absolute.
    expect(thrown(() => resolveClaudePath({ setting: "bin/claude", platform: "linux", isFile: () => true }))).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { reason: "relativePath" } });
    // A real relative entry with a real `claude` in it is never used.
    if (WIN) return;
    const { dir } = installMarker("rel");
    expect(thrown(() => resolveClaudePath({ pathEnv: [`.${sep}${dir.slice(tmp.length + 1)}`, join(tmp, "nothing")].join(delimiter), platform: "linux" })).code).toBe("PROVIDER_UNAVAILABLE");
    expect(resolveClaudePath({ pathEnv: [join(tmp, "nothing"), dir].join(delimiter), platform: "linux" })).toBe(join(dir, "claude"));
  });

  test("windows_npm_shim_refused", () => {
    const files = new Set(["C:\\npm\\claude.cmd", "C:\\npm\\claude.ps1", "C:\\npm\\claude", "C:\\native\\claude.exe"]);
    const isFile = (p: string) => files.has(p);
    // The first folder holding a `claude` is npm's: its shim is refused, not skipped.
    const e = thrown(() => resolveClaudePath({ pathEnv: "C:\\npm;C:\\native", platform: "win32", isFile }));
    expect(e).toMatchObject({ code: "PROVIDER_UNAVAILABLE", fix: "install the native build", details: { harness: "claudeCode", reason: "npmShim" } });
    expect(resolveClaudePath({ pathEnv: "C:\\native;C:\\npm", platform: "win32", isFile })).toBe("C:\\native\\claude.exe");
    for (const setting of ["C:\\npm\\claude.cmd", "C:\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js", "C:\\npm\\claude.ps1", "C:\\npm\\claude"]) {
      expect(thrown(() => resolveClaudePath({ setting, platform: "win32", isFile: () => true })).fix, setting).toBe("install the native build");
    }
    expect(resolveClaudePath({ setting: "C:\\native\\claude.exe", platform: "win32", isFile })).toBe("C:\\native\\claude.exe");
  });
});

describe("the sign-in commands", () => {
  test("signin_prints_and_runs_nothing", async () => {
    const data = join(tmp, "server data");
    mkdirSync(data, { mode: 0o700 });
    const { dir, marker } = WIN ? { dir: join(tmp, "bin"), marker: join(tmp, "bin", "ran") } : installMarker("it's bin");
    const path = join(dir, WIN ? "claude.exe" : "claude");
    if (WIN) {
      mkdirSync(dir);
      writeFileSync(path, "");
    }
    writeFileSync(join(data, "settings.json"), JSON.stringify({ agents: { claudeCode: { path } } }));
    const lines: string[] = [];
    const errs: string[] = [];
    const output = { out: (s: string) => void lines.push(s), err: (s: string) => void errs.push(s) };
    expect(await runConsole(["provider", "signin", "claudeCode", "--server-data", data], { output }), errs.join("\n")).toBe(0);
    const text = lines.join("\n");
    const home = join(data, "providers", "claudeCode", "home");
    const store = join(home, "store");
    // Nothing ran, nothing was made, and no server was needed.
    expect(existsSync(marker)).toBe(false);
    expect(existsSync(join(data, "providers"))).toBe(false);
    if (!WIN) {
      const q = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`;
      expect(lines).toEqual([
        "# Run in sh, bash or zsh on this server, as the account that runs finisherd.",
        "# Claude Code is off on this server: sessions start on it once agents.claudeCode.enabled is true in settings.json.",
        "# The CLI is the one agents.claudeCode.path names.",
        "(",
        "  umask 077",
        `  mkdir -p ${q(store)}`,
        `  chmod 700 ${q(store)}`,
        `  env ${CRED.map((v) => `-u ${v}`).join(" ")} CLAUDE_CONFIG_DIR=${q(home)} CLAUDE_SECURESTORAGE_CONFIG_DIR=${q(store)} DISABLE_AUTOUPDATER=1 ${q(path)} auth login`,
        ")",
        "# Or run /login inside an interactive claude started with the same variables.",
      ]);
      // Sign-out: the same environment, `auth logout`, no folder made.
      lines.length = 0;
      expect(await runConsole(["provider", "signout", "claudeCode", "--server-data", data], { output })).toBe(0);
      expect(lines.join("\n")).toContain(`DISABLE_AUTOUPDATER=1 ${q(path)} auth logout`);
      expect(lines.join("\n")).not.toContain("mkdir");
      expect(existsSync(marker)).toBe(false);
    }
    // No host or user name beyond the server's own paths.
    const rest = text.replaceAll(store, "").replaceAll(home, "").replaceAll(path, "").replaceAll(tmp, "");
    for (const name of [hostname(), userInfo().username]) if (name.length > 2) expect(rest, name).not.toContain(name);
    // Windows: the PowerShell form, quoted for it.
    const ps = signinCommands("login", { path: "C:\\Users\\O'Neil\\.local\\bin\\claude.exe", serverData: "C:\\Fin Data", platform: "win32" });
    expect(ps).toEqual([
      "# Run in a new PowerShell window on this server, as the account that runs finisherd (it changes that window's environment).",
      "New-Item -ItemType Directory -Force -Path 'C:\\Fin Data\\providers\\claudeCode\\home\\store' | Out-Null",
      `Remove-Item ${CRED.map((v) => `Env:${v}`).join(", ")} -ErrorAction SilentlyContinue`,
      "$env:CLAUDE_CONFIG_DIR = 'C:\\Fin Data\\providers\\claudeCode\\home'",
      "$env:CLAUDE_SECURESTORAGE_CONFIG_DIR = 'C:\\Fin Data\\providers\\claudeCode\\home\\store'",
      "$env:DISABLE_AUTOUPDATER = '1'",
      "& 'C:\\Users\\O''Neil\\.local\\bin\\claude.exe' auth login",
      "# Or run /login inside an interactive claude started with the same variables.",
    ]);
    // PowerShell reads U+2018 to U+201B as quotes too: each is doubled.
    const smart = signinCommands("logout", { path: "C:\\Users\\O\u2019Neil\\claude.exe", serverData: "C:\\Fin Data", platform: "win32" });
    expect(smart).toContain("& 'C:\\Users\\O\u2019\u2019Neil\\claude.exe' auth logout");
    // A control character in a path never reaches the terminal.
    const ctl = signinCommands("logout", { path: "/bin/cla\u001b[2Jude", serverData: "/d", platform: "linux" });
    expect(ctl.join("\n")).not.toContain("\u001b");
    // Only `claudeCode`; a CLI that is not there is the detection's refusal.
    expect(await runConsole(["provider", "signin", "codex", "--server-data", data], { output })).not.toBe(0);
    writeFileSync(join(data, "settings.json"), JSON.stringify({ agents: { claudeCode: { path: join(tmp, "missing", "claude") } } }));
    errs.length = 0;
    expect(await runConsole(["provider", "signin", "claudeCode", "--server-data", data], { output })).not.toBe(0);
    expect(errs.join("\n")).toContain("PROVIDER_UNAVAILABLE");
  });
});
