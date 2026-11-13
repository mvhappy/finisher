// The sandbox seams 7D needs (plan 7D Task 3b): a process's own environment on top of the
// session's (`SpawnOptions.env`, held to `assertSessionEnv` with the harness's names), and the
// host sandbox (`openHostSandbox`) that starts a CLI with no session for detection and the
// probe: no project, no credential, no sign-in store, a throwaway directory removed at close,
// and the same mode rules as a session. `sandbox.ts` stays the only module that starts one.
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import type { Harness, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { assertSessionEnv, openHostSandbox, openSessionSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { assertSpawnEnv, CLAUDE_CODE_ENV, harnessExtra, NEVER_IN_SESSION, sessionKeys } from "../src/agent/sandbox-env.ts";
import { ensureHostDirs, hostProjectDir, removeHostDirs, sweepHostSandboxes } from "../src/agent/session-dirs.ts";
import { resolveSandboxExecPath } from "../src/sandbox/exec.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const require = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  // The `agent` profile is not in `READY` until 7C Task 16: the test-hooks build adds it.
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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-seams-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const withAgent = (agent: SandboxStatus["agent"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), agent });
const NOT_READY = withAgent({ kind: "disabled", reason: "profileNotReady" });
const ENFORCED_STUB = withAgent({ kind: "enforced", detail: "stub" });
const SESSION_ID = "01S0000000000000000000000A";
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8" };

function deps(over: Partial<SessionSandboxDeps> = {}): SessionSandboxDeps {
  const data = join(tmp, "data");
  const project = join(tmp, "P");
  mkdirSync(data, { recursive: true, mode: 0o700 });
  mkdirSync(project, { recursive: true });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data, projectDirOf: () => project, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts"), ...over };
}

/** Runs `f` with a sandbox opened in its own scope, which is closed after. */
async function within<A>(open: Effect.Effect<SessionSandbox, unknown, Scope.Scope>, f: (box: SessionSandbox) => Promise<A>): Promise<A> {
  const scope = await Effect.runPromise(Scope.make());
  try {
    const box = await Effect.runPromise(Scope.provide(scope)(open) as Effect.Effect<SessionSandbox>);
    return await f(box);
  } finally {
    await Effect.runPromise(Scope.close(scope, Exit.void));
  }
}

const session = (harness: Harness) => ({ id: SESSION_ID, projectId: "01P0000000000000000000000A", harness, mode: "draft" }) as unknown as Session;

/** Runs `argv` in `box` to its end: its output and exit. */
async function run(box: SessionSandbox, argv: readonly string[], env?: Record<string, string>): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const child = await Effect.runPromise(box.spawn(argv, { cwd: box.workspace, stdio: "pipe", ...(env === undefined ? {} : { env }) }));
  return collect(child);
}

async function collect(child: SandboxedChild) {
  child.stdin.end();
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (b) => (stdout += String(b)));
  child.stderr.on("data", (b) => (stderr += String(b)));
  const exit = await child.exited;
  return { code: exit.code, stdout, stderr };
}

/** The refusal of `box.spawn(argv, {env})`: its code and details. */
async function refusal(box: SessionSandbox, argv: readonly string[], env: Record<string, string>) {
  const exit = await Effect.runPromiseExit(box.spawn(argv, { cwd: box.workspace, stdio: "pipe", env }));
  expect(Exit.isFailure(exit), JSON.stringify(env)).toBe(true);
  const s = JSON.stringify(Exit.isFailure(exit) ? exit.cause : null);
  return { code: /"code":"([A-Z_]+)"/.exec(s)?.[1], name: /"name":"([^"]+)"/.exec(s)?.[1], reason: /"reason":"([^"]+)"/.exec(s)?.[1] };
}

/** Prints the named variables, one `k=v` a line (`-` when unset). */
const printEnv = (...names: string[]) => [process.execPath, "-e", `for (const k of ${JSON.stringify(names)}) console.log(k + "=" + (process.env[k] ?? "-"))`];
const lines = (s: string) => Object.fromEntries(s.trim().split(/\r?\n/).map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]));

describe("a process's own environment (SpawnOptions.env)", () => {
  test("spawn_env_merged_and_checked", async () => {
    // The harness's names: Claude Code's switches (plan 7D Decision 5) and `FIN_CALL`; nothing else.
    expect(harnessExtra("claudeCode")).toEqual([
      "CLAUDE_CODE_TMPDIR",
      "DISABLE_AUTOUPDATER",
      "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
      "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
      "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
      "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
      "BASH_DEFAULT_TIMEOUT_MS",
      "FIN_CALL",
    ]);
    for (const h of ["codex", "fake"] as const) expect(harnessExtra(h)).toEqual(["FIN_CALL"]);
    for (const h of ["claudeCode", "codex", "fake"] as const) {
      for (const k of harnessExtra(h)) {
        expect(NEVER_IN_SESSION, k).not.toContain(k);
        expect(k, k).not.toMatch(/_API_KEY$|TOKEN|GIT_BASH_PATH|ENTRYPOINT/);
      }
    }
    const marker = join(tmp, "started");
    const d = deps();
    await within(openSessionSandbox(session("claudeCode"), "claudeCode", d), async (box) => {
      const tmpDir = box.env.TMPDIR!;
      const add = { CLAUDE_CODE_TMPDIR: tmpDir, DISABLE_AUTOUPDATER: "1", BASH_DEFAULT_TIMEOUT_MS: "120000" };
      const r = await run(box, printEnv("CLAUDE_CODE_TMPDIR", "DISABLE_AUTOUPDATER", "BASH_DEFAULT_TIMEOUT_MS", "FIN_SESSION", "FIN_PROJECT_DIR", "CLAUDE_CONFIG_DIR"), add);
      expect(r.code, r.stderr).toBe(0);
      const seen = lines(r.stdout);
      // Merged over the session's: both are there.
      expect(seen).toMatchObject({ CLAUDE_CODE_TMPDIR: tmpDir, DISABLE_AUTOUPDATER: "1", BASH_DEFAULT_TIMEOUT_MS: "120000", FIN_SESSION: SESSION_ID, FIN_PROJECT_DIR: box.env.FIN_PROJECT_DIR, CLAUDE_CONFIG_DIR: box.env.CLAUDE_CONFIG_DIR });
      // The sandbox's own environment is not changed by a process's.
      expect(box.env.DISABLE_AUTOUPDATER).toBeUndefined();
      // A name outside the harness's list, or a secret, is refused before anything starts.
      const plant = [process.execPath, "-e", `require("fs").writeFileSync(${JSON.stringify(marker)}, "x")`];
      for (const [env, reason] of [
        [{ EDITOR: "vi" }, "a variable outside the harness's"],
        [{ PATH: "/tmp/planted" }, "a variable outside the harness's"],
        [{ CLAUDE_CODE_GIT_BASH_PATH: "C:\\git\\bash.exe" }, "a variable outside the harness's"],
        [{ CLAUDE_CODE_ENTRYPOINT: "sdk-ts" }, "a variable outside the harness's"],
        [{ ANTHROPIC_API_KEY: "k" }, "a secret"],
        [{ OPENAI_API_KEY: "k" }, "a secret"],
        [{ CLAUDE_CODE_OAUTH_TOKEN: "t" }, "a variable that never passes"],
        [{ SSH_AUTH_SOCK: "/run/ssh" }, "a variable that never passes"],
        [{ FIN_CALL_IDEM: "1" }, "a variable that never passes"],
      ] as const) {
        const r = await refusal(box, plant, env);
        expect(r, JSON.stringify(env)).toEqual({ code: "SANDBOX_ENV", name: Object.keys(env)[0], reason });
      }
      expect(existsSync(marker)).toBe(false);
      // Codex has none of Claude Code's names.
      return undefined;
    });
    await within(openSessionSandbox(session("codex"), "codex", deps()), async (box) => {
      expect(await refusal(box, ["/bin/true"], { DISABLE_AUTOUPDATER: "1" })).toEqual({ code: "SANDBOX_ENV", name: "DISABLE_AUTOUPDATER", reason: "a variable outside the harness's" });
    });
    // A secret is refused even when a harness would list it (the contract's own rule).
    const env = { ...(await within(openSessionSandbox(session("claudeCode"), "claudeCode", deps()), async (b) => b.env)) };
    for (const k of ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", ...NEVER_IN_SESSION]) {
      expect(() => assertSessionEnv({ ...env, [k]: "x" }, { id: SESSION_ID, harness: "claudeCode" }, { extra: [...CLAUDE_CODE_ENV, k] }), k).toThrow(/the session environment is wrong/);
    }
  });

  test("spawn_env_cannot_override_session_names", async () => {
    await within(openSessionSandbox(session("claudeCode"), "claudeCode", deps()), async (box) => {
      const names = [...sessionKeys("claudeCode")];
      for (const k of ["FIN_CREDENTIAL", "FIN_SERVER", "FIN_SESSION", "FIN_PROJECT_DIR", "TMPDIR", "CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_COUNT", "GIT_CONFIG_VALUE_0", "GNUPGHOME"]) {
        expect(names, k).toContain(k);
      }
      for (const k of names) {
        // Another value, and even the sandbox's own: a process never names a session variable.
        for (const v of ["/elsewhere", box.env[k]!]) {
          expect(await refusal(box, ["/bin/true"], { [k]: v }), `${k}=${v}`).toEqual({ code: "SANDBOX_ENV", name: k, reason: "a session variable" });
        }
      }
    });
  });

  test("fin_call_per_spawn", async () => {
    for (const harness of ["claudeCode", "fake"] as const) {
      await within(openSessionSandbox(session(harness), harness, deps()), async (box) => {
        expect(box.env.FIN_CALL).toBeUndefined();
        // Each process its own call id; one started without one has none.
        const [a, b, none] = await Promise.all([run(box, printEnv("FIN_CALL"), { FIN_CALL: "01CALLA" }), run(box, printEnv("FIN_CALL"), { FIN_CALL: "01CALLB" }), run(box, printEnv("FIN_CALL"))]);
        expect([a.stdout.trim(), b.stdout.trim(), none.stdout.trim()]).toEqual(["FIN_CALL=01CALLA", "FIN_CALL=01CALLB", "FIN_CALL=-"]);
        expect(box.env.FIN_CALL).toBeUndefined();
      });
    }
  });

  test("spawn_env_values_checked", async () => {
    const flags = ["DISABLE_AUTOUPDATER", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "CLAUDE_CODE_DISABLE_AUTO_MEMORY", "CLAUDE_CODE_DISABLE_CLAUDE_MDS", "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"];
    await within(openSessionSandbox(session("claudeCode"), "claudeCode", deps()), async (box) => {
      const tmpDir = box.env.TMPDIR!;
      // Plan 7D Decision 5's values, and a call id as `fin` takes it (`agent/reach.ts`).
      const good: Record<string, string> = { CLAUDE_CODE_TMPDIR: tmpDir, ...Object.fromEntries(flags.map((k) => [k, "1"])), BASH_DEFAULT_TIMEOUT_MS: "120000", FIN_CALL: "toolu_01AbC.x:2-y" };
      const r = await run(box, printEnv(...Object.keys(good)), good);
      expect(r.code, r.stderr).toBe(0);
      expect(lines(r.stdout)).toEqual(good);
      const bad: Array<readonly [string, string]> = [
        ["CLAUDE_CODE_TMPDIR", "/tmp"],
        ["CLAUDE_CODE_TMPDIR", join(tmpDir, "x")],
        ["CLAUDE_CODE_TMPDIR", `${tmpDir}/`],
        ["CLAUDE_CODE_TMPDIR", ""],
        ...flags.flatMap((k) => ["0", "true", "", " 1", "1\n", "01"].map((v) => [k, v] as const)),
        ...["", "-1", "1e9", "120000ms", " 5", "5 ", "0x10", "1.5"].map((v) => ["BASH_DEFAULT_TIMEOUT_MS", v] as const),
        ...["", "a b", "a/b", "../x", "x".repeat(129), "a\nb", "é"].map((v) => ["FIN_CALL", v] as const),
      ];
      for (const [k, v] of bad) {
        expect(await refusal(box, ["/bin/true"], { ...good, [k]: v }), `${k}=${JSON.stringify(v)}`).toEqual({ code: "SANDBOX_ENV", name: k, reason: "a wrong value" });
        // The contract itself, as an adapter calls it on a launch environment.
        expect(() => assertSessionEnv({ ...box.env, ...good, [k]: v }, { id: SESSION_ID, harness: "claudeCode" }, { extra: CLAUDE_CODE_ENV }), `${k}=${JSON.stringify(v)}`).toThrow(/a wrong value/);
      }
      expect(() => assertSessionEnv({ ...box.env, ...good }, { id: SESSION_ID, harness: "claudeCode" }, { extra: CLAUDE_CODE_ENV, expect: box.env })).not.toThrow();
      // A name in another case is the same variable on Windows: a session name, a name that
      // never passes, or not the harness's exact name.
      for (const [k, reason] of [
        ["fin_session", "a session variable"],
        ["Fin_Credential", "a session variable"],
        ["Claude_Securestorage_Config_Dir", "a session variable"],
        ["claude_config_dir", "a session variable"],
        ["tmpdir", "a session variable"],
        ["Path", "a variable outside the harness's"],
        ["claude_code_oauth_token", "a variable that never passes"],
        ["Ssh_Auth_Sock", "a variable that never passes"],
        ["fin_call_idem", "a variable that never passes"],
        ["anthropic_api_key", "a secret"],
        ["fin_call", "a variable outside the harness's"],
        ["disable_autoupdater", "a variable outside the harness's"],
      ] as const) {
        expect(await refusal(box, ["/bin/true"], { [k]: "1" }), k).toEqual({ code: "SANDBOX_ENV", name: k, reason });
        expect(() => assertSpawnEnv({ [k]: "1" }, { id: SESSION_ID, harness: "claudeCode" }, "win32"), k).toThrow(new RegExp(reason));
      }
      // Empty values are refused (O4), the flags' and the call id's alike.
      for (const k of [...flags, "FIN_CALL", "CLAUDE_CODE_TMPDIR", "BASH_DEFAULT_TIMEOUT_MS"]) {
        expect((await refusal(box, ["/bin/true"], { [k]: "" })).reason, k).toBe("a wrong value");
      }
    });
    // Any harness's call id is checked; a host sandbox's too.
    await within(openSessionSandbox(session("fake"), "fake", deps()), async (box) => {
      expect(await refusal(box, ["/bin/true"], { FIN_CALL: "" })).toEqual({ code: "SANDBOX_ENV", name: "FIN_CALL", reason: "a wrong value" });
    });
    await within(openHostSandbox("probe", "claudeCode", deps()), async (box) => {
      expect(await refusal(box, ["/bin/true"], { CLAUDE_CODE_TMPDIR: "/tmp" })).toEqual({ code: "SANDBOX_ENV", name: "CLAUDE_CODE_TMPDIR", reason: "a wrong value" });
      expect(await refusal(box, ["/bin/true"], { claude_securestorage_config_dir: "/x" })).toEqual({ code: "SANDBOX_ENV", name: "claude_securestorage_config_dir", reason: "a session variable" });
      expect((await run(box, printEnv("CLAUDE_CODE_TMPDIR"), { CLAUDE_CODE_TMPDIR: box.env.TMPDIR! })).stdout.trim()).toBe(`CLAUDE_CODE_TMPDIR=${box.env.TMPDIR}`);
    });
  });
});

/** The host sandboxes' directories in server data, and their workspaces in the host project folder. */
const hostDirs = (d: SessionSandboxDeps) => {
  const workspaces = join(hostProjectDir(d.hostRoot!, d.serverData), "workspace");
  return [
    ...(existsSync(join(d.serverData, "agents")) ? readdirSync(join(d.serverData, "agents")).filter((n) => n.startsWith("host-")) : []),
    ...(existsSync(workspaces) ? readdirSync(workspaces) : []),
  ];
};

describe("the host sandbox (detection and the probe)", () => {
  test("host_sandbox_has_no_credential_no_project_no_store", async () => {
    const d = deps();
    await within(openHostSandbox("probe", "claudeCode", d), async (box) => {
      expect(box.mode).toBe("direct");
      expect(box.warning).toMatchObject({ code: "SANDBOX_DISABLED", details: { profile: "agent", reason: "profileNotReady" } });
      // The environment: no connection to the server, no session, no credential, no project,
      // no sign-in store variable; the provider configuration and temp are the host directory's.
      for (const k of ["FIN_SERVER", "FIN_SESSION", "FIN_CREDENTIAL", "FIN_PROJECT_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"]) expect(box.env[k], k).toBeUndefined();
      const agents = join(d.serverData, "agents");
      const [id] = readdirSync(agents);
      expect(id).toMatch(/^host-[0-9A-Z]{26}$/);
      const dir = join(agents, id!);
      expect(box.env.CLAUDE_CONFIG_DIR).toBe(join(dir, "provider"));
      expect(box.env.TMPDIR).toBe(join(dir, "tmp"));
      expect(box.env.GIT_CONFIG_GLOBAL).toBe(join(dir, "git", "config"));
      // The explicit host variant of the contract: it passes, and each session name fails it.
      const host = { id: id!, harness: "claudeCode" as const };
      expect(() => assertSessionEnv(box.env, host, { host: true, expect: box.env })).not.toThrow();
      expect(() => assertSessionEnv(box.env, host)).toThrow(/a missing variable/);
      for (const k of ["FIN_CREDENTIAL", "FIN_SESSION", "FIN_PROJECT_DIR", "FIN_SERVER", "CLAUDE_SECURESTORAGE_CONFIG_DIR"]) {
        expect(() => assertSessionEnv({ ...box.env, [k]: join(dir, "x") }, host, { host: true }), k).toThrow(/a session variable in a host sandbox/);
      }
      // The directories: no credential, no store link, nothing made under `providers/`.
      expect(existsSync(join(dir, "credential.json"))).toBe(false);
      expect(readdirSync(join(dir, "provider"))).toEqual([]);
      expect(existsSync(join(d.serverData, "providers"))).toBe(false);
      // The workspace is an empty throwaway folder `workspace/<id>` of the server's one host
      // project folder: in no project, outside server data.
      expect(readdirSync(box.workspace)).toEqual([]);
      expect(box.workspace).toBe(join(hostProjectDir(d.hostRoot!, d.serverData), "workspace", id!));
      expect(relative(d.serverData, box.workspace).startsWith("..")).toBe(true);
      expect(box.workspace.startsWith(join(tmp, "P") + sep)).toBe(false);
      // What a process there sees, and that it cannot be handed a session name either.
      const r = await run(box, printEnv("FIN_CREDENTIAL", "FIN_SESSION", "FIN_PROJECT_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "CLAUDE_CONFIG_DIR", "DISABLE_AUTOUPDATER"), { DISABLE_AUTOUPDATER: "1" });
      expect(lines(r.stdout)).toEqual({ FIN_CREDENTIAL: "-", FIN_SESSION: "-", FIN_PROJECT_DIR: "-", CLAUDE_SECURESTORAGE_CONFIG_DIR: "-", CLAUDE_CONFIG_DIR: join(dir, "provider"), DISABLE_AUTOUPDATER: "1" });
      for (const k of ["CLAUDE_SECURESTORAGE_CONFIG_DIR", "FIN_CREDENTIAL", "CLAUDE_CONFIG_DIR"]) {
        expect((await refusal(box, ["/bin/true"], { [k]: "/x" })).code, k).toBe("SANDBOX_ENV");
      }
    });
  });

  test("host_sandbox_has_throwaway_home", async () => {
    // `$HOME/.config/anthropic/*` and the like can sign a CLI in: a host sandbox's `HOME` is its
    // own empty directory `tmp/home`, never the server's. So is a Claude Code session's (7D
    // Decision 5, Task 0 signed in); a Codex session keeps the server's.
    const d = deps();
    await within(openHostSandbox("probe", "claudeCode", d), async (box) => {
      const [id] = readdirSync(join(d.serverData, "agents"));
      const home = join(d.serverData, "agents", id!, "tmp", "home");
      expect(box.env.HOME).toBe(home);
      if (WIN) expect(box.env.USERPROFILE).toBe(home);
      expect(readdirSync(home)).toEqual([]);
      expect(lines((await run(box, printEnv("HOME"))).stdout)).toEqual({ HOME: home });
      const host = { id: id!, harness: "claudeCode" as const };
      expect(() => assertSessionEnv(box.env, host, { host: true, expect: box.env })).not.toThrow();
      expect(() => assertSessionEnv({ ...box.env, HOME: "/home/u" }, host, { host: true })).toThrow(/not the session's own path/);
      const { HOME: _, ...homeless } = box.env;
      expect(() => assertSessionEnv(homeless, host, { host: true })).toThrow(/a missing variable/);
      // A process cannot be handed another.
      expect((await refusal(box, ["/bin/true"], { HOME: "/home/u" })).code).toBe("SANDBOX_ENV");
    });
    await within(openHostSandbox("detect", "claudeCode", d), async (box) => {
      expect(box.env.HOME).toMatch(/[\\/]tmp[\\/]home$/);
    });
    await within(openSessionSandbox(session("claudeCode"), "claudeCode", d), async (box) => {
      const home = join(d.serverData, "agents", SESSION_ID, "tmp", "home");
      expect(box.env.HOME).toBe(home);
      if (WIN) expect(box.env.USERPROFILE).toBe(home);
      else expect(box.env.SHELL).toMatch(/^\/bin\/(ba)?sh$/);
      expect(readdirSync(home)).toEqual([]);
      expect(lines((await run(box, printEnv("HOME"))).stdout)).toEqual({ HOME: home });
    });
    if (!WIN) {
      await within(openSessionSandbox(session("codex"), "codex", d), async (box) => {
        expect(box.env.HOME).toBe("/home/u");
        expect(box.env.SHELL).toBeUndefined();
      });
    }
  });

  test.skipIf(WIN)("host_sandbox_uses_launcher_when_enforced", async () => {
    // A launcher that records its request (descriptor 3) and runs the program: what the host
    // sandbox hands `fin-sandbox-exec`.
    const stub = join(tmp, "launcher.sh");
    const request = join(tmp, "request.json");
    writeFileSync(stub, `#!/bin/sh\ncat <&3 > '${request}'\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    const d = deps({ status: ENFORCED_STUB, launcher: () => stub });
    await within(openHostSandbox("probe", "claudeCode", d), async (box) => {
      expect(box.mode).toBe("launcher");
      expect(box.warning).toBeUndefined();
      expect(await run(box, ["/bin/echo", "ran"])).toMatchObject({ code: 0, stdout: "ran\n" });
      const req = JSON.parse(readFileSync(request, "utf8")) as { profile: string; input: Record<string, unknown> };
      expect(req.profile).toBe("agent");
      const id = String(req.input.sessionId);
      expect(id).toMatch(/^host-/);
      // No sign-in store, no `fin` endpoint (so no listener), no shared workspace, no libraries;
      // the project is the empty throwaway folder whose workspace the process started in.
      expect(req.input.signInStore).toBeUndefined();
      expect(req.input.fin).toBeUndefined();
      expect(req.input.sharedDir).toBeUndefined();
      expect(req.input.libraryRoots).toEqual([]);
      expect(req.input.mode).toBe("plan");
      expect(req.input.sessionDir).toBe(join(d.serverData, "agents", id));
      expect(req.input.workspaceDir).toBe(box.workspace);
      expect(req.input.workspaceDir).toBe(join(String(req.input.projectDir), "workspace", id));
      expect(String(req.input.projectDir).startsWith(d.hostRoot!)).toBe(true);
    });
    // The same mode rules as a session: Landlock off runs the launcher (F2), a profile not
    // ready runs directly with the warning, an unavailable one opens nothing.
    await within(openHostSandbox("detect", "codex", deps({ status: withAgent({ kind: "disabled", reason: "landlockDisabled" }), launcher: () => stub })), async (box) => {
      expect(box.mode).toBe("launcher");
      expect(box.warning).toMatchObject({ code: "SANDBOX_DISABLED", details: { reason: "landlockDisabled" } });
    });
    const u = deps({ status: withAgent({ kind: "unavailable", need: "kernel ≥ 6.2" }) });
    const refused = await Effect.runPromiseExit(Effect.scoped(openHostSandbox("detect", "claudeCode", u)));
    expect(Exit.isFailure(refused) && JSON.stringify(refused.cause)).toMatch(/SANDBOX_UNAVAILABLE/);
    expect(hostDirs(u)).toEqual([]);
    // Under the real launcher, where the `agent` profile is enforced here: the host directory's
    // provider folder is written, server data's other files are not read.
    const m = sandboxStatus().agent;
    if (m.kind !== "enforced" || !existsSync(resolveSandboxExecPath())) {
      expect(require, `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
      return;
    }
    const { status: _stub, ...real } = deps();
    mkdirSync(join(real.serverData, "auth"), { recursive: true });
    writeFileSync(join(real.serverData, "auth", "local.key"), "secret");
    await within(openHostSandbox("probe", "claudeCode", real), async (box) => {
      expect(box.mode).toBe("launcher");
      const write = await run(box, ["/bin/sh", "-c", 'echo hi > "$CLAUDE_CONFIG_DIR/x" && cat "$CLAUDE_CONFIG_DIR/x" && pwd']);
      expect(write.code, write.stderr).toBe(0);
      expect(write.stdout).toBe(`hi\n${box.workspace}\n`);
      const key = await run(box, ["/bin/cat", join(real.serverData, "auth", "local.key")]);
      expect(key.code).not.toBe(0);
      expect(key.stdout).toBe("");
    });
  });

  test("host_dir_removed_at_close", async () => {
    const d = deps();
    const scope = await Effect.runPromise(Scope.make());
    const box = await Effect.runPromise(Scope.provide(scope)(openHostSandbox("detect", "claudeCode", d)));
    // Its own `provider/` (detection's CLI is given the sign-in home instead, Decision 19).
    writeFileSync(join(box.env.TMPDIR!, "..", "provider", "settings.json"), "{}");
    writeFileSync(join(box.workspace, "left.txt"), "x");
    const child = await Effect.runPromise(box.spawn([process.execPath, "-e", "setTimeout(() => {}, 300000)"], { cwd: box.workspace, stdio: "pipe" }));
    expect(hostDirs(d)).toHaveLength(2);
    await Effect.runPromise(Scope.close(scope, Exit.void));
    // Its processes are killed as a session's are, then both folders go.
    expect(await child.exited).toEqual({ code: null, signal: WIN ? "SIGTERM" : "SIGKILL" });
    expect(hostDirs(d)).toEqual([]);
    expect(existsSync(box.workspace)).toBe(false);
    // A closed host sandbox starts nothing more.
    // (`/bin/sh`, which every Unix has, in the workspace, so only the closed guard can refuse it.)
    const closed = await Effect.runPromiseExit(box.spawn(["/bin/sh", "-c", "true"], { cwd: box.workspace, stdio: "pipe" }));
    expect(Exit.isFailure(closed)).toBe(true);
    expect(JSON.stringify(Exit.isFailure(closed) ? closed.cause : null)).toContain('"code":"SESSION_INTERRUPTED"');
    // Two at once never share a directory.
    await within(openHostSandbox("detect", "claudeCode", d), (a) =>
      within(openHostSandbox("probe", "claudeCode", d), async (b) => {
        expect(a.env.CLAUDE_CONFIG_DIR).not.toBe(b.env.CLAUDE_CONFIG_DIR);
        expect(a.workspace).not.toBe(b.workspace);
      }),
    );
    expect(hostDirs(d)).toEqual([]);
  });

  test.skipIf(WIN)("probe_host_sandbox_never_has_store", async () => {
    const stub = join(tmp, "launcher.sh");
    const request = join(tmp, "request.json");
    writeFileSync(stub, `#!/bin/sh\ncat <&3 > '${request}'\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    const d = deps({ status: ENFORCED_STUB, launcher: () => stub });
    for (const harness of ["claudeCode", "fake", "codex"] as const) {
      await within(openHostSandbox("probe", harness, d), async (box) => {
        expect(box.env.CLAUDE_SECURESTORAGE_CONFIG_DIR).toBeUndefined();
        const r = await run(box, printEnv("CLAUDE_SECURESTORAGE_CONFIG_DIR"));
        expect(r.stdout.trim()).toBe("CLAUDE_SECURESTORAGE_CONFIG_DIR=-");
        const req = JSON.parse(readFileSync(request, "utf8")) as { input: Record<string, unknown> };
        expect(req.input.signInStore, harness).toBeUndefined();
        // No link to a store in its provider directory either.
        expect(readdirSync(join(String(req.input.sessionDir), "provider")), harness).toEqual([]);
        const host = { id: String(req.input.sessionId), harness };
        expect(() => assertSessionEnv(box.env, host, { host: true, expect: box.env })).not.toThrow();
      });
    }
    expect(existsSync(join(d.serverData, "providers"))).toBe(false);
  });

  test.skipIf(WIN)("detect_host_sandbox_has_store_only_by_purpose", async () => {
    const stub = join(tmp, "launcher.sh");
    const request = join(tmp, "request.json");
    writeFileSync(stub, `#!/bin/sh\ncat <&3 > '${request}'\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    const d = deps({ status: ENFORCED_STUB, launcher: () => stub });
    const home = (h: string) => join(d.serverData, "providers", h, "home");
    const store = join(home("claudeCode"), "store");
    await within(openHostSandbox("detect", "claudeCode", d), async (box) => {
      // Decision 22: `auth status --json` reads the CLI's own store, so detection names it.
      // Decision 19 (as 7D Task 0 amended it): its configuration directory is the sign-in home,
      // where the account's profile is; the home is granted read only, the store read-write.
      expect(box.env.CLAUDE_SECURESTORAGE_CONFIG_DIR).toBe(store);
      const r = await run(box, printEnv("CLAUDE_SECURESTORAGE_CONFIG_DIR", "CLAUDE_CONFIG_DIR"));
      const req = JSON.parse(readFileSync(request, "utf8")) as { input: Record<string, unknown> };
      const id = String(req.input.sessionId);
      expect(lines(r.stdout)).toEqual({ CLAUDE_SECURESTORAGE_CONFIG_DIR: store, CLAUDE_CONFIG_DIR: home("claudeCode") });
      expect(req.input.signInStore).toBe(store);
      expect(req.input.signInHome).toBe(home("claudeCode"));
      expect(req.input.fin).toBeUndefined();
      // The contract's store switch: with it the variable is required and must be the
      // harness's store; without it the variable is refused.
      const host = { id, harness: "claudeCode" as const };
      const own: Record<string, string | undefined> = { ...box.env, CLAUDE_CONFIG_DIR: join(d.serverData, "agents", id, "provider") };
      expect(() => assertSessionEnv(box.env, host, { host: true, store: true, signInHome: true, expect: box.env })).not.toThrow();
      expect(() => assertSessionEnv(own, host, { host: true, store: true })).not.toThrow();
      expect(() => assertSessionEnv(own, host, { host: true })).toThrow(/a session variable in a host sandbox/);
      const { CLAUDE_SECURESTORAGE_CONFIG_DIR: _s, ...without } = own;
      expect(() => assertSessionEnv(without, host, { host: true, store: true })).toThrow(/a missing variable/);
      expect(() => assertSessionEnv({ ...own, CLAUDE_SECURESTORAGE_CONFIG_DIR: join(home("codex"), "store") }, host, { host: true, store: true })).toThrow(/not the harness's store/);
      expect(() => assertSessionEnv({ ...own, CLAUDE_SECURESTORAGE_CONFIG_DIR: "/elsewhere" }, host, { host: true, store: true })).toThrow(/not the harness's store/);
      // The sign-in home as the configuration directory only where it was asked for, and only
      // the harness's own, with its own store.
      expect(() => assertSessionEnv(box.env, host, { host: true, store: true })).toThrow(/not the session's own path/);
      expect(() => assertSessionEnv(own, host, { host: true, store: true, signInHome: true })).toThrow(/not the harness's sign-in home/);
      expect(() => assertSessionEnv({ ...box.env, CLAUDE_CONFIG_DIR: home("codex") }, host, { host: true, store: true, signInHome: true })).toThrow(/not the harness's sign-in home/);
      expect(() => assertSessionEnv({ ...box.env, CLAUDE_CONFIG_DIR: `${home("claudeCode")}${sep}x${sep}..` }, host, { host: true, store: true, signInHome: true })).toThrow(/not the harness's sign-in home/);
      // Anchored to this server's data (review of Decision 19, O5): another's home is refused itself.
      expect(() => assertSessionEnv({ ...box.env, CLAUDE_CONFIG_DIR: join(tmp, "other", "providers", "claudeCode", "home") }, host, { host: true, store: true, signInHome: true })).toThrow(/not the harness's sign-in home/);
      const otherHome = join(tmp, "other", "providers", "claudeCode", "home");
      expect(() => assertSessionEnv({ ...box.env, CLAUDE_CONFIG_DIR: otherHome, CLAUDE_SECURESTORAGE_CONFIG_DIR: join(otherHome, "store") }, host, { host: true, store: true, signInHome: true })).toThrow(/not the harness's sign-in home/);
      expect(() => assertSessionEnv(box.env, host, { host: true, signInHome: true })).toThrow(/a session variable in a host sandbox|a sign-in home outside/);
      expect(() => assertSessionEnv(box.env, { id: "01JQ0000000000000000000000", harness: "claudeCode" }, { signInHome: true })).toThrow(/SANDBOX_ENV|session environment/);
      // A process cannot name the home either.
      expect(await refusal(box, ["/bin/true"], { CLAUDE_CONFIG_DIR: home("claudeCode") })).toEqual({ code: "SANDBOX_ENV", name: "CLAUDE_CONFIG_DIR", reason: "a session variable" });
      // A process still cannot name the store itself.
      expect(await refusal(box, ["/bin/true"], { CLAUDE_SECURESTORAGE_CONFIG_DIR: store })).toEqual({ code: "SANDBOX_ENV", name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", reason: "a session variable" });
    });
    // A link-store harness: the link in the host directory's provider folder, the store granted.
    await within(openHostSandbox("detect", "fake", d), async (box) => {
      // `/bin/sh`, not `/bin/true`: macOS has only `/usr/bin/true`.
      expect((await run(box, ["/bin/sh", "-c", "true"])).code).toBe(0);
      const req = JSON.parse(readFileSync(request, "utf8")) as { input: Record<string, unknown> };
      const link = join(String(req.input.sessionDir), "provider", "sign-in.json");
      expect(lstatSync(link).isSymbolicLink()).toBe(true);
      expect(req.input.signInStore).toBe(join(home("fake"), "sign-in.json"));
    });
    // Nothing is made or read under `providers/` (spec §8.6).
    expect(existsSync(join(d.serverData, "providers"))).toBe(false);
    // Under the real launcher (the profile validates the store grant).
    const m = sandboxStatus().agent;
    if (m.kind !== "enforced" || !existsSync(resolveSandboxExecPath())) {
      expect(require, `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
      return;
    }
    const { status: _stub, launcher: _l, ...real } = d;
    // A person signed in: the sign-in home holds the profile and the store (made by the test).
    mkdirSync(store, { recursive: true, mode: 0o700 });
    writeFileSync(join(home("claudeCode"), ".claude.json"), '{"oauthAccount":{}}');
    mkdirSync(home("codex"), { recursive: true, mode: 0o700 });
    writeFileSync(join(home("codex"), "auth.json"), "{}");
    await within(openHostSandbox("detect", "claudeCode", real), async (box) => {
      expect(box.mode).toBe("launcher");
      const r = await run(box, ["/bin/sh", "-c", 'echo "$CLAUDE_SECURESTORAGE_CONFIG_DIR"']);
      expect(r.code, r.stderr).toBe(0);
      expect(r.stdout).toBe(`${store}\n`);
      // Decision 19 (review S1): the CLI reads the profile in the home, writes its store, and
      // writes nothing else there (no rewritten profile, no settings or hooks, no lock) ...
      const read = await run(box, ["/bin/sh", "-c", 'cat "$CLAUDE_CONFIG_DIR/.claude.json"']);
      expect(read.code, read.stderr).toBe(0);
      expect(read.stdout).toBe('{"oauthAccount":{}}');
      const stored = await run(box, ["/bin/sh", "-c", 'echo x > "$CLAUDE_SECURESTORAGE_CONFIG_DIR/.credentials.json"']);
      expect(stored.code, stored.stderr).toBe(0);
      for (const write of ['echo x > "$CLAUDE_CONFIG_DIR/.claude.json"', 'echo x > "$CLAUDE_CONFIG_DIR/settings.json"', 'mkdir "$CLAUDE_CONFIG_DIR/.claude.json.lock"', 'mkdir "$CLAUDE_CONFIG_DIR/backups"']) {
        expect((await run(box, ["/bin/sh", "-c", write])).code, write).not.toBe(0);
      }
      expect(readFileSync(join(home("claudeCode"), ".claude.json"), "utf8")).toBe('{"oauthAccount":{}}');
      expect(readdirSync(home("claudeCode")).sort()).toEqual([".claude.json", "store"]);
      // ... and nothing beside it: another harness's home, server data's own files.
      const other = await run(box, ["/bin/sh", "-c", `cat '${join(home("codex"), "auth.json")}'`]);
      expect(other.code).not.toBe(0);
    });
  });

  test.skipIf(WIN)("host_project_folder_stable_and_private", async () => {
    const d = deps();
    const folder = hostProjectDir(d.hostRoot!, d.serverData);
    expect(folder.startsWith(d.hostRoot! + sep)).toBe(true);
    expect(relative(d.serverData, folder).startsWith("..")).toBe(true);
    // One per server: another server's data has another folder.
    expect(hostProjectDir(d.hostRoot!, join(tmp, "data2"))).not.toBe(folder);
    const seen = new Set<string>();
    for (const purpose of ["detect", "probe", "probe"] as const) {
      await within(openHostSandbox(purpose, "claudeCode", d), async (box) => {
        seen.add(box.env.TMPDIR!);
        expect(box.workspace.startsWith(join(folder, "workspace") + sep)).toBe(true);
        expect(statSync(folder).mode & 0o777).toBe(0o700);
      });
    }
    // The same folder each time (one AppContainer profile on Windows), each open its own workspace.
    expect(seen.size).toBe(3);
    expect(readdirSync(folder)).toEqual(["workspace"]);
    expect(readdirSync(join(folder, "workspace"))).toEqual([]);
    // Made private again.
    chmodSync(folder, 0o755);
    await within(openHostSandbox("probe", "claudeCode", d), async () => expect(statSync(folder).mode & 0o777).toBe(0o700));
    // A link or a file planted at its name is refused, never followed; nothing is left behind.
    const elsewhere = join(tmp, "elsewhere");
    mkdirSync(elsewhere);
    rmSync(folder, { recursive: true });
    for (const plant of [() => symlinkSync(elsewhere, folder), () => writeFileSync(folder, "x")]) {
      plant();
      const refused = await Effect.runPromiseExit(Effect.scoped(openHostSandbox("probe", "claudeCode", d)));
      expect(Exit.isFailure(refused) && JSON.stringify(refused.cause)).toMatch(/SANDBOX_UNAVAILABLE/);
      expect(readdirSync(join(d.serverData, "agents")).filter((n) => n.startsWith("host-"))).toEqual([]);
      expect(readdirSync(elsewhere)).toEqual([]);
      rmSync(folder);
    }
  });

  test("host_sandboxes_swept_at_start", async () => {
    const d = deps();
    const agents = join(d.serverData, "agents");
    // What a crash leaves: open host sandboxes, never closed.
    const scope = await Effect.runPromise(Scope.make());
    for (const p of ["detect", "probe"] as const) await Effect.runPromise(Scope.provide(scope)(openHostSandbox(p, "claudeCode", d)));
    mkdirSync(join(agents, SESSION_ID, "tmp"), { recursive: true });
    mkdirSync(join(agents, "host-notanid"));
    const other = hostProjectDir(d.hostRoot!, join(tmp, "data2"));
    mkdirSync(other, { mode: 0o700 });
    const folder = hostProjectDir(d.hostRoot!, d.serverData);
    expect(existsSync(folder)).toBe(true);
    await sweepHostSandboxes(d.serverData, { hostRoot: d.hostRoot! });
    // Every host directory and this server's host folder go; a session's, a name that is not a
    // host id and another server's folder stay.
    expect(readdirSync(agents).sort()).toEqual([SESSION_ID, "host-notanid"].sort());
    expect(existsSync(folder)).toBe(false);
    expect(existsSync(other)).toBe(true);
    await Effect.runPromise(Scope.close(scope, Exit.void));
    // Nothing to sweep is fine; a link at the folder's name goes, its target stays.
    await sweepHostSandboxes(join(tmp, "nothing"), { hostRoot: d.hostRoot! });
    if (!WIN) {
      const elsewhere = join(tmp, "elsewhere");
      mkdirSync(elsewhere);
      writeFileSync(join(elsewhere, "keep"), "x");
      symlinkSync(elsewhere, folder);
      await sweepHostSandboxes(d.serverData, { hostRoot: d.hostRoot! });
      expect(existsSync(folder)).toBe(false);
      expect(readdirSync(elsewhere)).toEqual(["keep"]);
    }
    // The server sweeps after the ACL broker's sweep and before anything listens (no host
    // sandbox is alive then).
    const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
    const at = main.indexOf("sweepHostSandboxes(serverData");
    expect(at).toBeGreaterThan(main.indexOf("sweepAtStart(acl)"));
    expect(at).toBeLessThan(main.indexOf("new RemoteListener("));
    expect(at).toBeLessThan(main.indexOf("makeAgentListener(() =>"));
  });

  test("host_dirs_partial_creation_removed", async () => {
    const agents = join(tmp, "data", "agents");
    const id = "host-01ARZ3NDEKTSV4RRFFQ69G5FAV";
    // A directory where the git configuration file goes: the creation fails after the first folders.
    mkdirSync(join(agents, id, "git", "config"), { recursive: true });
    await expect(ensureHostDirs(agents, { id })).rejects.toThrow();
    expect(existsSync(join(agents, id))).toBe(false);
    const made = await ensureHostDirs(agents);
    expect(made.id).toMatch(/^host-[0-9A-Z]{26}$/);
    expect(made.dirs.dir).toBe(join(agents, made.id));
    await removeHostDirs(agents, made.id);
    expect(existsSync(made.dirs.dir)).toBe(false);
  });

  test("remove_host_dirs_only_host_ids", async () => {
    const agents = join(tmp, "data", "agents");
    mkdirSync(join(agents, SESSION_ID), { recursive: true });
    mkdirSync(join(tmp, "data", "host-01ARZ3NDEKTSV4RRFFQ69G5FAV"), { recursive: true });
    for (const id of [SESSION_ID, "shared", "host-", "host-x", "host-01arz3ndektsv4rrffq69g5fav", "host-01ARZ3NDEKTSV4RRFFQ69G5FAVX", `host-01ARZ3NDEKTSV4RRFFQ69G5FAV/../${SESSION_ID}`, "../host-01ARZ3NDEKTSV4RRFFQ69G5FAV", "host-01ARZ3NDEKTSV4RRFFQ69G5FA/"]) {
      await expect(removeHostDirs(agents, id), id).rejects.toThrow(/not a host sandbox's directory/);
    }
    expect(existsSync(join(agents, SESSION_ID))).toBe(true);
    expect(existsSync(join(tmp, "data", "host-01ARZ3NDEKTSV4RRFFQ69G5FAV"))).toBe(true);
  });
});

describe("the spawn gate (scripts/check-agent-spawn.mjs)", () => {
  test("spawn_gate_unchanged", async () => {
    const repo = fileURLToPath(new URL("../../../", import.meta.url));
    const gate = (await import(new URL("../../../scripts/check-agent-spawn.mjs", import.meta.url).href)) as {
      ALLOWED: Set<string>;
      checked: (p: string) => boolean;
      findings: (p: string, text: string) => string[];
    };
    // `sandbox.ts` stays the only spawner of `agent/` (beside our own `fin`'s two).
    expect([...gate.ALLOWED].sort()).toEqual(["apps/finisherd/src/agent/builtin/environment.ts", "apps/finisherd/src/agent/builtin/fin-tool.ts", "apps/finisherd/src/agent/sandbox.ts"]);
    // Its helpers start nothing, and nothing the Claude Code adapter has does (whatever is there yet).
    const claude = join(repo, "apps/finisherd/src/agent/claude");
    const files = ["apps/finisherd/src/agent/sandbox-input.ts", "apps/finisherd/src/agent/sandbox-env.ts", "apps/finisherd/src/agent/session-dirs.ts", ...(existsSync(claude) ? readdirSync(claude, { recursive: true, encoding: "utf8" }).map((f) => `apps/finisherd/src/agent/claude/${f.split(sep).join("/")}`) : [])].filter((f) => /\.(?:[cm]?[jt]s|tsx)$/.test(f));
    for (const f of files) {
      expect(gate.checked(f), f).toBe(true);
      const text = readFileSync(join(repo, f), "utf8");
      expect(gate.findings(f, text), f).toEqual([]);
      expect(text, f).not.toMatch(/child_process/);
    }
  });
});
