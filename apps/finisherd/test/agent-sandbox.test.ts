// The session sandbox (plan 7C Task 11; spec §8.5, §8.6, §8.8, §8.9): the session environment
// and its contract, the session's directories, the sign-in store reached in place and never
// opened by `finisherd`, and processes of a CLI session (the scripted `fake` harness's `spawn`
// step) under the real `agent` profile: no `local.key`, no `fin` as `local`, no other project's
// folder, writes only in the workspace. Each denial has an unsandboxed control (the profile not
// ready on this OS: the same processes, started directly). `FIN_REQUIRE_LANDLOCK=1` turns the
// skip on a host without Landlock into a failure.
import fs, { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Node, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { FakeScript } from "../src/agent/fake.ts";
import { Effect, Exit, Scope } from "effect";
import { assertSessionEnv, openSessionSandbox, sandboxMode, sessionEnv } from "../src/agent/sandbox.ts";
import { sessionKeys } from "../src/agent/sandbox-env.ts";
import { ensureSessionDirs, linkSignInStore, sessionDirs } from "../src/agent/session-dirs.ts";
import { childEnv } from "../src/env.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const FIN = fileURLToPath(new URL("../../../target/debug/fin", import.meta.url));
const require = process.env.FIN_REQUIRE_LANDLOCK === "1";
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
    const dirs = sessionDirs("/data/agents", SESSION.id);
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
    expect(() => assertSessionEnv(env, SESSION, { expect: { projectDir: "/p/P", credential: env.FIN_CREDENTIAL!, server: env.FIN_SERVER! }, platform: "linux" })).not.toThrow();
    // Codex: its own directory variable, no store variable.
    const codex = make({ session: { id: SESSION.id, harness: "codex" }, store: { grant: "/data/providers/codex/home/auth.json" } });
    expect(codex.CODEX_HOME).toBe(`/data/agents/${SESSION.id}/provider`);
    expect(codex.CLAUDE_CONFIG_DIR).toBeUndefined();
    // Windows: the session's temp and LOCALAPPDATA (spike S9 point 7).
    const win = make({ platform: "win32", parent: { ...parent, TEMP: "C:\\T", TMP: "C:\\T", LOCALAPPDATA: "C:\\L" } });
    expect([win.TEMP, win.TMP, win.LOCALAPPDATA]).toEqual([win.TMPDIR, win.TMPDIR, join(`/data/agents/${SESSION.id}`, "localappdata")]);
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
      ["FIN_PROJECT_DIR", { ...good, FIN_PROJECT_DIR: "/p/Other" }, { expect: { projectDir: "/p/P" } }],
      ["FIN_SESSION", { ...good, FIN_SESSION: "01S0000000000000000000000B" }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: "/data/auth/local.key" }],
      ["FIN_CREDENTIAL", { ...good, FIN_CREDENTIAL: undefined }],
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
    // `FIN_CALL` (one tool call's id) and a declared extra are fine.
    expect(() => assertSessionEnv({ ...good, FIN_CALL: "c", CLAUDE_X: "1" }, SESSION, { extra: ["CLAUDE_X"], platform: "linux" })).not.toThrow();
    expect([...sessionKeys("codex", "linux")]).not.toContain("CLAUDE_SECURESTORAGE_CONFIG_DIR");
  });

  test("git_settings_beat_a_repositorys_own_configuration", async () => {
    // A repository's `.git/config` overrides `GIT_CONFIG_GLOBAL`; `GIT_CONFIG_COUNT` does not.
    const d = await ensureSessionDirs(join(tmp, "agents"), SESSION.id, { platform: "linux" });
    const repo = join(tmp, "repo");
    mkdirSync(repo);
    const env = sessionEnv({ session: SESSION, server: "tcp:127.0.0.1:1", projectDir: repo, dirs: d, parent: { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: tmp }, platform: "linux" });
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
  test("session_dirs_are_private_and_hold_the_git_configuration", async () => {
    const agents = join(tmp, "agents");
    mkdirSync(join(agents, SESSION.id, "tmp"), { recursive: true, mode: 0o755 });
    const d = await ensureSessionDirs(agents, SESSION.id, { platform: "linux" });
    for (const p of [d.dir, d.settings, d.tmp, d.provider, d.git, d.blobs, d.gnupg]) expect(statSync(p).mode & 0o777, p).toBe(0o700);
    expect(statSync(d.gitConfig).mode & 0o777).toBe(0o600);
    const git = readFileSync(d.gitConfig, "utf8");
    expect(git).toMatch(/fsmonitor = false/);
    expect(git).toMatch(/helper =\s*$/m);
  });

  test("sign_in_store_is_linked_in_place", async () => {
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
    // Claude Code: a store-only directory named by its variable (spike S9 B7), no link.
    const cc = await linkSignInStore("claudeCode", data, d);
    expect(cc).toEqual({ grant: join(data, "providers", "claudeCode", "home", "store"), env: { name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", value: join(data, "providers", "claudeCode", "home", "store") } });
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
  async function run(argvs: (p: Paths) => ReadonlyArray<readonly string[]>, status?: () => SandboxStatus, root = mkdtempSync(join(tmp, "run-"))) {
    if (status !== undefined) restore = setSandboxStatusSource(status);
    const paths: Paths = { serverData: join(root, "server-data"), key: join(root, "server-data", "auth", "local.key"), dir: join(root, "P"), otherDir: join(root, "Q") };
    mkdirSync(paths.serverData, { recursive: true, mode: 0o700 });
    const script: FakeScript = { name: "children", turns: [argvs(paths).map((spawn) => ({ spawn }))] };
    server = new AgentServer(paths.serverData, { timers: false, fakeScripts: [script] });
    await server.start();
    const local = await server.local();
    const projectId = await project(local, paths.dir, "P");
    await project(local, paths.otherDir, "Q");
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
  const violations = async () => {
    const audit = ok<{ entries: Array<{ kind: string; outcome: string; details: Record<string, unknown> }> }>(await (await server!.console()).call("auth.audit.list", {}));
    return audit.entries.filter((e) => e.kind === "sandbox.violation");
  };
  const notices = (nodes: Node[], k: string) => nodes.filter((n) => n.kind === "notice" && (n.payload as Record<string, unknown>)[k] !== undefined).map((n) => (n.payload as Record<string, unknown>)[k]);

  test("agent_child_cannot_read_local_key", async () => {
    if (!enforced("agent_child_cannot_read_local_key")) return;
    const argvs = (p: Paths) => [sh(`cat '${p.key}'`), sh(`env -u FIN_CREDENTIAL -u FIN_SERVER FIN_SERVER_DATA='${p.serverData}' '${FIN}' whoami`)];
    const boxed = await run(argvs);
    expect(boxed.children).toHaveLength(2);
    expect(boxed.children[0]).toMatchObject({ code: 1 });
    expect(boxed.children[0]!.stderr).toMatch(/Permission denied/);
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

  test("harness_with_wrong_project_dir_reads_nothing", async () => {
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
    expect(mismatch!.stdout).toMatch(/PATH_NOT_ALLOWED/);
    expect(mismatch!.stdout).toMatch(/projectMismatch/);
    expect(read).toMatchObject({ code: 1 });
    expect(read!.stderr).toMatch(/Permission denied/);
    expect(write!.stdout).toBe("hi\n");
    expect(write!.stderr).toMatch(/Permission denied/);
    expect(existsSync(join(boxed.paths.dir, "planted.txt"))).toBe(false);
    expect(whoami).toMatchObject({ code: 0 });
    expect(whoami!.stdout).toMatch(/"kind":"agent"/);
    expect(whoami!.stdout).toMatch(/^tcp:127\.0\.0\.1:\d+$/m);
    await server!.stop();
    // Control: unsandboxed, the other project's folder is readable and the plant lands.
    const open = await run(argvs, NOT_READY);
    expect(open.children[1]).toMatchObject({ code: 0 });
    expect(open.children[2]!.stderr).toBe("");
    expect(existsSync(join(open.paths.dir, "planted.txt"))).toBe(true);
  });

  test("sign_in_store_never_opened", async () => {
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

  test("a_leaders_group_goes_with_it", async () => {
    // What a process leaves running in its group is killed when it exits.
    const r = await run(() => [sh("sleep 300 & echo $!")], NOT_READY);
    const pid = Number(r.children[0]!.stdout.trim());
    expect(pid).toBeGreaterThan(0);
    await until(() => (alive(pid) ? undefined : true));
  });

  test("disabled_host_runs_with_notice", async () => {
    const r = await run(() => [sh("echo ran")], withAgent({ kind: "disabled", reason: "landlockDisabled" }));
    expect(r.children[0]!.stdout).toBe("ran\n");
    expect(r.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent", reason: "landlockDisabled" }) })]);
    expect(notices(r.nodes, "sandbox")).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", profile: "agent", reason: "landlockDisabled" })]);
    const got = ok<{ warnings?: Array<{ code: string; details: { reason: string } }> }>(await r.local.call("agent.session.get", { sessionId: r.session!.id }));
    expect(got.warnings?.map((w) => [w.code, w.details.reason])).toEqual([["SANDBOX_DISABLED", "landlockDisabled"]]);
  });

  test("unavailable_refuses_start", async () => {
    const r = await run(() => [sh("echo ran")], withAgent({ kind: "unavailable", need: "kernel ≥ 6.2" }));
    expect(r.started.ok).toBe(false);
    expect(!r.started.ok && r.started.error).toMatchObject({ code: "SANDBOX_UNAVAILABLE", details: { profile: "agent", need: "kernel ≥ 6.2" } });
    const sessions = ok<{ sessions: Session[] }>(await r.local.call("agent.session.list", {})).sessions;
    expect(sessions).toEqual([]);
  });

  test("profile_not_ready_keeps_warning", async () => {
    const r = await run(() => [sh("echo ran")], NOT_READY);
    expect(r.children[0]!.stdout).toBe("ran\n");
    expect(r.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent", reason: "profileNotReady" }) })]);
    expect(notices(r.nodes, "sandbox")).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", reason: "profileNotReady" })]);
  });

  test("violation_is_audited", async () => {
    // Without the seccomp filter (plan 7C Task 4, deferred) nothing kills with `SIGSYS` by
    // itself: the child sends it to itself, which is what the launcher observes either way.
    if (!enforced("violation_is_audited")) return;
    const r = await run(() => [sh("kill -SYS $$")]);
    expect(r.children[0]).toMatchObject({ signal: "SIGSYS" });
    const v = await until(() => notices(server!.core.store.nodes(r.session!.id, 1, 1000) as Node[], "sandboxViolation")[0]);
    expect(v).toEqual({ op: "seccompKill", profile: "agent", rootKind: "none" });
    const audit = await violations();
    expect(audit).toEqual([expect.objectContaining({ outcome: "denied", details: { session: r.session!.id, op: "seccompKill", profile: "agent", rootKind: "none" } })]);
    await server!.stop();
    // Control: a direct process's signal is not the launcher's observation.
    const open = await run(() => [sh("kill -SYS $$")], NOT_READY);
    expect(open.children[0]).toMatchObject({ signal: "SIGSYS" });
    expect(await violations()).toEqual([]);
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
    const child = await Effect.runPromise(box.spawn(["/bin/sh", "-c", "sleep 300"], { cwd: box.workspace, stdio: "pipe" }));
    expect(alive(child.pid)).toBe(true);
    await Effect.runPromise(Scope.close(scope, Exit.void));
    expect(await child.exited).toEqual({ code: null, signal: "SIGKILL" });
    // A refused profile opens nothing.
    const refused = await Effect.runPromiseExit(Effect.scoped(openSessionSandbox(s, "fake", { serverData: data, projectDirOf: () => dir, finPath: "/bin/sh", localEndpoint: "x", status: withAgent({ kind: "unavailable", need: "n" }) })));
    expect(Exit.isFailure(refused) && JSON.stringify(refused.cause)).toMatch(/SANDBOX_UNAVAILABLE/);
  });
});

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
