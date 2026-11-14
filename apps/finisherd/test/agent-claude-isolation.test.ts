// Claude Code's isolation (plan 7D Task 14, Decisions 4–7): nothing planted where the CLI would
// load configuration from reaches a session, with the flags and environment alone and, where the
// `agent` profile is enforced (Linux, `FIN_TEST_SANDBOX_READY=agent`, the test-hooks build), by
// the sandbox as well; the per-harness toolchain list's pieces that live in TypeScript (the
// install tree comes from the resolved binary); and the escape cases that cover the rest, which
// run in `crates/fin-sandbox/tests/escape_common.rs` (`pnpm sandbox:suite`).
//
// `fake-claude` loads what a real CLI would from the paths it is given and reports each planted
// canary it read and applied (`observed.canaries`) and every read or listing it attempted, with
// its outcome and whether the flags let it apply (`observed.loads`): Task 0 saw the CLI read the
// working directory's `.claude/settings.json` and `settings.local.json` and list
// `.claude/{agents,commands,output-styles,skills,workflows}` in it and every parent up to `/`
// whatever the flags say, applying none of it.
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { cargoProfileDirs } from "../src/cargo-target.ts";
import { installTreeRead } from "../src/agent/claude/install.ts";
import { instancePlan, sessionUuid, writeLaunchFiles } from "../src/agent/claude/launch.ts";
import { openSessionSandbox, type SessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { sessionDirs } from "../src/agent/session-dirs.ts";
import { resolveSandboxExecPath } from "../src/sandbox/exec.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";
const REQUIRE_SANDBOX = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const FAKE = cargoProfileDirs()
  .map((d) => join(d, "fake-claude"))
  .find(existsSync);
const NO_FAKE = FAKE === undefined && !REQUIRE_FAKE;
const needFake = (): string => {
  if (FAKE === undefined) throw new Error("FIN_REQUIRE_SERVER=1 but fake-claude is missing: run `cargo build -p fin-agent-fakes`");
  return FAKE;
};
const REPO = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const FIXTURE = join(REPO, "crates", "fin-sandbox", "tests", "fixtures", "claude-code-start.json");
const CASES = join(REPO, "crates", "fin-sandbox", "tests", "cases", "agent-claude.json");

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY, home: process.env.HOME };
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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "claude-isolation-")));
  // The sandbox's `home` input (the user's home, whose Claude Code files it denies) is `os.homedir()`.
  process.env.HOME = join(tmp, "home");
});
afterEach(() => {
  if (saved.home === undefined) delete process.env.HOME;
  else process.env.HOME = saved.home;
  rmSync(tmp, { recursive: true, force: true });
});

const canary = (tag: string) => `{"note":"FIN_CANARY_${tag}"}`;
const SID = "01S0000000000000000000000A";
const OTHER = "01S0000000000000000000000B";
const PROJECT = "01P0000000000000000000000A";
const session = { id: SID, projectId: PROJECT, harness: "claudeCode", mode: "draft" } as unknown as Session;
const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });

/** The paths of one planted world: the user's home, the project, server data and the install. */
function world() {
  const home = join(tmp, "home");
  const data = join(tmp, "data");
  const project = join(tmp, "P");
  const workspace = join(project, "workspace", SID);
  const signIn = join(data, "providers", "claudeCode", "home");
  return { home, data, project, workspace, signIn, other: join(data, "agents", OTHER, "provider") };
}
type World = ReturnType<typeof world>;

/** Every place the CLI would load configuration from, one canary each. `protectedFiles` are the
 * ones the `agent` profile denies; the workspace's are the agent's own to write. */
function plant(w: World): { protectedFiles: Record<string, string>; workspaceFiles: Record<string, string> } {
  const put = (path: string, tag: string) => {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, canary(tag));
    return path;
  };
  const protectedFiles = {
    project_memory: put(join(w.project, "CLAUDE.md"), "project_memory"),
    project_local_memory: put(join(w.project, "CLAUDE.local.md"), "project_local_memory"),
    project_settings: put(join(w.project, ".claude", "settings.json"), "project_settings"),
    // A parent's `.claude/` the CLI lists (Task 0): the project's skill.
    project_skill: put(join(w.project, ".claude", "skills", "x", "SKILL.md"), "project_skill"),
    user_settings: put(join(w.home, ".claude", "settings.json"), "user_settings"),
    user_state: put(join(w.home, ".claude.json"), "user_state"),
    other_session_settings: put(join(w.other, "settings.json"), "other_session_settings"),
    signin_profile: put(join(w.signIn, ".claude.json"), "signin_profile"),
    signin_settings: put(join(w.signIn, "settings.json"), "signin_settings"),
    signin_memory: put(join(w.signIn, "CLAUDE.md"), "signin_memory"),
    signin_dot_claude: put(join(w.signIn, ".claude", "settings.json"), "signin_dot_claude"),
  };
  const workspaceFiles = {
    workspace_memory: put(join(w.workspace, "CLAUDE.md"), "workspace_memory"),
    workspace_agents: put(join(w.workspace, "AGENTS.md"), "workspace_agents"),
    workspace_settings: put(join(w.workspace, ".claude", "settings.json"), "workspace_settings"),
    workspace_local_settings: put(join(w.workspace, ".claude", "settings.local.json"), "workspace_local_settings"),
    workspace_mcp: put(join(w.workspace, ".mcp.json"), "workspace_mcp"),
    project_agents: put(join(w.project, "AGENTS.md"), "project_agents"),
  };
  // The store holds a credential, never a canary.
  mkdirSync(join(w.signIn, "store"), { recursive: true, mode: 0o700 });
  writeFileSync(join(w.signIn, "store", ".credentials.json"), "{}");
  return { protectedFiles, workspaceFiles };
}

/** The CLI's install as Task 0 recorded it: one file in `versions/`, `bin/claude` a link to it,
 * and the fake's sidecar beside the file (it names where the fake records). */
function installFake(w: World, fx: { install: { link: string; target: string } }): { link: string; real: string } {
  const place = (p: string) => p.replace("<home>", w.home);
  const link = place(fx.install.link);
  const target = place(fx.install.target);
  mkdirSync(dirname(target), { recursive: true });
  mkdirSync(dirname(link), { recursive: true });
  copyFileSync(needFake(), target);
  chmodSync(target, 0o755);
  writeFileSync(join(dirname(target), "fake-claude.json"), JSON.stringify({ observed: "@config/observed.json" }));
  symlinkSync(target, link);
  return { link, real: realpathSync(link) };
}

const fixture = () => JSON.parse(readFileSync(FIXTURE, "utf8")) as { install: { link: string; target: string }; deniedReads: string[]; tolerated: Record<string, string> };

function deps(w: World, over: Partial<SessionSandboxDeps> = {}): SessionSandboxDeps {
  mkdirSync(w.data, { recursive: true, mode: 0o700 });
  mkdirSync(w.project, { recursive: true });
  const parent = { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: w.home, LANG: "C.UTF-8" };
  return {
    serverData: w.data,
    projectDirOf: () => w.project,
    finPath: "/bin/sh",
    localEndpoint: join(w.data, "run", "fin.sock"),
    // A loopback port the policy allows; nothing listens (the fake never connects).
    agentListener: async () => "tcp:127.0.0.1:1",
    status: NOT_READY,
    killGraceMs: 2000,
    parent,
    hostRoot: join(tmp, "hosts"),
    ...over,
  };
}

async function within<A>(open: Effect.Effect<SessionSandbox, unknown, Scope.Scope>, f: (box: SessionSandbox) => Promise<A>): Promise<A> {
  const scope = await Effect.runPromise(Scope.make());
  try {
    return await f(await Effect.runPromise(Scope.provide(scope)(open) as Effect.Effect<SessionSandbox>));
  } finally {
    await Effect.runPromise(Scope.close(scope, Exit.void));
  }
}

async function run(box: SessionSandbox, argv: readonly string[], env?: Record<string, string>, extraRead?: readonly string[]) {
  const child = await Effect.runPromise(box.spawn(argv, { cwd: box.workspace, stdio: "pipe", ...(env === undefined ? {} : { env }), ...(extraRead === undefined ? {} : { extraRead }) }));
  child.stdin.end();
  let stdout = "";
  child.stdout.on("data", (b) => (stdout += String(b)));
  child.stderr.on("data", () => undefined);
  const exit = await child.exited;
  return { code: exit.code, stdout };
}

type Status = "ok" | "absent" | { unreadable: string };
interface Observed {
  canaries: { source: string; where: string; file: string; canary: string }[];
  loads: { op: "read" | "list"; source: string; where: string; file: string; status: Status; applies: boolean }[];
  /** The launch's exit code (set by `launch`; the record itself shows the start ran). */
  code: number | null;
}

/** Starts the installed fake the way the adapter does (`instancePlan`'s flags and environment,
 * the install's directory as `extraRead`), or `bare`: no isolation flags, no switches. */
async function launch(box: SessionSandbox, real: string, how: "finisher" | "bare"): Promise<Observed> {
  const dirs = sessionDirs(join(box.env.TMPDIR!, "..", ".."), SID);
  const plan = instancePlan({ binary: real, dirs, os: process.platform, uuid: sessionUuid(SID), resume: false, tools: ["Read"], shell: false, probe: { permissionPath: "stdio" }, model: "default", system: "role", retentionDays: 30, budgetUsd: null });
  await writeLaunchFiles(plan);
  const bare = how === "bare";
  const argv = bare ? [real, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"] : plan.argv;
  const r = await run(box, argv, bare ? {} : plan.env, installTreeRead(real));
  const file = join(box.env.CLAUDE_CONFIG_DIR!, "observed.json");
  const records = JSON.parse(readFileSync(file, "utf8")) as Observed[];
  rmSync(file);
  return { ...records.at(-1)!, code: r.code };
}

const tags = (o: Observed) => o.canaries.map((c) => c.canary.replace("FIN_CANARY_", "")).sort();
const status = (o: Observed, where: string, file: string) => o.loads.find((l) => l.where === where && l.file === file)?.status;
const applying = (o: Observed) => o.loads.filter((l) => l.applies);
const attempt = (o: Observed, op: "read" | "list", where: string, file: string) => {
  const l = o.loads.find((x) => x.op === op && x.where === where && x.file === file);
  return l === undefined ? undefined : { status: l.status, applies: l.applies };
};
const LISTED = ["agents", "commands", "output-styles", "skills", "workflows"];

/** Task 0's attempts under Finisher's flags: the working directory's two settings files read and
 * not applied, `.claude/<listed>` listed (not applied) in it and every parent up to `/`; nothing
 * else attempted but what applies. `cwdStatus` is what the reads see. */
function expectTask0Attempts(o: Observed, workspace: string, cwdStatus: Status) {
  const msg = JSON.stringify(o.loads);
  expect(attempt(o, "read", "cwd", ".claude/settings.json"), msg).toEqual({ status: cwdStatus, applies: false });
  expect(attempt(o, "read", "cwd", ".claude/settings.local.json"), msg).toEqual({ status: cwdStatus, applies: false });
  let depth = 0;
  for (let d = workspace; ; d = dirname(d)) {
    for (const sub of LISTED) expect(attempt(o, "list", depth === 0 ? "cwd" : `cwd+${depth}`, `.claude/${sub}`)?.applies, `${d} ${sub}`).toBe(false);
    depth += 1;
    if (dirname(d) === d) break;
  }
  expect(o.loads.filter((l) => !l.applies)).toHaveLength(2 + depth * LISTED.length);
}

describe.skipIf(WIN)("a Claude Code session loads nothing planted", () => {
  test.skipIf(NO_FAKE)("planted_files_never_load", async () => {
    const w = world();
    const fx = fixture();
    const { protectedFiles, workspaceFiles } = plant(w);
    const { real } = installFake(w, fx);

    // Without a sandbox (the profile not ready): the flags and environment alone.
    const d = deps(w);
    await within(openSessionSandbox(session, "claudeCode", d), async (box) => {
      expect(box.mode).toBe("direct");
      // Control: a bare CLI reads the project's files. Its working directory is the workspace.
      const bare = await launch(box, real, "bare");
      expect(tags(bare)).toEqual(expect.arrayContaining(["workspace_memory", "workspace_agents", "workspace_settings", "workspace_local_settings", "workspace_mcp", "project_memory", "project_local_memory", "project_agents"]));
      // Finisher's launch: none of the planted canaries, anywhere.
      const finisher = await launch(box, real, "finisher");
      expect(tags(finisher), JSON.stringify(finisher.loads)).toEqual([]);
      // What applies is its own configuration directory and the store, and nothing else: not
      // the user's home, the project, a parent of the workspace or the rest of the sign-in home.
      expect(new Set(applying(finisher).map((l) => l.where))).toEqual(new Set(["config", "store"]));
      expect(status(finisher, "store", ".credentials.json")).toBe("ok");
      // As Task 0 recorded: the workspace's planted settings are read (not applied) and every
      // parent's `.claude/` listed, the project's skills included, nothing of it applied.
      expectTask0Attempts(finisher, w.workspace, "ok");
      expect(attempt(finisher, "list", "cwd+2", ".claude/skills")).toEqual({ status: "ok", applies: false });
      expect(finisher.code).toBe(0);
    });

    // Under the launcher, where the profile is enforced here: every file the profile protects
    // is unreadable as well, and a bare CLI cannot load them either.
    const m = sandboxStatus().agent;
    if (m.kind !== "enforced" || !existsSync(resolveSandboxExecPath())) {
      expect(REQUIRE_SANDBOX, `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
      return;
    }
    const { status: _s, launcher: _l, ...real_ } = deps(w);
    await within(openSessionSandbox(session, "claudeCode", real_), async (box) => {
      expect(box.mode).toBe("launcher");
      const finisher = await launch(box, real, "finisher");
      expect(tags(finisher), JSON.stringify(finisher.loads)).toEqual([]);
      expect(status(finisher, "store", ".credentials.json")).toBe("ok");
      // The same attempts, now refused where the profile denies them, and the CLI still starts:
      // the project's `.claude/` (a parent's listing) is unreadable. The workspace's own two
      // settings files are the agent's (it may write them), so they read; `--setting-sources
      // user` is what keeps them from applying.
      expectTask0Attempts(finisher, w.workspace, "ok");
      expect(attempt(finisher, "list", "cwd+2", ".claude/skills")).toEqual({ status: { unreadable: "PermissionDenied" }, applies: false });
      expect(finisher.code, JSON.stringify(finisher.loads)).toBe(0);
      // Every protected planted file is unreadable to a process of the session ...
      for (const [name, path] of Object.entries(protectedFiles)) {
        const r = await run(box, ["/bin/cat", path]);
        expect(r.code, name).not.toBe(0);
        expect(r.stdout, name).toBe("");
      }
      // ... the workspace's own files are the agent's (it may write them), readable; the store too.
      for (const path of Object.values(workspaceFiles).filter((p) => p.startsWith(w.workspace))) expect((await run(box, ["/bin/cat", path])).code, path).toBe(0);
      expect((await run(box, ["/bin/cat", join(w.signIn, "store", ".credentials.json")])).code).toBe(0);
      // A bare CLI, no flags and no switches: the sandbox alone. What the project and the user
      // planted is refused; the workspace's files load (nothing but the switches guards them),
      // and so does the project's `AGENTS.md`, which the profile does not deny (Finisher's launch
      // keeps it out with `CLAUDE_CODE_DISABLE_CLAUDE_MDS`).
      const bare = await launch(box, real, "bare");
      const loaded = tags(bare);
      for (const t of Object.keys(protectedFiles)) expect(loaded, t).not.toContain(t);
      expect(loaded).toEqual(expect.arrayContaining(["workspace_memory", "workspace_settings", "project_agents"]));
      expect(status(bare, "cwd+2", "CLAUDE.md")).toEqual({ unreadable: "PermissionDenied" });
      expect(status(bare, "cwd+2", "CLAUDE.local.md")).toEqual({ unreadable: "PermissionDenied" });
      expect(status(bare, "cwd+2", ".claude/settings.json")).toEqual({ unreadable: "PermissionDenied" });
    });
  });

  test.skipIf(NO_FAKE || process.platform !== "linux")("the_harness_list_reaches_the_session_and_no_other", async () => {
    // `/proc/stat` is in Claude Code's own list (Task 0), not in the base list: a Claude Code
    // session reads it, a Codex session does not (the profile loads the list by harness).
    const m = sandboxStatus().agent;
    if (m.kind !== "enforced" || !existsSync(resolveSandboxExecPath())) {
      expect(REQUIRE_SANDBOX, `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
      return;
    }
    const w = world();
    const { status: _s, launcher: _l, ...real } = deps(w);
    mkdirSync(join(w.data, "providers", "codex", "home"), { recursive: true, mode: 0o700 });
    await within(openSessionSandbox(session, "claudeCode", real), async (box) => {
      for (const p of ["/proc/stat", "/proc/version", "/sys/devices/system/cpu/online"]) expect((await run(box, ["/bin/cat", p])).code, p).toBe(0);
      // Not /proc as a whole (other processes' files), nor this process's own link.
      expect((await run(box, ["/bin/ls", "/proc"])).code).not.toBe(0);
    });
    await within(openSessionSandbox({ ...session, harness: "codex" } as Session, "codex", real), async (box) => {
      expect((await run(box, ["/bin/cat", "/proc/stat"])).code).not.toBe(0);
    });
  });
});

describe("the install tree and the list", () => {
  test("toolchain_list_is_minimal", () => {
    // The drift test against the install tree's layout (a fixture of Task 0's, not the real
    // install): one file under `versions/`, the link beside other tools. The directory granted is
    // the resolved binary's, so a newer version (a new file beside it) is read without a change.
    const w = world();
    const fx = fixture();
    const place = (p: string) => p.replace("<home>", w.home);
    const target = place(fx.install.target);
    const link = place(fx.install.link);
    mkdirSync(dirname(target), { recursive: true });
    mkdirSync(dirname(link), { recursive: true });
    writeFileSync(target, "ELF");
    if (WIN) return; // the link needs a privilege; the layout test is the Unix one
    symlinkSync(target, link);
    const real = realpathSync(link);
    expect(real).toBe(target);
    expect(installTreeRead(real)).toEqual([dirname(target)]);
    expect(installTreeRead(real)).not.toContain(dirname(link));
    // The one place a path of the install could hide: the list. It names none (Rust test
    // `the_list_never_names_the_install_tree` holds the data; this holds the wiring's output).
    expect(installTreeRead("C:\\claude\\versions\\2.1.321.exe", "win32")).toEqual(["C:\\claude\\versions"]);
  });

  test("the_escape_cases_cover_the_matrix", () => {
    // Task 14's list of escape cases, each present as a sandbox-suite case that is an observed
    // denial (or, for the store's own files, an allowed access).
    const doc = JSON.parse(readFileSync(CASES, "utf8")) as { cases: { name: string; op: string[]; expect: unknown; harness?: string; profiles: string[] }[] };
    const H = "$T/data/providers/claudeCode/home";
    const denied = (path: string) => doc.cases.some((c) => c.expect === "deny" && c.harness === "claudeCode" && c.profiles.includes("agent") && c.op.includes(path));
    for (const p of ["$T/p/CLAUDE.md", "$T/p/CLAUDE.local.md", "$T/p/.claude/settings.json", "$T/home/.claude/settings.json", "$T/home/.claude.json", "$T/data/agents/s2/provider/settings.json", `${H}/.claude.json`]) {
      expect(denied(p), p).toBe(true);
    }
    // The store's parent is not writable; another session's inbox socket is not connectable.
    expect(doc.cases.some((c) => c.expect === "deny" && c.op[0] === "create" && c.op[1] === `${H}/settings.json`)).toBe(true);
    expect(doc.cases.some((c) => c.expect === "deny" && c.op[0] === "connect-unix" && c.op[1]!.includes("cc-socks"))).toBe(true);
    // The store itself works.
    expect(doc.cases.some((c) => c.expect === "allow" && c.op[0] === "create" && c.op[1]!.startsWith(`${H}/store/`))).toBe(true);
  });
});
