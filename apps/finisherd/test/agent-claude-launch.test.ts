// The Claude Code launch plan (plan 7D Task 4; Decisions 1, 2, 4, 5, 7, 10, 12, 15, 21): the argv
// per OS and mode, the role's allow-set as `--tools`, the session settings file written from a
// typed schema, the role prompt in a file, the money backstop, the extra environment held to
// the session contract, and the session ULID carried losslessly as the CLI's UUID.
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, Session } from "@finisher/contracts";
import { Effect } from "effect";
import fc from "fast-check";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { SessionSpec } from "../src/agent/adapter.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import {
  CLI_TOOLS,
  checkSettings,
  DISALLOWED_TOOLS,
  type LaunchContext,
  launchPlan,
  sessionUlid,
  sessionUuid,
  writeLaunchFiles,
} from "../src/agent/claude/launch.ts";
import type { ProbeResult } from "../src/agent/claude/probe-types.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { assertSessionEnv, sessionEnv } from "../src/agent/sandbox.ts";
import { CLAUDE_CODE_ENV, harnessExtra, NEVER_IN_SESSION } from "../src/agent/sandbox-env.ts";
import { sessionDirs } from "../src/agent/session-dirs.ts";
import { AgentStore } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { makeRuntime, META, PEOPLE, planner } from "./support/runtime-doubles.ts";

type Os = "linux" | "darwin" | "win32";

// Task 0's synthetic id: a ULID's 128 bits as a (non-v4) UUID, accepted by 2.1.321.
const SID = "01JC8Z5Q3M4N6P7R8S9T0VWXYZ";
const UUID = "019311f2-dc74-254d-63e1-194e81be77df";
const EDITOR = ["fin", "workspace_read", "workspace_write", "workspace_list", "Read", "Write", "Grep", "Edit", "Bash", "Glob"];
const ROOT: Record<Os, string> = { linux: "/srv/data/agents", darwin: "/Users/x/data/agents", win32: "C:\\data\\agents" };
const BIN: Record<Os, string> = { linux: "/opt/claude/bin/claude", darwin: "/opt/claude/bin/claude", win32: "C:\\claude\\claude.exe" };
const SETTINGS: Record<Os, string> = {
  linux: `/srv/data/agents/${SID}/settings`,
  darwin: `/Users/x/data/agents/${SID}/settings`,
  win32: `C:\\data\\agents\\${SID}\\settings`,
};
const sep = (os: Os) => (os === "win32" ? "\\" : "/");
const DISALLOWED =
  "Task,Agent,Workflow,SendMessage,Skill,Cron*,ScheduleWakeup,Monitor,EnterWorktree,ExitWorktree,AskUserQuestion,NotebookEdit,ToolSearch," +
  "EnterPlanMode,ExitPlanMode,ListAgents,ReportFindings,TaskStop";

const session = { id: SID, harness: "claudeCode" } as Session;
const PROBE: ProbeResult = { version: "2.1.321", permissionPath: "stdio", tools: [], models: [], capabilities: [] };
const spec = (over: Partial<SessionSpec> = {}): SessionSpec => ({
  sessionId: SID,
  projectId: "P1",
  model: "default",
  role: "editor",
  mode: "draft",
  system: "You are Finisher's editor. ROLE-PROMPT-MARKER",
  cwd: `/p/workspace/${SID}`,
  env: {},
  providerRef: null,
  tools: EDITOR,
  ...over,
});
const ctx = (os: Os, over: Partial<LaunchContext> = {}): LaunchContext => ({
  binary: BIN[os],
  agentsRoot: ROOT[os],
  retentionDays: 90,
  remainingMoneyMicros: null,
  planSignIn: true,
  ...over,
});
const plan = (os: Os = "linux", s: Partial<SessionSpec> = {}, c: Partial<LaunchContext> = {}, p: Partial<ProbeResult> = {}) =>
  launchPlan(session, spec(s), { ...PROBE, ...p }, os, ctx(os, c));
const flag = (argv: readonly string[], name: string): string | undefined => {
  const i = argv.indexOf(name);
  return i < 0 ? undefined : argv[i + 1];
};
const file = (p: ReturnType<typeof plan>, name: string) => p.files.find((f) => f.path.endsWith(name))!;
const caught = (f: () => unknown): FinError => {
  try {
    f();
  } catch (e) {
    return e as FinError;
  }
  throw new Error("expected a failure");
};

function golden(os: Os, tools: string, opts: { restricted?: boolean; prompt?: string; tail?: string[] } = {}): string[] {
  const s = SETTINGS[os] + sep(os);
  return [
    BIN[os],
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--permission-mode",
    "manual",
    "--permission-prompts",
    "host",
    "--permission-prompt-tool",
    opts.prompt ?? "stdio",
    ...(opts.restricted ? ["--restricted"] : ["--setting-sources", "user"]),
    "--settings",
    `${s}claude.json`,
    "--strict-mcp-config",
    "--mcp-config",
    `${s}mcp.json`,
    "--tools",
    tools,
    "--disallowedTools",
    DISALLOWED,
    "--disable-slash-commands",
    "--append-system-prompt-file",
    `${s}role.md`,
    "--model",
    "default",
    ...(opts.tail ?? ["--session-id", UUID]),
  ];
}

describe("Claude Code launch plan", () => {
  test.each(["linux", "darwin"] as const)("argv_matches_decision_1 (%s)", (os) => {
    const p = plan(os);
    expect(p.argv).toEqual(golden(os, "Read,Edit,Write,Glob,Grep,Bash"));
    expect(p.argv.some((a) => a.startsWith("--add-dir"))).toBe(false);
    expect(p.tools).toEqual(["Read", "Edit", "Write", "Glob", "Grep", "Bash"]);
    expect(JSON.parse(file(p, "mcp.json").body)).toEqual({ mcpServers: { finisher: { type: "sdk", name: "finisher" } } });
    expect(p.mcpTools).toEqual([]);
    // The probe's `--restricted` variant replaces `--setting-sources user` and nothing else.
    expect(plan(os, {}, {}, { restricted: true }).argv).toEqual(golden(os, "Read,Edit,Write,Glob,Grep,Bash", { restricted: true }));
  });

  test("argv_matches_decision_1 (win32)", () => {
    // Decision 21: no `Bash` (Git Bash cannot start in an AppContainer); the brokered shell is
    // the `finisher` SDK MCP server's `shell` tool, which the host serves (Task 13).
    const p = plan("win32");
    expect(p.argv).toEqual(golden("win32", "Read,Edit,Write,Glob,Grep"));
    expect(p.argv.some((a) => a.startsWith("--add-dir"))).toBe(false);
    expect(p.tools).not.toContain("Bash");
    expect(JSON.parse(file(p, "mcp.json").body)).toEqual({ mcpServers: { finisher: { type: "sdk", name: "finisher" } } });
    expect(p.mcpTools).toEqual(["mcp__finisher__shell"]);
    expect(p.files.map((f) => f.path)).toEqual(["claude.json", "mcp.json", "role.md"].map((n) => `${SETTINGS.win32}\\${n}`));
    expect(plan("win32", {}, {}, { restricted: true }).argv).toEqual(golden("win32", "Read,Edit,Write,Glob,Grep", { restricted: true }));
    // A role without Bash gets no shell either.
    expect(plan("win32", { tools: ["Read"] }).mcpTools).toEqual([]);
  });

  test("tools_is_role_allow_set_intersection", () => {
    const p = plan("linux", { tools: ["WebSearch", "fin", "Read", "NotARealTool", "workspace_write", "Bash", "WebFetch"] });
    // In the CLI's mapped order, whatever the role's order.
    expect(flag(p.argv, "--tools")).toBe("Read,Bash,WebFetch,WebSearch");
    // `fin` and `workspace_*` are the built-in agent's names (a CLI reaches `fin` through its
    // shell): not CLI tools, not noticed. Anything else unknown is dropped and noticed.
    expect(p.dropped).toEqual(["NotARealTool"]);
    expect(p.argv.join(" ")).not.toContain("NotARealTool");
    expect(JSON.stringify(p.files)).not.toContain("NotARealTool");
    // No allow-set recorded: no tool at all (fails closed), never the CLI's default set.
    const { tools: _, ...untooled } = spec();
    const none = launchPlan(session, untooled, PROBE, "linux", ctx("linux"));
    expect(flag(none.argv, "--tools")).toBe("");
    expect(CLI_TOOLS).toEqual(["Read", "Edit", "Write", "Glob", "Grep", "Bash", "WebFetch", "WebSearch"]);
  });

  test("disallowed_lists_agentic_tools_without_mcp_wildcard", () => {
    for (const os of ["linux", "darwin", "win32"] as const) {
      const p = plan(os, { tools: [...CLI_TOOLS] });
      const denied = flag(p.argv, "--disallowedTools")!.split(",");
      expect(denied).toEqual([...DISALLOWED_TOOLS]);
      for (const t of ["Task", "Agent", "Workflow", "SendMessage", "Skill", "Cron*", "ScheduleWakeup", "Monitor", "EnterWorktree", "ExitWorktree"]) {
        expect(denied).toContain(t);
      }
      for (const t of ["AskUserQuestion", "NotebookEdit", "ToolSearch", "EnterPlanMode", "ExitPlanMode", "ListAgents", "ReportFindings", "TaskStop"]) {
        expect(denied).toContain(t);
      }
      // Deny beats allow: an `mcp__*` deny would also remove Finisher's own server's tools.
      expect(denied.some((d) => d.startsWith("mcp"))).toBe(false);
      expect(denied.filter((d) => p.tools.includes(d))).toEqual([]);
    }
  });

  test("settings_file_contents", async () => {
    const p = plan("linux", { tools: ["Read", "Bash", "Glob"] }, { retentionDays: 30 });
    const settings = file(p, "claude.json");
    expect(settings.path).toBe(`${SETTINGS.linux}/claude.json`);
    expect(JSON.parse(settings.body)).toEqual({
      disableAllHooks: true,
      autoMemoryEnabled: false,
      crossSessionInbound: "refuse",
      attribution: { commit: "", pr: "" },
      cleanupPeriodDays: 30,
      permissions: { defaultMode: "default", blockWritesOutsideWorkingDirectories: true, ask: ["Read", "Glob", "Bash", "mcp__finisher__shell"] },
      env: {},
    });
    const body = JSON.parse(settings.body) as { permissions: Record<string, unknown> };
    expect(body.permissions.deny).toBeUndefined();
    expect(body.permissions.allow).toBeUndefined();
    // Every enabled tool asks (Decision 4), whatever the role.
    const all = plan("win32", { tools: [...CLI_TOOLS] });
    expect((JSON.parse(file(all, "claude.json").body) as { permissions: { ask: string[] } }).permissions.ask).toEqual([...all.tools, "mcp__finisher__shell"]);

    // Written 0600, temp-then-rename, into `settings/` (the sandbox's protected, read-only root).
    const tmp = mkdtempSync(join(tmpdir(), "claude-launch-"));
    try {
      const dirs = sessionDirs(join(tmp, "agents"), SID);
      mkdirSync(dirs.settings, { recursive: true, mode: 0o700 });
      writeFileSync(join(dirs.settings, "claude.json"), "stale", { mode: 0o644 });
      const local = launchPlan(session, spec(), PROBE, process.platform, ctx("linux", { agentsRoot: join(tmp, "agents") }));
      for (const f of local.files) expect(f.path.startsWith(dirs.settings)).toBe(true);
      await writeLaunchFiles(local, process.platform);
      expect(readdirSync(dirs.settings).sort()).toEqual(["claude.json", "mcp.json", "role.md"]);
      for (const f of local.files) {
        expect(readFileSync(f.path, "utf8")).toBe(f.body);
        if (process.platform !== "win32") expect(statSync(f.path).mode & 0o777).toBe(0o600);
      }
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });

  test("settings_body_is_schema_checked", () => {
    // Task 0: one wrongly typed known key makes `-p` drop the whole file silently.
    const good = JSON.parse(file(plan(), "claude.json").body) as Record<string, unknown>;
    expect(() => checkSettings(good)).not.toThrow();
    const bad = { ...good, permissions: { ...(good.permissions as object), blockWritesOutsideWorkingDirectories: "yes" } };
    expect(caught(() => checkSettings(bad)).code).toBe("INTERNAL");
    expect(caught(() => checkSettings({ ...good, disableAllHooks: false })).code).toBe("INTERNAL");
    expect(caught(() => checkSettings({ ...good, cleanupPeriodDays: "90" })).code).toBe("INTERNAL");
    expect(caught(() => plan("linux", {}, { retentionDays: 0 })).code).toBe("INTERNAL");
    // The probe's canary (Decision 4) is a typed key too.
    expect(() => checkSettings({ ...good, outputStyle: "Explanatory" })).not.toThrow();
    expect(caught(() => checkSettings({ ...good, outputStyle: 1 })).code).toBe("INTERNAL");
  });

  test("role_prompt_in_file_not_argv", () => {
    for (const os of ["linux", "win32"] as const) {
      const p = plan(os);
      expect(p.argv.join("\n")).not.toContain("ROLE-PROMPT-MARKER");
      expect(p.argv).not.toContain("--append-system-prompt");
      expect(p.argv).not.toContain("--system-prompt");
      const role = file(p, "role.md");
      expect(flag(p.argv, "--append-system-prompt-file")).toBe(role.path);
      expect(role.body).toBe(spec().system);
    }
  });

  test("no_budget_flag_on_plan_sign_in", () => {
    expect(plan("linux", {}, { planSignIn: true, remainingMoneyMicros: 5_000_000 }).argv).not.toContain("--max-budget-usd");
    expect(plan("linux", {}, { planSignIn: false, remainingMoneyMicros: null }).argv).not.toContain("--max-budget-usd");
  });

  test("max_budget_is_remaining_money", () => {
    const at = (micros: number) => {
      const argv = plan("linux", {}, { planSignIn: false, remainingMoneyMicros: micros }).argv;
      expect(argv.slice(-2)[0]).toBe("--max-budget-usd");
      return flag(argv, "--max-budget-usd");
    };
    expect(at(2_500_000)).toBe("2.5");
    expect(at(1_234_567)).toBe("1.234567");
    expect(at(1)).toBe("0.000001");
    expect(at(0)).toBe("0");
    expect(at(-40)).toBe("0");
    // Set on every (re)start to what is left then.
    const resumed = plan("linux", { providerRef: { kind: "claudeCode", id: UUID } }, { planSignIn: false, remainingMoneyMicros: 700_000 });
    expect(resumed.argv.slice(-3)).toEqual([`--resume=${UUID}`, "--max-budget-usd", "0.7"]);
  });

  describe("environment", () => {
    const parent = { PATH: "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8", CLAUDE_CODE_OAUTH_TOKEN: "tok", ANTHROPIC_API_KEY: "k1", OPENAI_API_KEY: "k2" };
    const sessionOf = (root: string) => {
      const dirs = sessionDirs(root, SID, "linux");
      const store = { grant: "/srv/data/providers/claudeCode/home/store", env: { name: "CLAUDE_SECURESTORAGE_CONFIG_DIR", value: "/srv/data/providers/claudeCode/home/store" } };
      return { dirs, env: sessionEnv({ session, server: "/srv/data/run/fin.sock", projectDir: "/p", dirs, store, parent, platform: "linux" }) };
    };

    test("env_passes_assert_session_env", () => {
      const { dirs, env } = sessionOf(ROOT.linux);
      const p = launchPlan(session, spec({ env }), PROBE, "linux", ctx("linux"));
      expect(Object.keys(p.env).sort()).toEqual([...CLAUDE_CODE_ENV].sort());
      expect(p.env).toEqual({
        CLAUDE_CODE_TMPDIR: dirs.tmp,
        DISABLE_AUTOUPDATER: "1",
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
        CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1",
        CLAUDE_CODE_DISABLE_CLAUDE_MDS: "1",
        CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1",
        BASH_DEFAULT_TIMEOUT_MS: "120000",
      });
      expect(() => assertSessionEnv({ ...env, ...p.env }, session, { extra: harnessExtra("claudeCode"), expect: env, platform: "linux" })).not.toThrow();
      // Windows: the same names, never `CLAUDE_CODE_GIT_BASH_PATH` (Decision 21).
      const win = plan("win32");
      expect(Object.keys(win.env).sort()).toEqual([...CLAUDE_CODE_ENV].sort());
      expect(win.env.CLAUDE_CODE_TMPDIR).toBe(`C:\\data\\agents\\${SID}\\tmp`);
      expect(win.env).not.toHaveProperty("CLAUDE_CODE_GIT_BASH_PATH");
      expect(win.env).not.toHaveProperty("CLAUDE_CODE_ENTRYPOINT");
    });

    test("env_never_has_secret_names", () => {
      const { env } = sessionOf(ROOT.linux);
      const p = launchPlan(session, spec({ env }), PROBE, "linux", ctx("linux"));
      const merged = { ...env, ...p.env };
      for (const k of ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", ...NEVER_IN_SESSION]) expect(merged).not.toHaveProperty(k);
      expect(Object.keys(merged).filter((k) => /_API_KEY$|TOKEN/i.test(k))).toEqual([]);
      expect(JSON.stringify([p.argv, p.files, p.env])).not.toMatch(/tok|k1|k2/);
    });
  });

  test("session_uuid_is_lossless", () => {
    expect(sessionUuid(SID)).toBe(UUID);
    expect(sessionUlid(UUID)).toBe(SID);
    expect(sessionUlid(UUID.toUpperCase())).toBe(SID);
    const crockford = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    const ulid = fc
      .tuple(fc.constantFrom(..."01234567"), fc.array(fc.constantFrom(...crockford), { minLength: 25, maxLength: 25 }))
      .map(([h, t]) => h + t.join(""));
    fc.assert(
      fc.property(ulid, (u) => {
        const id = sessionUuid(u);
        expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
        expect(sessionUlid(id)).toBe(u);
      }),
      { numRuns: 1000 },
    );
    fc.assert(
      fc.property(fc.uuid(), (id) => {
        expect(sessionUuid(sessionUlid(id))).toBe(id.toLowerCase());
      }),
      { numRuns: 500 },
    );
    expect(sessionUuid("00000000000000000000000000")).toBe("00000000-0000-0000-0000-000000000000");
    expect(sessionUuid("7ZZZZZZZZZZZZZZZZZZZZZZZZZ")).toBe("ffffffff-ffff-ffff-ffff-ffffffffffff");
    // Not a ULID (over 128 bits, a letter Crockford leaves out, the wrong length) or a UUID.
    for (const bad of ["8ZZZZZZZZZZZZZZZZZZZZZZZZZ", "01JC8Z5Q3M4N6P7R8S9T0VWXYU", "01JC8Z5Q3M", ""]) expect(caught(() => sessionUuid(bad)).code).toBe("INVALID_REQUEST");
    for (const bad of ["019311f2dc74254d63e1194e81be77df", "019311f2-dc74-254d-63e1-194e81be77dg", ""]) expect(caught(() => sessionUlid(bad)).code).toBe("INVALID_REQUEST");
  });

  test("resume_uses_equals_form", () => {
    const p = plan("linux", { providerRef: { kind: "claudeCode", id: UUID } });
    expect(p.argv).toEqual(golden("linux", "Read,Edit,Write,Glob,Grep,Bash", { tail: [`--resume=${UUID}`] }));
    expect(p.argv).not.toContain("--resume");
    expect(p.argv).not.toContain("--session-id");
    // A reference that is not this session's Claude Code session never resumes another one.
    expect(caught(() => plan("linux", { providerRef: { kind: "claudeCode", id: sessionUuid("01JC8Z5Q3M4N6P7R8S9T0VWXY0") } })).code).toBe("PROVIDER_PROTOCOL");
    expect(caught(() => plan("linux", { providerRef: { kind: "codex", id: UUID } })).code).toBe("PROVIDER_PROTOCOL");
  });

  test("mcp_fallback_argv", () => {
    // Decision 10: the documented MCP prompt tool, served by the same `finisher` SDK server.
    const p = plan("linux", {}, {}, { permissionPath: "mcp" });
    expect(p.argv).toEqual(golden("linux", "Read,Edit,Write,Glob,Grep,Bash", { prompt: "mcp__finisher__approve" }));
    expect(p.argv).not.toContain("stdio");
    expect(p.mcpTools).toEqual(["mcp__finisher__approve"]);
    expect(JSON.parse(file(p, "mcp.json").body)).toEqual({ mcpServers: { finisher: { type: "sdk", name: "finisher" } } });
    // The prompt tool itself never asks (it is how asking happens).
    expect(file(p, "claude.json").body).not.toContain("mcp__finisher__approve");
    const win = plan("win32", {}, {}, { permissionPath: "mcp", restricted: true });
    expect(win.argv).toEqual(golden("win32", "Read,Edit,Write,Glob,Grep", { prompt: "mcp__finisher__approve", restricted: true }));
    expect(win.mcpTools).toEqual(["mcp__finisher__shell", "mcp__finisher__approve"]);
  });
});

describe("the session's allow-set across a resume", () => {
  let dir: string;
  let control: ControlStore;
  let store: AgentStore;
  const cleanups: (() => Promise<void>)[] = [];
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "claude-launch-store-"));
    const clock = manualClock(Date.now());
    control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, dir, clock, systemIds(clock));
  });
  afterEach(async () => {
    for (const c of cleanups.splice(0)) await c();
    await control.close();
    rmSync(dir, { recursive: true, force: true });
  });
  const fixture = (name: string): FakeScript =>
    JSON.parse(readFileSync(new URL(`./fixtures/fake-scripts/${name}.json`, import.meta.url), "utf8")) as FakeScript;
  async function until(f: () => boolean, what: string): Promise<void> {
    const t0 = Date.now();
    while (!f()) {
      if (Date.now() - t0 > 5000) throw new Error(`timed out waiting for ${what}`);
      await new Promise((r) => setTimeout(r, 5));
    }
  }

  test("tools_survive_resume", async () => {
    const adapter = FakeAdapter([fixture("crash-late"), fixture("simple")]);
    const tools = ["Read", "Bash", "fin"];
    const h = makeRuntime({ store, control }, adapter, { tools: {}, planner: planner({ tools }) });
    cleanups.push(() => h.runtime.close());
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    expect(store.tools(s.id)).toEqual(tools);
    await until(() => store.get(s.id)!.status === "interrupted", "interrupted");
    await Effect.runPromise(h.runtime.resume(s.id, PEOPLE, "again"));
    await until(() => store.get(s.id)!.status === "idle", "idle");
    expect(adapter.log.opened.map((o) => o.spec.tools)).toEqual([tools, tools]);
    // Read back from `sessions.tools` by a fresh store over the same database (a restart).
    const again = new AgentStore(control, dir, manualClock(Date.now()), systemIds(manualClock(Date.now())));
    expect(again.tools(s.id)).toEqual(tools);
    const ref = { kind: "claudeCode", id: sessionUuid(SID) };
    const resumed = launchPlan(session, spec({ tools: again.tools(s.id)!, providerRef: ref }), PROBE, "linux", ctx("linux"));
    expect(flag(resumed.argv, "--tools")).toBe("Read,Bash");
    expect(resumed.argv).toContain(`--resume=${UUID}`);
    // A session that recorded no allow-set reads back null, and its spec carries none.
    const bare = await store.createSession({
      id: "01JC8Z5Q3M4N6P7R8S9T0VWXY1",
      projectId: "P1",
      harness: "fake",
      provider: "local",
      model: "fake",
      weights: "unknown",
      role: "critic",
      mode: "plan",
      base: { branch: "main", seq: 1 },
      budget: {},
      createdBy: { principal: { kind: "local", id: "local" } },
    });
    expect(store.tools(bare.id)).toBeNull();
  });
});
