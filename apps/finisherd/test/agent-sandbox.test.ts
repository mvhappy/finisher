// The session sandbox (plan 7C Task 11; spec §8.5, §8.6, §8.8, §8.9): the session environment
// and its contract, the session's directories, the sign-in store reached in place and never
// opened by `finisherd`, and processes of a CLI session (the scripted `fake` harness's `spawn`
// step) under the real `agent` profile: no `local.key`, no `fin` as `local`, no other project's
// folder, writes only in the workspace. Each denial has an unsandboxed control (the profile not
// ready on this OS: the same processes, started directly). `FIN_REQUIRE_LANDLOCK=1` turns the
// skip on a host without Landlock into a failure.
// Windows (plan 7C Decision 27): the session's processes run in the project's AppContainer
// under the real launcher, and the suite proves what they are given (their working directory,
// `LOCALAPPDATA`, the broker's grants, `fin` over the pipe as the session's agent); what they
// must not reach is proven by configuration (no grant on it, server data owner-only), never
// by an attempt. `FIN_REQUIRE_SANDBOX=1` turns a skip there into a failure.
import fs, { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { join, win32 } from "node:path";
import type { Node, Session } from "@finisher/contracts";
import { aclGrantsForTest, aclRevoke, checkPrivateDir, makePrivateDir, sandboxContainerDeleteForTest, sandboxProjectContainer, sandboxStatus } from "@finisher/fin-node";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, onTestFinished, test } from "vitest";
import type { FakeScript } from "../src/agent/fake.ts";
import { Effect, Exit, Scope } from "effect";
import { assertSessionEnv, openSessionSandbox, REAP_GRACE_MS, sandboxMode, sessionEnv } from "../src/agent/sandbox.ts";
import { sessionKeys } from "../src/agent/sandbox-env.ts";
import { ensureSessionDirs, linkSignInStore, sessionDirs } from "../src/agent/session-dirs.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import { childEnv } from "../src/env.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

// The `fin` the server's sessions are granted (beside the bundled binaries, else the checkout's).
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const require = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const WIN = process.platform === "win32";
const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };

let tmp: string;
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  // The `agent` profile is not in `READY` until Task 16: the test-hooks build adds it.
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "agent-sandbox-"));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const SESSION = { id: "01S0000000000000000000000A", harness: "claudeCode" } as const;
/** A denied read or write: Landlock answers EACCES, Seatbelt EPERM. */
const DENIED = process.platform === "darwin" ? /Operation not permitted/ : /Permission denied/;

describe("the session environment (spec §8.5)", () => {
  const parent = {
    PATH: "/usr/bin:/bin",
    HOME: "/home/u",
    LANG: "C.UTF-8",
    LC_ALL: "C",
    TMPDIR: "/tmp",
    FIN_SERVER_DATA: "/srv/data",
    FIN_AUTHOR: "someone",
    FIN_TRAINING: "1",
    FIN_DIAGNOSTICS: "1",
    SSH_AUTH_SOCK: "/run/user/1/ssh",
    GPG_AGENT_INFO: "x",
    DBUS_SESSION_BUS_ADDRESS: "unix:path=/run/bus",
    DOCKER_HOST: "unix:///var/run/docker.sock",
    DISPLAY: ":0",
    WAYLAND_DISPLAY: "wayland-0",
    CLAUDE_CODE_OAUTH_TOKEN: "t",
    OPENAI_API_KEY: "k",
    ANTHROPIC_API_KEY: "k",
    EDITOR: "vi",
  };
  const make = (over: Partial<Parameters<typeof sessionEnv>[0]> = {}) => {
    const dirs = sessionDirs("/data/agents", SESSION.id, over.platform ?? "linux");
    return sessionEnv({ session: SESSION, server: "tcp:127.0.0.1:4100", projectDir: "/p/P", dirs, store: { grant: "/data/providers/claudeCode/home/store", env: { name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", value: "/data/providers/claudeCode/home/store" } }, parent, platform: "linux", ...over });
  };

  test("session_env_is_exact", () => {
    const env = make();
    expect(env).toEqual({
      PATH: "/usr/bin:/bin",
      HOME: "/home/u",
      LANG: "C.UTF-8",
      LC_ALL: "C",
      FIN_SERVER: "tcp:127.0.0.1:4100",
      FIN_SESSION: SESSION.id,
      FIN_PROJECT_DIR: "/p/P",
      FIN_CREDENTIAL: `/data/agents/${SESSION.id}/credential.json`,
      TMPDIR: `/data/agents/${SESSION.id}/tmp`,
      CLAUDE_CONFIG_DIR: `/data/agents/${SESSION.id}/provider`,
      CLAUDE_SECURESTORAGE_CONFIG_DIR: "/data/providers/claudeCode/home/store",
      GIT_CONFIG_GLOBAL: `/data/agents/${SESSION.id}/git/config`,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_COUNT: "2",
      GIT_CONFIG_KEY_0: "core.fsmonitor",
      GIT_CONFIG_VALUE_0: "false",
      GIT_CONFIG_KEY_1: "credential.helper",
      GIT_CONFIG_VALUE_1: "",
      GNUPGHOME: `/data/agents/${SESSION.id}/tmp/gnupg`,
    });
    expect(() => assertSessionEnv(env, SESSION, { expect: env, platform: "linux" })).not.toThrow();
    // Codex: its own directory variable, no store variable.
    const codex = make({ session: { id: SESSION.id, harness: "codex" }, store: { grant: "/data/providers/codex/home/auth.json" } });
    expect(codex.CODEX_HOME).toBe(`/data/agents/${SESSION.id}/provider`);
    expect(codex.CLAUDE_CONFIG_DIR).toBeUndefined();
    // Windows: the session's temp and LOCALAPPDATA (spike S9 point 7).
    const store = "C:\\data\\providers\\claudeCode\\home\\store";
    const win = make({ platform: "win32", parent: { ...parent, TEMP: "C:\\T", TMP: "C:\\T", LOCALAPPDATA: "C:\\L" }, store: { grant: store, env: { name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", value: store } } });
    expect([win.TEMP, win.TMP, win.LOCALAPPDATA]).toEqual([win.TMPDIR, win.TMPDIR, win32.join(`/data/agents/${SESSION.id}`, "localappdata")]);
    // Spelled for the platform named, whatever the host.
    expect(win.TMPDIR).toBe(`\\data\\agents\\${SESSION.id}\\tmp`);
    expect(() => assertSessionEnv(win, SESSION, { expect: win, platform: "win32" })).not.toThrow();
  });

  test("never_passed_variables_are_dropped_by_child_env_itself", () => {
    const env = childEnv({}, parent, "linux");
    for (const k of ["FIN_SERVER_DATA", "FIN_AUTHOR", "FIN_TRAINING", "FIN_DIAGNOSTICS", "SSH_AUTH_SOCK", "GPG_AGENT_INFO", "DBUS_SESSION_BUS_ADDRESS", "DOCKER_HOST", "DISPLAY", "WAYLAND_DISPLAY", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY"]) {
      expect(env[k], k).toBeUndefined();
    }
  });

  test("assert_session_env_throws_on_each_violation", () => {
    const good = make();
    const reasons: Array<[string, Record<string, string | undefined>, object?]> = [
      ["FIN_PROJECT_DIR", { ...good, FIN_PROJECT_DIR: undefined }],
      ["FIN_PROJECT_DIR", { ...good, FIN_PROJECT_DIR: "p/P" }],
      ["FIN_PROJECT_DIR", { ...good, FIN_PROJECT_DIR: "/p/Other" }, { expect: good }],
      ["FIN_SERVER", { ...good, FIN_SERVER: "/data/run/fin.sock" }, { expect: good }],
      ["FIN_SESSION", { ...good, FIN_SESSION: "01S0000000000000000000000B" }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: "/data/auth/local.key" }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: undefined }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: `/xagents/${SESSION.id}/credential.json` }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: `agents/${SESSION.id}/credential.json` }],
      // The session's own paths, and its git settings, exactly (review of Task 11).
      ["TMPDIR", { ...good, TMPDIR: "/tmp" }],
      ["CLAUDE_CONFIG_DIR", { ...good, CLAUDE_CONFIG_DIR: "/home/u/.claude" }],
      ["CLAUDE_CONFIG_DIR", { ...good, CLAUDE_CONFIG_DIR: undefined }],
      ["CLAUDE_SECURESTORAGE_CONFIG_DIR", { ...good, CLAUDE_SECURESTORAGE_CONFIG_DIR: "/home/u/.claude" }],
      ["GIT_CONFIG_GLOBAL", { ...good, GIT_CONFIG_GLOBAL: "/home/u/.gitconfig" }],
      ["GIT_CONFIG_NOSYSTEM", { ...good, GIT_CONFIG_NOSYSTEM: undefined }],
      ["GIT_CONFIG_COUNT", { ...good, GIT_CONFIG_COUNT: undefined }],
      ["GIT_CONFIG_COUNT", { ...good, GIT_CONFIG_COUNT: "0" }],
      ["GIT_CONFIG_VALUE_0", { ...good, GIT_CONFIG_VALUE_0: "/bin/planted" }],
      ["GIT_CONFIG_KEY_1", { ...good, GIT_CONFIG_KEY_1: "user.name" }],
      ["GNUPGHOME", { ...good, GNUPGHOME: "/home/u/.gnupg" }],
      ["FIN_SERVER", { ...good, FIN_SERVER: undefined }],
      ["TMPDIR", { ...good, TMPDIR: undefined }],
      ["OPENAI_API_KEY", { ...good, OPENAI_API_KEY: "k" }],
      ["CLAUDE_CODE_OAUTH_TOKEN", { ...good, CLAUDE_CODE_OAUTH_TOKEN: "t" }],
      ["EDITOR", { ...good, EDITOR: "vi" }],
      ["FIN_CALL_IDEM", { ...good, FIN_CALL_IDEM: "1" }],
    ];
    const never = ["FIN_SERVER_DATA", "FIN_AUTHOR", "FIN_TRAINING", "FIN_DIAGNOSTICS", "SSH_AUTH_SOCK", "GPG_AGENT_INFO", "DBUS_SESSION_BUS_ADDRESS", "DOCKER_HOST", "DISPLAY", "WAYLAND_DISPLAY", "CLAUDE_CODE_OAUTH_TOKEN", "FIN_CALL_IDEM"];
    for (const k of never) reasons.push([k, { ...good, [k]: "x" }]);
    for (const [name, env, opts] of reasons) {
      let err: { code?: string; details?: { name?: string; reason?: string } } | undefined;
      try {
        assertSessionEnv(env, SESSION, { platform: "linux", ...opts });
      } catch (e) {
        err = e as typeof err;
      }
      expect(err?.code, name).toBe("SANDBOX_ENV");
      expect(err?.details?.name, name).toBe(name);
      // Named as never passed, not merely as unknown (so the list, not the allow-list, holds them).
      if (never.includes(name)) expect(err?.details?.reason, name).toBe("a variable that never passes");
    }
    // `FIN_CALL` (one tool call's id) and a declared extra (a non-secret `CLAUDE_CODE_*` switch) are fine.
    expect(() => assertSessionEnv({ ...good, FIN_CALL: "c", CLAUDE_X: "1", CLAUDE_CODE_DISABLE_X: "1" }, SESSION, { extra: ["CLAUDE_X", "CLAUDE_CODE_DISABLE_X"], platform: "linux" })).not.toThrow();
    // Never a key or a never-passed name, declared or not.
    for (const k of ["OPENAI_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "SSH_AUTH_SOCK"]) {
      expect(() => assertSessionEnv({ ...good, [k]: "x" }, SESSION, { extra: [k], platform: "linux" }), k).toThrow(/the session environment is wrong/);
    }
    expect([...sessionKeys("codex", "linux")]).not.toContain("CLAUDE_SECURESTORAGE_CONFIG_DIR");
  });

  test("git_settings_beat_a_repositorys_own_configuration", async () => {
    // A repository's `.git/config` overrides `GIT_CONFIG_GLOBAL`; `GIT_CONFIG_COUNT` does not.
    const d = await ensureSessionDirs(join(tmp, "agents"), SESSION.id);
    const repo = join(tmp, "repo");
    mkdirSync(repo);
    const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", USERPROFILE: tmp } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: tmp };
    const env = sessionEnv({ session: SESSION, server: "tcp:127.0.0.1:1", projectDir: repo, dirs: d, parent });
    const git = (...a: string[]) => spawnSync("git", ["-C", repo, ...a], { env, encoding: "utf8" });
    expect(git("init", "-q").status).toBe(0);
    expect(git("config", "core.fsmonitor", "/bin/planted").status).toBe(0);
    expect(git("config", "credential.helper", "store").status).toBe(0);
    expect(git("config", "--get", "core.fsmonitor").stdout.trim()).toBe("false");
    expect(git("config", "--get-all", "credential.helper").stdout.split("\n").at(-2)).toBe("");
    expect(git("config", "--get", "commit.gpgsign").stdout.trim()).toBe("false");
  });

  test("the_mode_follows_the_agent_profile", () => {
    expect(sandboxMode({ kind: "enforced", detail: "x" }, "linux")).toBe("launcher");
    expect(sandboxMode({ kind: "disabled", reason: "landlockDisabled" }, "linux")).toBe("launcher");
    expect(sandboxMode({ kind: "disabled", reason: "profileNotReady" }, "linux")).toBe("direct");
    expect(sandboxMode({ kind: "unavailable", need: "kernel ≥ 6.2" }, "linux")).toMatchObject({ code: "SANDBOX_UNAVAILABLE", details: { profile: "agent", os: "linux", need: "kernel ≥ 6.2" } });
  });
});

describe("the session's directories (spec §8.5, §8.6)", () => {
  /** `agents/` inside a private server data, as `finisherd` makes it. */
  const agentsRoot = () => {
    const data = join(tmp, "data");
    mkdirSync(data, { mode: 0o700 });
    if (WIN) makePrivateDir(data);
    return join(data, "agents");
  };

  test("session_dirs_are_private_and_hold_the_git_configuration", async () => {
    const agents = agentsRoot();
    mkdirSync(join(agents, SESSION.id, "tmp"), { recursive: true, mode: 0o755 });
    const d = await ensureSessionDirs(agents, SESSION.id);
    const all = [d.dir, d.settings, d.tmp, d.provider, d.git, d.blobs, d.gnupg, ...(WIN ? [d.localAppData] : [])];
    if (WIN) {
      // No modes: each is a real directory that only this user, SYSTEM and Administrators
      // open (inherited from server data's protected DACL).
      for (const p of [...all, d.gitConfig]) expect(checkPrivateDir(p), p).toMatchObject({ ok: true, problems: [] });
      for (const p of all) expect(lstatSync(p).isDirectory(), p).toBe(true);
      expect(existsSync(d.localAppData)).toBe(true);
    } else {
      for (const p of all) expect(statSync(p).mode & 0o777, p).toBe(0o700);
      expect(statSync(d.gitConfig).mode & 0o777).toBe(0o600);
      // `localappdata/` is Windows' alone.
      expect(existsSync(d.localAppData)).toBe(false);
    }
    const git = readFileSync(d.gitConfig, "utf8");
    expect(git).toMatch(/fsmonitor = false/);
    expect(git).toMatch(/helper =\s*$/m);
  });

  test.runIf(WIN)("a_planted_link_is_never_followed_on_windows", async () => {
    // A container may make a junction (no privilege needed) or a file at `tmp/gnupg`.
    const agents = agentsRoot();
    const d = await ensureSessionDirs(agents, SESSION.id);
    const victim = join(tmp, "victim");
    mkdirSync(victim);
    writeFileSync(join(victim, "keep.txt"), "x");
    rmSync(d.gnupg, { recursive: true });
    symlinkSync(victim, d.gnupg, "junction");
    await ensureSessionDirs(agents, SESSION.id);
    expect(lstatSync(d.gnupg).isSymbolicLink()).toBe(false);
    expect(lstatSync(d.gnupg).isDirectory()).toBe(true);
    expect(fs.readdirSync(d.gnupg)).toEqual([]);
    expect(readFileSync(join(victim, "keep.txt"), "utf8")).toBe("x");
    expect(checkPrivateDir(d.gnupg).ok).toBe(true);
    rmSync(d.gnupg, { recursive: true });
    writeFileSync(d.gnupg, "x");
    await ensureSessionDirs(agents, SESSION.id);
    expect(lstatSync(d.gnupg).isDirectory()).toBe(true);
  });

  test.skipIf(WIN)("a_planted_link_is_never_followed", async () => {
    // `tmp/` is a write root: a sandboxed process may replace `tmp/gnupg` with a link.
    const agents = join(tmp, "agents");
    const d = await ensureSessionDirs(agents, SESSION.id, { platform: "linux" });
    const victim = join(tmp, "victim");
    mkdirSync(victim, { mode: 0o755 });
    chmodSync(victim, 0o755);
    rmSync(d.gnupg, { recursive: true });
    symlinkSync(victim, d.gnupg);
    await ensureSessionDirs(agents, SESSION.id, { platform: "linux" });
    expect(statSync(victim).mode & 0o777).toBe(0o755);
    expect(lstatSync(d.gnupg).isDirectory()).toBe(true);
    expect(statSync(d.gnupg).mode & 0o777).toBe(0o700);
    // A file of that name goes the same way.
    rmSync(d.gnupg, { recursive: true });
    writeFileSync(d.gnupg, "x");
    await ensureSessionDirs(agents, SESSION.id, { platform: "linux" });
    expect(lstatSync(d.gnupg).isDirectory()).toBe(true);
  });

  // Windows without the symlink privilege leaves the CLI signed out (`acl-broker.test.ts`).
  test.skipIf(WIN)("sign_in_store_is_linked_in_place", async () => {
    const data = join(tmp, "data");
    const d = await ensureSessionDirs(join(data, "agents"), SESSION.id, { platform: "linux" });
    const target = join(data, "providers", "codex", "home", "auth.json");
    expect(await linkSignInStore("codex", data, d)).toEqual({ grant: target });
    expect(readlinkSync(join(d.provider, "auth.json"))).toBe(target);
    // A file of that name the CLI left (a rename that replaced the link) becomes the link again.
    rmSync(join(d.provider, "auth.json"));
    writeFileSync(join(d.provider, "auth.json"), "{}");
    await linkSignInStore("codex", data, d);
    expect(lstatSync(join(d.provider, "auth.json")).isSymbolicLink()).toBe(true);
    // Claude Code: a store-only directory named by its variable (spike S9 B7), no link, and
    // nothing made under `providers/` (the user's sign-in makes it).
    const cc = await linkSignInStore("claudeCode", data, d);
    expect(cc).toEqual({ grant: join(data, "providers", "claudeCode", "home", "store"), env: { name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", value: join(data, "providers", "claudeCode", "home", "store") } });
    expect(existsSync(join(data, "providers", "claudeCode"))).toBe(false);
    expect(await linkSignInStore("builtin", data, d)).toBeUndefined();
  });
});

/** The status with the `agent` profile replaced. */
const withAgent = (agent: SandboxStatus["agent"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), agent });
const NOT_READY = withAgent({ kind: "disabled", reason: "profileNotReady" });

/** Whether the real `agent` profile is enforced here; else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
function enforced(name: string): boolean {
  const m = sandboxStatus().agent;
  if (m.kind === "enforced" && existsSync(FIN)) return true;
  expect(require, `${name}: FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)} (fin built: ${existsSync(FIN)})`).toBe(false);
  return false;
}

describe("a CLI session's processes (the fake harness's spawn step)", () => {
  let server: AgentServer | undefined;
  let restore: (() => SandboxStatus) | undefined;
  afterEach(async () => {
    await server?.stop();
    server = undefined;
    if (restore !== undefined) setSandboxStatusSource(restore);
    restore = undefined;
  });

  interface Paths {
    readonly serverData: string;
    readonly key: string;
    readonly dir: string;
    readonly otherDir: string;
  }

  /** A server whose fake session runs `argvs(paths)` in its first turn; answers the children's results. */
  async function run(argvs: (p: Paths) => ReadonlyArray<readonly string[]>, status?: () => SandboxStatus, root = mkdtempSync(join(tmp, "run-")), launcher?: string) {
    if (status !== undefined) restore = setSandboxStatusSource(status);
    const paths: Paths = { serverData: join(root, "server-data"), key: join(root, "server-data", "auth", "local.key"), dir: join(root, "P"), otherDir: join(root, "Q") };
    mkdirSync(paths.serverData, { recursive: true, mode: 0o700 });
    const script: FakeScript = { name: "children", turns: [argvs(paths).map((spawn) => ({ spawn }))] };
    server = new AgentServer(paths.serverData, { timers: false, fakeScripts: [script], ...(launcher === undefined ? {} : { sandboxLauncher: launcher }) });
    await server.start();
    const local = await server.local();
    const projectId = await project(local, paths.dir, "P");
    await project(local, paths.otherDir, "Q");
    if (WIN) {
      // What a launch made outside the temp tree: the static grants (toolchain, `fin`, the
      // project) and the projects' container profiles.
      const data = realpathSyncNative(paths.serverData);
      const containers = [paths.dir, paths.otherDir].map((d) => sandboxProjectContainer(realpathSyncNative(d)));
      onTestFinished(async () => {
        await aclRevoke(data, "static").catch(() => 0);
        for (const c of containers) sandboxContainerDeleteForTest(c);
      });
    }
    const started = await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go" });
    if (!started.ok) return { started, local, paths, children: [] as Child[], nodes: [] as Node[], session: undefined };
    const { session, warnings } = started.value as { session: Session; warnings?: unknown[] };
    const s = await settle(local, session.id);
    const nodes = server.core.store.nodes(s.id, 1, 1000) as Node[];
    const children = nodes.filter((n) => n.kind === "notice" && (n.payload as { child?: unknown }).child !== undefined).map((n) => (n.payload as { child: Child }).child);
    return { started, warnings, local, paths, session: s, nodes, children };
  }

  async function project(local: NdjsonClient, dir: string, name: string): Promise<string> {
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir, name, rate: "30" })).projectId;
    ok(await commit(local, projectId, tx(`01aaaaaaaaaaaaaaaaaaaaaaa${name}`, 0, [addAsset()])));
    return projectId;
  }

  async function settle(c: NdjsonClient, sessionId: string): Promise<Session> {
    for (let i = 0; i < 1500; i++) {
      const { session } = ok<{ session: Session }>(await c.call("agent.session.get", { sessionId }));
      if (session.status === "idle" || session.status === "interrupted" || session.status === "ended") return session;
      await new Promise((r) => setTimeout(r, 20));
    }
    throw new Error("the session never settled");
  }

  const sh = (script: string) => ["/bin/sh", "-c", script] as const;
  /** `echo <w>` on this OS (its output is `<w>` and a line end). */
  const echo = (w: string) => (WIN ? (["cmd.exe", "/d", "/s", "/c", `echo ${w}`] as const) : sh(`echo ${w}`));
  const line = (s: string) => s.replace(/\r?\n$/, "");
  const violations = async () => {
    const audit = ok<{ entries: Array<{ kind: string; outcome: string; details: Record<string, unknown> }> }>(await (await server!.console()).call("auth.audit.list", {}));
    return audit.entries.filter((e) => e.kind === "sandbox.violation");
  };
  const notices = (nodes: Node[], k: string) => nodes.filter((n) => n.kind === "notice" && (n.payload as Record<string, unknown>)[k] !== undefined).map((n) => (n.payload as Record<string, unknown>)[k]);

  test.skipIf(WIN)("agent_child_cannot_read_local_key", async () => {
    if (!enforced("agent_child_cannot_read_local_key")) return;
    const argvs = (p: Paths) => [sh(`cat '${p.key}'`), sh(`env -u FIN_CREDENTIAL -u FIN_SERVER FIN_SERVER_DATA='${p.serverData}' '${FIN}' whoami`)];
    const boxed = await run(argvs);
    expect(boxed.children).toHaveLength(2);
    expect(boxed.children[0]).toMatchObject({ code: 1 });
    expect(boxed.children[0]!.stderr).toMatch(DENIED);
    // `local.key` is unreadable, so `fin` is never `local`: it fails, or (where `fin.sock` is
    // reachable: no namespace and Landlock below ABI 9, the residual of Tasks 4 and 6) it is
    // `anonymous`, which holds only the `public` scope.
    const whoami = boxed.children[1]!;
    expect(whoami.stdout, JSON.stringify(whoami)).not.toMatch(/"kind":"local"/);
    if (whoami.code === 0) expect(whoami.stdout).toMatch(/"kind":"anonymous"/);
    // The profile was enforced: no `SANDBOX_DISABLED`; only, where this host cannot hide Unix
    // sockets, `SANDBOX_SOCKETS_EXPOSED`, as warning and notice alike.
    const codes = ((boxed.warnings ?? []) as Array<{ code: string }>).map((w) => w.code);
    expect(codes.filter((c) => c !== "SANDBOX_SOCKETS_EXPOSED")).toEqual([]);
    expect(notices(boxed.nodes, "sandbox").map((n) => (n as { code: string }).code)).toEqual(codes);
    await server!.stop();
    // Control: the same processes, not sandboxed (the profile not ready), read the key and are `local`.
    const open = await run(argvs, NOT_READY);
    expect(open.children[0]).toMatchObject({ code: 0 });
    expect(open.children[0]!.stdout.length).toBeGreaterThan(0);
    expect(open.children[1]).toMatchObject({ code: 0 });
    expect(open.children[1]!.stdout).toMatch(/"kind":"local"/);
  });

  test.skipIf(WIN)("harness_with_wrong_project_dir_reads_nothing", async () => {
    if (!enforced("harness_with_wrong_project_dir_reads_nothing")) return;
    const argvs = (p: Paths) => [
      // `fin` checks the folder against the session's project (`agent_paths.rs`).
      sh(`echo '[]' > w.json; FIN_PROJECT_DIR='${p.otherDir}' '${FIN}' caption import --track t9 --words w.json --style preset:bold`),
      // The sandbox: another project's folder is not readable at all.
      sh(`cat '${join(p.otherDir, "finisher.project.json")}'`),
      // Writes: the workspace yes, the project folder no.
      sh(`echo hi > ok.txt && cat ok.txt && echo x > '${join(p.dir, "planted.txt")}'`),
      // `fin` over the agent listener with the session's credential.
      sh(`'${FIN}' whoami; echo "$FIN_SERVER"`),
    ];
    const boxed = await run(argvs);
    const [mismatch, read, write, whoami] = boxed.children;
    expect(mismatch!.stdout, JSON.stringify(mismatch)).toMatch(/PATH_NOT_ALLOWED/);
    expect(mismatch!.stdout).toMatch(/projectMismatch/);
    expect(read).toMatchObject({ code: 1 });
    expect(read!.stderr).toMatch(DENIED);
    expect(write!.stdout).toBe("hi\n");
    expect(write!.stderr).toMatch(DENIED);
    expect(existsSync(join(boxed.paths.dir, "planted.txt"))).toBe(false);
    expect(whoami).toMatchObject({ code: 0 });
    expect(whoami!.stdout).toMatch(/"kind":"agent"/);
    // Linux: the agent listener; macOS: `fin.sock` itself, resolved (what the handshake binds).
    if (process.platform === "darwin") expect(whoami!.stdout.trim().split("\n").at(-1)).toBe(realpathSync(join(boxed.paths.serverData, "run", "fin.sock")));
    else expect(whoami!.stdout).toMatch(/^tcp:127\.0\.0\.1:\d+$/m);
    await server!.stop();
    // Control: unsandboxed, the other project's folder is readable and the plant lands.
    const open = await run(argvs, NOT_READY);
    expect(open.children[1]).toMatchObject({ code: 0 });
    expect(open.children[2]!.stderr).toBe("");
    expect(existsSync(join(open.paths.dir, "planted.txt"))).toBe(true);
  });

  test.skipIf(WIN)("sign_in_store_never_opened", async () => {
    // A spy on every call that opens, reads or inspects a path, over the whole session: start,
    // a child's run and the end. The store's link is made with `symlink` and `lstat` of the
    // link's own name in `agents/<id>/provider/`, never under `providers/`.
    const seen: string[] = [];
    const names = ["open", "openSync", "readFile", "readFileSync", "createReadStream", "stat", "statSync", "access", "accessSync", "existsSync", "opendir", "opendirSync", "readdir", "readdirSync", "realpath", "realpathSync", "copyFile", "copyFileSync", "cp", "cpSync", "readlink", "readlinkSync", "lstat", "lstatSync"] as const;
    const fsAny = fs as unknown as Record<string, (...a: unknown[]) => unknown>;
    const fsp = fs.promises as unknown as Record<string, (...a: unknown[]) => unknown>;
    const originals: Array<[Record<string, (...a: unknown[]) => unknown>, string, (...a: unknown[]) => unknown]> = [];
    const watch = (target: Record<string, (...a: unknown[]) => unknown>, name: string) => {
      const f = target[name];
      if (typeof f !== "function") return;
      originals.push([target, name, f]);
      // Keeps `realpathSync.native` and the like.
      target[name] = Object.assign(function (this: unknown, ...a: unknown[]) {
        if (typeof a[0] === "string" && a[0].includes(`${join("server-data", "providers")}`)) seen.push(`${name} ${a[0]}`);
        return f.apply(this, a);
      }, f);
    };
    for (const n of names) {
      watch(fsAny, n);
      watch(fsp, n);
    }
    syncBuiltinESMExports();
    try {
      const r = await run(() => [sh("cat \"$FIN_FAKE_PROVIDER_DIR/sign-in.json\" || true; echo done")], NOT_READY);
      expect(r.children[0]!.stdout).toMatch(/done/);
      const store = join(r.paths.serverData, "agents", r.session!.id, "provider", "sign-in.json");
      expect(readlinkSync(store)).toBe(join(realpathSyncNative(r.paths.serverData), "providers", "fake", "home", "sign-in.json"));
      ok(await r.local.call("agent.session.end", { sessionId: r.session!.id }));
    } finally {
      for (const [t, n, f] of originals) t[n] = f;
      syncBuiltinESMExports();
    }
    expect(seen).toEqual([]);
  });

  test.skipIf(WIN)("a_leaders_group_goes_with_it", async () => {
    // What a process leaves running in its group is killed when it exits.
    const r = await run(() => [sh("sleep 300 & echo $!")], NOT_READY);
    const pid = Number(r.children[0]!.stdout.trim());
    expect(pid).toBeGreaterThan(0);
    await until(() => (alive(pid) ? undefined : true));
  });

  test("disabled_host_runs_with_notice", async () => {
    const r = await run(() => [echo("ran")], withAgent({ kind: "disabled", reason: "landlockDisabled" }));
    expect(line(r.children[0]!.stdout)).toBe("ran");
    expect(r.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent", reason: "landlockDisabled" }) })]);
    expect(notices(r.nodes, "sandbox")).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", profile: "agent", reason: "landlockDisabled" })]);
    const got = ok<{ warnings?: Array<{ code: string; details: { reason: string } }> }>(await r.local.call("agent.session.get", { sessionId: r.session!.id }));
    expect(got.warnings?.map((w) => [w.code, w.details.reason])).toEqual([["SANDBOX_DISABLED", "landlockDisabled"]]);
  });

  test("unavailable_refuses_start", async () => {
    const r = await run(() => [echo("ran")], withAgent({ kind: "unavailable", need: "kernel ≥ 6.2" }));
    expect(r.started.ok).toBe(false);
    expect(!r.started.ok && r.started.error).toMatchObject({ code: "SANDBOX_UNAVAILABLE", details: { profile: "agent", need: "kernel ≥ 6.2" } });
    const sessions = ok<{ sessions: Session[] }>(await r.local.call("agent.session.list", {})).sessions;
    expect(sessions).toEqual([]);
  });

  test("profile_not_ready_keeps_warning", async () => {
    const r = await run(() => [echo("ran")], NOT_READY);
    expect(line(r.children[0]!.stdout)).toBe("ran");
    expect(r.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent", reason: "profileNotReady" }) })]);
    expect(notices(r.nodes, "sandbox")).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", reason: "profileNotReady" })]);
  });

  test.skipIf(WIN)("violation_is_audited", async () => {
    // Only what the launcher itself reports (its failure line on descriptor 3, then exit 78).
    // A launcher that fails to apply, as the real one does when Landlock refuses the policy.
    const failing = join(tmp, "failing-launcher.sh");
    writeFileSync(failing, '#!/bin/sh\ncat <&3 >/dev/null\nline=\'{"code":"SANDBOX_UNAVAILABLE","profile":"agent","need":"x"}\'\necho "$line" >&2\necho "$line" >&3\nexit 78\n', { mode: 0o755 });
    const r = await run(() => [sh("echo never")], withAgent({ kind: "enforced", detail: "stub" }), undefined, failing);
    expect(r.children[0]).toMatchObject({ code: 78 });
    const v = await until(() => notices(server!.core.store.nodes(r.session!.id, 1, 1000) as Node[], "sandboxViolation")[0]);
    expect(v).toEqual({ op: "apply", profile: "agent", rootKind: "none" });
    const audit = await violations();
    expect(audit).toEqual([expect.objectContaining({ outcome: "denied", details: { session: r.session!.id, op: "apply", profile: "agent", rootKind: "none" } })]);
    await server!.stop();
    // Control: a program's own exit 78, and a SIGSYS it sends itself, under the real launcher, are not the launcher's.
    if (!enforced("violation_is_audited")) return;
    const own = await run(() => [sh("exit 78"), sh("kill -SYS $$")]);
    expect(own.children.map((c) => [c.code, c.signal])).toEqual([
      [78, null],
      [null, "SIGSYS"],
    ]);
    expect(await violations()).toEqual([]);
  });

  test.skipIf(WIN)("a_process_that_leaves_its_group_goes_with_the_session", async () => {
    // Task 11's review: a `setsid` grandchild left its group and outlived the session. The
    // Linux and macOS launchers stay the parent and reap it (Decision 3 as amended).
    const escape = sh(`${setsid("echo $$ > pid; exec sleep 300")} >/dev/null 2>&1 < /dev/null & while [ ! -s pid ]; do sleep 0.05; done; cat pid`);
    // Control: without the launcher (the profile not ready on this OS), it outlives the session.
    const direct = await run(() => [escape], NOT_READY);
    const free = Number(direct.children[0]!.stdout.trim());
    try {
      ok(await direct.local.call("agent.session.end", { sessionId: direct.session!.id }));
      await new Promise((res) => setTimeout(res, 200));
      expect(alive(free)).toBe(true);
    } finally {
      if (alive(free)) process.kill(free, "SIGKILL");
    }
    await server!.stop();
    if (restore !== undefined) setSandboxStatusSource(restore);
    restore = undefined;
    if (!enforced("a_process_that_leaves_its_group_goes_with_the_session")) return;
    const r = await run(() => [escape]);
    const pid = Number(r.children[0]!.stdout.trim());
    expect(pid).toBeGreaterThan(0);
    try {
      ok(await r.local.call("agent.session.end", { sessionId: r.session!.id }));
      await until(() => (alive(pid) ? undefined : true));
    } finally {
      if (alive(pid)) process.kill(pid, "SIGKILL");
    }
  });

  test.runIf(WIN)("windows_agent_runs_in_its_container_with_the_sessions_grants", async () => {
    if (!enforced("windows_agent_runs_in_its_container_with_the_sessions_grants")) return;
    const cmd = (c: string) => ["cmd.exe", "/d", "/s", "/c", c] as const;
    const r = await run(() => [cmd("cd"), cmd("echo hi> ok.txt && type ok.txt"), cmd("echo %LOCALAPPDATA%"), cmd("echo %TEMP%"), [FIN, "whoami"], cmd("echo %FIN_SERVER%")]);
    const [data, container] = [realpathSyncNative(r.paths.serverData), sandboxProjectContainer(realpathSyncNative(r.paths.dir))];
    expect(r.started.ok).toBe(true);
    const sid = r.session!.id;
    const workspace = realpathSyncNative(join(r.paths.dir, "workspace", sid));
    const lower = (c: Child | undefined) => line(c?.stdout ?? "").trim().toLowerCase();
    const [cwd, write, lad, temp, whoami, server] = r.children;
    // It starts in its own workspace (granted; Node reads the `package.json` above it).
    expect(lower(cwd), JSON.stringify(cwd)).toBe(workspace.toLowerCase());
    expect(write, JSON.stringify(write)).toMatchObject({ code: 0 });
    expect(line(write!.stdout).trim()).toBe("hi");
    expect(readFileSync(join(workspace, "ok.txt"), "utf8").trim()).toBe("hi");
    // `LOCALAPPDATA` is the session's own: Windows roots a container's under it
    // (`Packages\<container>\AC`) and puts `TEMP` there (S9 point 7).
    const own = `${join(data, "agents", sid, "localappdata").toLowerCase()}\\packages\\${container}\\ac`;
    expect(lower(lad)).toBe(own);
    expect(lower(temp)).toBe(`${own}\\temp`);
    // `fin` reaches the server over the pipe as the session's agent.
    expect(whoami, JSON.stringify(whoami)).toMatchObject({ code: 0 });
    expect(whoami!.stdout).toMatch(/"kind":"agent"/);
    expect(lower(server)).toMatch(/^\\\\\.\\pipe\\/);
    expect(((r.warnings ?? []) as Array<{ code: string }>).map((w) => w.code)).toEqual([]);
    // The broker's grants (plan 7C Task 10): the session's directories to its capability, at
    // the policy's masks; nothing on `blobs/`, on server data or its keys, or on the other
    // project's folder, for any owner. Server data stays owner-only.
    const MODIFY = 0x1301bf;
    const READ = 0x120089;
    // The broker records canonical (`\\?\`) paths.
    const grants = aclGrantsForTest(data).map((g) => ({ ...g, path: g.path.replace(/^\\\\\?\\/, "") }));
    const mine = grants.filter((g) => g.owner === `session:${sid}`);
    const at = (rel: string) => mine.filter((g) => g.path.toLowerCase() === join(data, "agents", sid, rel).toLowerCase()).map((g) => g.mask);
    expect([at("tmp"), at("provider"), at("localappdata"), at("settings"), at("git")]).toEqual([[MODIFY], [MODIFY], [MODIFY], [READ], [READ]]);
    expect(new Set(mine.map((g) => g.sid)).size).toBe(1);
    const reaches = (p: string) => grants.some((g) => p.toLowerCase() === g.path.toLowerCase() || p.toLowerCase().startsWith(`${g.path.toLowerCase()}\\`));
    for (const p of [data, join(data, "auth"), join(data, "auth", "local.key"), join(data, "agents", sid, "blobs"), realpathSyncNative(r.paths.otherDir)]) expect(reaches(p), p).toBe(false);
    expect(checkPrivateDir(data)).toMatchObject({ ok: true });
    // The project folder is read through its view capability, a static grant.
    expect(grants.some((g) => g.owner === "static" && g.path.toLowerCase() === realpathSyncNative(r.paths.dir).toLowerCase() && g.mask === 0x1200a9)).toBe(true);
    // The session's own grants go with it.
    ok(await r.local.call("agent.session.end", { sessionId: sid }));
    await until(() => (aclGrantsForTest(data).some((g) => g.owner === `session:${sid}`) ? undefined : true), 10_000);
  });
});

describe("the sandbox's scope", () => {
  test("closing_the_scope_kills_every_process_it_started", async () => {
    const data = join(tmp, "data");
    const dir = join(tmp, "P");
    mkdirSync(dir, { recursive: true });
    mkdirSync(data, { recursive: true });
    const s = { id: SESSION.id, projectId: "01P0000000000000000000000A", harness: "fake", mode: "draft" } as unknown as Session;
    const scope = await Effect.runPromise(Scope.make());
    const box = await Effect.runPromise(Scope.provide(scope)(openSessionSandbox(s, "fake", { serverData: data, projectDirOf: () => dir, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000 })));
    expect(box.mode).toBe("direct");
    const child = await Effect.runPromise(box.spawn([process.execPath, "-e", "setTimeout(() => {}, 300000)"], { cwd: box.workspace, stdio: "pipe" }));
    expect(alive(child.pid)).toBe(true);
    // A process starts in the session's workspace or the shared one, at any depth, and nowhere
    // else (on Windows its working directory must be granted; plan 7C Task 9 follow-up).
    const quick = async (cwd: string) => {
      const c = await Effect.runPromise(box.spawn([process.execPath, "-e", "0"], { cwd, stdio: "pipe" }));
      c.stdin.end();
      return (await c.exited).code;
    };
    mkdirSync(join(box.workspace, "sub"));
    expect(await quick(join(box.workspace, "sub"))).toBe(0);
    expect(await quick(join(dir, "workspace", "shared"))).toBe(0);
    // A link in the workspace that leads out of it is resolved, not trusted (a junction on Windows).
    symlinkSync(tmp, join(box.workspace, "out"), "junction");
    for (const cwd of [tmp, dir, join(dir, "workspace"), join(dir, "workspace", `${SESSION.id}x`), join(box.workspace, "out")]) {
      const outside = await Effect.runPromiseExit(box.spawn([process.execPath, "-e", "0"], { cwd, stdio: "pipe" }));
      expect(Exit.isFailure(outside) && JSON.stringify(outside.cause), cwd).toMatch(/cwdOutsideWorkspace/);
    }
    await Effect.runPromise(Scope.close(scope, Exit.void));
    expect(await child.exited).toEqual({ code: null, signal: "SIGKILL" });
    // A refused profile opens nothing.
    const refused = await Effect.runPromiseExit(Effect.scoped(openSessionSandbox(s, "fake", { serverData: data, projectDirOf: () => dir, finPath: "/bin/sh", localEndpoint: "x", status: withAgent({ kind: "unavailable", need: "n" }) })));
    expect(Exit.isFailure(refused) && JSON.stringify(refused.cause)).toMatch(/SANDBOX_UNAVAILABLE/);
  });
});

describe("the hang-up of a Linux or macOS launcher", () => {
  test("a_launcher_that_ignores_the_hang_up_is_killed_after_the_grace", async () => {
    if (process.platform !== "linux" && process.platform !== "darwin") return;
    // A launcher that never reaps: the session end hangs it up, then kills its group.
    const stuck = join(tmp, "stuck-launcher.sh");
    writeFileSync(stuck, "#!/bin/sh\ntrap '' HUP\ncat <&3 >/dev/null\nwhile :; do sleep 0.05; done\n", { mode: 0o755 });
    const data = join(tmp, "data");
    const dir = join(tmp, "P");
    for (const d of [data, dir]) mkdirSync(d, { recursive: true });
    const s = { id: SESSION.id, projectId: "01P0000000000000000000000A", harness: "fake", mode: "draft" } as unknown as Session;
    const scope = await Effect.runPromise(Scope.make());
    const box = await Effect.runPromise(
      Scope.provide(scope)(
        openSessionSandbox(s, "fake", { serverData: data, projectDirOf: () => dir, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), agentListener: async () => "tcp:127.0.0.1:9", status: withAgent({ kind: "enforced", detail: "stub" }), launcher: () => stuck, killGraceMs: 5000 }),
      ),
    );
    expect(box.mode).toBe("launcher");
    const child = await Effect.runPromise(box.spawn(["/bin/true"], { cwd: box.workspace, stdio: "pipe" }));
    await new Promise((r) => setTimeout(r, 200));
    const t0 = Date.now();
    await Effect.runPromise(Scope.close(scope, Exit.void));
    expect(await child.exited).toEqual({ code: null, signal: "SIGKILL" });
    // Not at once (the hang-up came first), and within the grace.
    expect(Date.now() - t0).toBeGreaterThanOrEqual(REAP_GRACE_MS - 100);
    expect(Date.now() - t0).toBeLessThan(REAP_GRACE_MS + 2000);
  });
});

describe("the sandbox under the real launcher", () => {
  test.skipIf(WIN)("extra_read_roots_and_the_scope_kill_hold_under_the_launcher", async () => {
    if (!enforced("extra_read_roots_and_the_scope_kill_hold_under_the_launcher")) return;
    const data = join(tmp, "data");
    const dir = join(tmp, "P");
    const extra = join(tmp, "harness-files");
    for (const d of [data, dir, extra]) mkdirSync(d, { recursive: true });
    writeFileSync(join(extra, "f.txt"), "harness\n");
    const s = { id: SESSION.id, projectId: "01P0000000000000000000000A", harness: "fake", mode: "draft" } as unknown as Session;
    const scope = await Effect.runPromise(Scope.make());
    const box = await Effect.runPromise(
      Scope.provide(scope)(openSessionSandbox(s, "fake", { serverData: data, projectDirOf: () => dir, finPath: FIN, localEndpoint: join(data, "run", "fin.sock"), agentListener: async () => "tcp:127.0.0.1:9", killGraceMs: 2000 })),
    );
    expect(box.mode).toBe("launcher");
    const read = async (extraRead?: string[]) => {
      const c = await Effect.runPromise(box.spawn(["/bin/cat", join(extra, "f.txt")], { cwd: box.workspace, stdio: "pipe", ...(extraRead === undefined ? {} : { extraRead }) }));
      c.stdin.end();
      let out = "";
      let err = "";
      c.stdout.on("data", (b) => (out += String(b)));
      c.stderr.on("data", (b) => (err += String(b)));
      return { exit: await c.exited, out, err };
    };
    // Not a read root: refused; named in `extraRead`: read.
    const refused = await read();
    expect(refused.exit.code).toBe(1);
    expect(refused.err).toMatch(DENIED);
    expect(await read([extra])).toMatchObject({ exit: { code: 0 }, out: "harness\n" });
    // A process under the launcher is killed, its group with it, when the scope closes, and
    // so is a `setsid` grandchild that left the group (the launcher reaps it).
    const long = await Effect.runPromise(
      box.spawn(["/bin/sh", "-c", `${setsid("echo $$ > g.pid; exec sleep 300")} >/dev/null 2>&1 < /dev/null & while [ ! -s g.pid ]; do sleep 0.02; done; cat g.pid; exec sleep 300`], { cwd: box.workspace, stdio: "pipe" }),
    );
    let said = "";
    long.stdout.on("data", (b) => (said += String(b)));
    const escaped = await until(() => (/^\d+\n/.test(said) ? Number(said.trim()) : undefined));
    try {
      expect(alive(escaped)).toBe(true);
      await Effect.runPromise(Scope.close(scope, Exit.void));
      expect(await long.exited).toEqual({ code: null, signal: "SIGKILL" });
      await until(() => (alive(escaped) ? undefined : true));
    } finally {
      if (alive(escaped)) process.kill(escaped, "SIGKILL");
    }
    // And the closed sandbox starts nothing more.
    const after = await Effect.runPromiseExit(box.spawn(["/bin/true"], { cwd: box.workspace, stdio: "pipe" }));
    expect(Exit.isFailure(after)).toBe(true);
  });
});

/** A shell command that runs `/bin/sh -c <script>` in a session of its own (`script` holds no
 * single quote; macOS has no `setsid` command). */
function setsid(script: string): string {
  return process.platform === "darwin" ? `perl -MPOSIX -e 'POSIX::setsid() > 0 or die; exec "/bin/sh", "-c", $ARGV[0]' '${script}'` : `setsid sh -c '${script}'`;
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

const realpathSyncNative = (p: string) => fs.realpathSync.native(p);

interface Child {
  readonly code: number | null;
  readonly signal: string | null;
  readonly stdout: string;
  readonly stderr: string;
}
