// The Codex launch plan (plan 7E Task 7; Decisions 4–7, 9, 19, 21): the `app-server` argv per
// OS with every default-on feature of the pinned `features.txt` switched off unless named in
// `ALLOWED_FEATURES`, the session environment held to the sandbox contract, `thread/start` and
// `thread/resume` built separately from the generated protocol types, the role's allow-set
// mapped to Codex's tools, and the granular fallback of Decision 7.
import { readFileSync } from "node:fs";
import type { FinError, Session } from "@finisher/contracts";
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import type { SessionSpec } from "../src/agent/adapter.ts";
import {
  ALLOWED_FEATURES,
  type CodexLaunch,
  codexLaunch,
  codexPinFacts,
  codexTools,
  disabledFeatures,
  SHELL_DYNAMIC_TOOL,
  threadResumeParams,
} from "../src/agent/codex/launch.ts";
import type { CodexProbe } from "../src/agent/codex/probe-types.ts";
import { ThreadResumeParams, ThreadStartParams } from "../src/agent/codex/generated/protocol.ts";
import { assertSessionEnv, harnessExtra, sessionEnv, sessionShell } from "../src/agent/sandbox-env.ts";
import { sessionDirs } from "../src/agent/session-dirs.ts";

type Os = "linux" | "darwin" | "win32";

const SID = "01JC8Z5Q3M4N6P7R8S9T0VWXYZ";
const EDITOR = ["fin", "workspace_read", "workspace_write", "workspace_list", "Read", "Write", "Grep", "Edit", "Bash", "Glob"];
const CRITIC = ["fin", "workspace_read", "workspace_list", "Read", "Grep", "Glob"];
const ROOT: Record<Os, string> = { linux: "/srv/data/agents", darwin: "/Users/x/data/agents", win32: "C:\\data\\agents" };
const BIN: Record<Os, string> = { linux: "/opt/fin/codex/linux-x64/codex", darwin: "/opt/fin/codex/darwin-arm64/codex", win32: "C:\\fin\\codex\\win32-x64\\codex.exe" };
const CWD: Record<Os, string> = { linux: `/p/P/workspace/${SID}`, darwin: `/Users/x/P/workspace/${SID}`, win32: `C:\\p\\P\\workspace\\${SID}` };
const PARENT: Record<Os, NodeJS.ProcessEnv> = {
  linux: { PATH: "/usr/bin:/bin", LANG: "C.UTF-8", HOME: "/home/u", SHELL: "/bin/zsh" },
  darwin: { PATH: "/usr/bin:/bin", LANG: "en_US.UTF-8", HOME: "/Users/u", SHELL: "/bin/zsh" },
  win32: { Path: "C:\\Windows\\System32", USERPROFILE: "C:\\Users\\u", SystemRoot: "C:\\Windows" },
};
const PROMPT = "You are Finisher's editor. ROLE-PROMPT-MARKER \"quoted\" and\nmultiline";
const MODEL = "gpt-6.1-sol";

const session = { id: SID, harness: "codex" } as Session;
const PROBE: CodexProbe = { version: "0.167.0", sandbox: "read-only", approval: "on-request", models: [{ id: MODEL, displayName: "GPT-6.1 Sol", isDefault: true }] };
const COMMITTED = readFileSync(new URL("../../../vendor/codex/0.167.0/features.txt", import.meta.url), "utf8");

const dirs = (os: Os) => sessionDirs(ROOT[os], SID, os);
const envOf = (os: Os) =>
  sessionEnv({ session, server: os === "win32" ? "\\\\.\\pipe\\finisher" : "/run/fin.sock", projectDir: os === "win32" ? "C:\\p\\P" : "/p/P", dirs: dirs(os), parent: PARENT[os], platform: os });
const spec = (over: Partial<SessionSpec> = {}, os: Os = "linux"): SessionSpec => ({
  sessionId: SID,
  projectId: "P1",
  model: MODEL,
  role: "editor",
  mode: "draft",
  system: PROMPT,
  cwd: CWD[os],
  env: envOf(os),
  providerRef: null,
  tools: EDITOR,
  ...over,
});
const launch = (os: Os = "linux", s: Partial<SessionSpec> = {}, p: Partial<CodexProbe> = {}, features?: string): CodexLaunch =>
  codexLaunch(session, spec(s, os), { ...PROBE, ...p }, os, { binary: BIN[os], ...(features === undefined ? {} : { features }) });
const caught = (f: () => unknown): FinError => {
  try {
    f();
  } catch (e) {
    return e as FinError;
  }
  throw new Error("expected a failure");
};
/** `-c` values, in order. */
const overrides = (argv: readonly string[]) => argv.flatMap((a, i) => (argv[i - 1] === "-c" ? [a] : []));
const disables = (argv: readonly string[]) => argv.flatMap((a, i) => (argv[i - 1] === "--disable" ? [a] : []));
/** The `true` rows of a `features.txt`. */
const defaultOn = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim().split(/\s+/))
    .filter((r) => r.length >= 3 && r.at(-1) === "true")
    .map((r) => r[0]!);
/** The check itself: every default-on row not allowed must have a `--disable` in `argv`. */
const undisabled = (text: string, argv: readonly string[]) => {
  const off = new Set(disables(argv));
  return defaultOn(text).filter((f) => !ALLOWED_FEATURES.has(f) && !off.has(f));
};

/** The 0.167.0 pin's default-on rows Finisher switches off (`features.txt` order). */
const DISABLED_0167 = [
  "api_key_cyber_access_programs",
  "apps",
  "auth_elicitation",
  "bedrock_setup_wizard",
  "browser_annotation_api",
  "browser_use",
  "browser_use_external",
  "browser_use_full_cdp_access",
  "cli_daybreak",
  "code_mode_host",
  "code_mode_interrupt",
  "collaboration_modes",
  "compaction_image_budget",
  "computer_use",
  "daemon_auto_start",
  "deferred_tool_world_state",
  "enable_request_compression",
  "fast_mode",
  "goals",
  "guardian_approval",
  "guardian_reuse_parent_compaction",
  "hooks",
  "image_generation",
  "in_app_browser",
  "in_app_chat",
  "in_app_dictation",
  "in_app_local_automation",
  "in_app_updates",
  "in_app_voice",
  "incremental_tools",
  "instant_interrupt",
  "mcp_oauth_refresh_coordination",
  "mentions_v2",
  "multi_agent",
  "plugin_discovery_prefetch",
  "plugin_sharing",
  "plugins",
  "prediction_preserve_parent_tools",
  "realtime_conversation",
  "reasoning_effort_override",
  "remote_plugin",
  "resize_all_images",
  "shell_snapshot",
  "shell_tool",
  "skill_mcp_dependency_install",
  "skill_search",
  "sleep_tool",
  "system_proxy_fallback",
  "terminal_resize_reflow",
  "tool_call_mcp_elicitation",
  "tool_search_always_defer_mcp_tools",
  "tool_suggest",
  "tui_app_server",
  "ultrafast_mode",
  "unbounded_connection_retries",
  "unified_exec",
  "unified_exec_tty",
  "view_image",
  "workspace_dependencies",
  "worktrees",
  "write_stdin_approval",
];

/** `shell_environment_policy.set` as TOML: the session environment, keys sorted. */
const tomlSet = (env: Readonly<Record<string, string>>) =>
  `shell_environment_policy.set={${Object.keys(env)
    .sort()
    .map((k) => `${k}=${JSON.stringify(env[k])}`)
    .join(", ")}}`;

function golden(os: Os, web: "disabled" | "live" = "disabled"): string[] {
  return [
    BIN[os],
    "app-server",
    "--listen",
    "stdio://",
    "--strict-config",
    ...["-c", 'cli_auth_credentials_store="file"'],
    ...["-c", "project_doc_max_bytes=0"],
    ...["-c", "project_doc_fallback_filenames=[]"],
    ...["-c", "check_for_update_on_startup=false"],
    ...["-c", "analytics.enabled=false"],
    ...["-c", 'history.persistence="none"'],
    ...["-c", `web_search="${web}"`],
    ...["-c", 'shell_environment_policy.inherit="none"'],
    ...["-c", tomlSet(envOf(os))],
    ...["-c", "skills.bundled.enabled=false"],
    ...["-c", "mcp_servers={}"],
    ...["-c", "agents.enabled=false"],
    ...DISABLED_0167.flatMap((f) => ["--disable", f]),
  ];
}

describe("Codex launch plan", () => {
  test.each(["linux", "darwin", "win32"] as const)("argv_matches_decision_4 (%s)", (os) => {
    const l = launch(os);
    expect(l.argv).toEqual(golden(os));
    expect(l.disabledFeatures).toEqual(DISABLED_0167);
    // The role prompt travels over the protocol only (Decision 4, spec §4.6): no argv, no file.
    const prompt = l.argv.filter((a) => a.includes("ROLE-PROMPT-MARKER") || a.includes("Finisher's editor"));
    expect(prompt).toEqual([]);
    expect(l.argv.filter((a) => a === "--strict-config")).toHaveLength(1);
    // No `--dangerously-*`, no `--enable`, no `--profile`, no `--config` file.
    expect(l.argv.some((a) => /^--(dangerously|enable|profile|yolo|full-auto)/.test(a))).toBe(false);
  });

  test("new_default_feature_is_disabled", () => {
    const l = launch();
    // Every `true` row of the committed `features.txt` not allowed has its `--disable`.
    expect(undisabled(COMMITTED, l.argv)).toEqual([]);
    expect(l.disabledFeatures).toEqual(defaultOn(COMMITTED).filter((f) => !ALLOWED_FEATURES.has(f)));
    expect(disables(l.argv)).toEqual(l.disabledFeatures);
    // A row a re-pin turns on fails the check against today's launch...
    const planted = `${COMMITTED}brand_new_default                        stable             true\n`;
    expect(undisabled(planted, l.argv)).toEqual(["brand_new_default"]);
    // ...and a launch from that text switches it off: off until named.
    const p = launch("linux", {}, {}, planted);
    expect(undisabled(planted, p.argv)).toEqual([]);
    expect(p.disabledFeatures).toContain("brand_new_default");
    expect(disabledFeatures(planted)).toEqual([...l.disabledFeatures, "brand_new_default"]);
    // A planted row under a two-word stage is read the same way.
    expect(disabledFeatures(`${COMMITTED}another_one   under development   true\n`)).toContain("another_one");
    // Every allowed name is a default-on row of the pin: no stale allowance.
    const on = new Set(defaultOn(COMMITTED));
    for (const f of ALLOWED_FEATURES) expect(on.has(f), f).toBe(true);
    for (const f of ALLOWED_FEATURES) expect(disables(l.argv)).not.toContain(f);
    // A row that is neither true nor false is no feature list: fail closed.
    expect(caught(() => disabledFeatures(`${COMMITTED}garbled stable maybe\n`)).code).toBe("INTERNAL");
    expect(caught(() => disabledFeatures("")).code).toBe("INTERNAL");
  });

  test("thread_params_match", () => {
    for (const os of ["linux", "darwin", "win32"] as const) {
      const l = launch(os);
      expect(l.startParams).toEqual({
        cwd: CWD[os],
        model: MODEL,
        sandbox: "read-only",
        approvalPolicy: "on-request",
        approvalsReviewer: "user",
        developerInstructions: PROMPT,
        ephemeral: false,
        serviceName: "finisher",
        dynamicTools: [SHELL_DYNAMIC_TOOL],
        config: {},
      });
      // The generated type takes it as is (no key Codex does not know).
      expect(Schema.decodeUnknownSync(ThreadStartParams)(l.startParams, { onExcessProperty: "error" })).toEqual(l.startParams);
    }
    expect(SHELL_DYNAMIC_TOOL).toMatchObject({ type: "function", name: "shell", inputSchema: { type: "object", required: ["command"] } });
    expect(Object.keys((SHELL_DYNAMIC_TOOL.inputSchema as { properties: object }).properties).sort()).toEqual(["command", "timeoutMs"]);
    // The cwd is the session's own workspace, nothing else.
    expect(caught(() => launch("linux", { cwd: "/p/P/workspace/OTHER" })).code).toBe("INTERNAL");
    expect(caught(() => launch("linux", { cwd: `/p/P/elsewhere/${SID}` })).code).toBe("INTERNAL");
    expect(caught(() => launch("linux", { cwd: `p/P/workspace/${SID}` })).code).toBe("INTERNAL");
    // A version other than the pin is refused.
    const v = caught(() => launch("linux", {}, { version: "0.166.0" }));
    expect(v.code).toBe("PROVIDER_VERSION");
    expect(v.details).toMatchObject({ harness: "codex", version: "0.166.0", pinned: "0.167.0" });
  });

  test("tools_mapping_from_allow_set", () => {
    expect(codexTools(EDITOR)).toEqual({ shell: true, patches: true, webSearch: false, dropped: ["Read", "Grep", "Glob"] });
    expect(codexTools(CRITIC)).toEqual({ shell: true, patches: false, webSearch: false, dropped: ["Read", "Grep", "Glob"] });
    expect(codexTools(["MultiEdit"])).toEqual({ shell: false, patches: true, webSearch: false, dropped: [] });
    expect(codexTools(["Write"]).patches).toBe(true);
    expect(codexTools(["WebSearch", "WebFetch", "NotebookEdit"])).toEqual({ shell: false, patches: false, webSearch: true, dropped: ["WebFetch", "NotebookEdit"] });
    // The built-in agent's own tools are not CLI tools: neither mapped nor noticed.
    expect(codexTools(["workspace_read", "workspace_write", "workspace_list"])).toEqual({ shell: false, patches: false, webSearch: false, dropped: [] });
    expect(codexTools(undefined)).toEqual({ shell: false, patches: false, webSearch: false, dropped: [] });
    // The plan carries the mapping and the dropped names, for the session's notice.
    const l = launch("linux", { tools: [...CRITIC, "WebFetch"] });
    expect(l.tools).toEqual({ shell: true, patches: false, webSearch: false, dropped: ["Read", "Grep", "Glob", "WebFetch"] });
    expect(l.startParams.dynamicTools).toEqual([SHELL_DYNAMIC_TOOL]);
  });

  test("no_shell_tool_without_fin_or_bash", () => {
    for (const tools of [["Read", "Edit", "Write"], [], ["WebSearch"]]) {
      const l = launch("linux", { tools });
      expect(l.startParams).not.toHaveProperty("dynamicTools");
      expect(l.tools.shell).toBe(false);
      // Codex's own shells stay off whatever the role says (Decision 9).
      expect(disables(l.argv)).toEqual(expect.arrayContaining(["shell_tool", "unified_exec", "unified_exec_tty", "shell_snapshot", "write_stdin_approval"]));
      // Such a role resumes normally: nothing to restore (Decision 4).
      expect(() => threadResumeParams(l, "t-1", { version: "0.167.0", resumeRestoresDynamicTools: false })).not.toThrow();
    }
    for (const tools of [["fin"], ["Bash"]]) expect(launch("linux", { tools }).startParams.dynamicTools).toEqual([SHELL_DYNAMIC_TOOL]);
  });

  test("web_search_only_when_allowed", () => {
    const web = (tools: string[]) => overrides(launch("linux", { tools }).argv).filter((o) => o.startsWith("web_search="));
    expect(web(EDITOR)).toEqual(['web_search="disabled"']);
    expect(web([])).toEqual(['web_search="disabled"']);
    expect(web(["WebFetch"])).toEqual(['web_search="disabled"']);
    expect(web([...EDITOR, "WebSearch"])).toEqual(['web_search="live"']);
    expect(launch("linux", { tools: [...EDITOR, "WebSearch"] }).argv).toEqual(golden("linux", "live"));
  });

  test("env_passes_assert_session_env", () => {
    for (const os of ["linux", "darwin", "win32"] as const) {
      const l = launch(os);
      // A process adds only Codex's own switch (Decision 5; `harnessExtra("codex")`).
      expect(l.env).toEqual({ NO_COLOR: "1" });
      const merged = { ...envOf(os), ...l.env };
      expect(() => assertSessionEnv(merged, session, { extra: harnessExtra("codex"), platform: os, expect: envOf(os) })).not.toThrow();
      const d = dirs(os);
      expect(merged.CODEX_HOME).toBe(d.provider);
      expect(merged.HOME).toBe(d.home);
      if (os === "win32") expect(merged.USERPROFILE).toBe(d.home);
      else expect(merged.SHELL).toBe(sessionShell());
      expect(merged).not.toHaveProperty("RUST_LOG");
      // The second layer, for commands Codex would run itself: the same session environment.
      const set = overrides(l.argv).find((o) => o.startsWith("shell_environment_policy.set="))!;
      expect(set).toBe(tomlSet(envOf(os)));
      expect(set).toContain(`HOME=${JSON.stringify(d.home)}`);
      expect(set).not.toContain(JSON.stringify(PARENT[os].HOME ?? PARENT[os].USERPROFILE));
    }
    // A session environment whose `HOME` is the user's is refused, not launched.
    const user = caught(() => launch("linux", { env: { ...envOf("linux"), HOME: "/home/u" } }));
    expect(user.code).toBe("SANDBOX_ENV");
    expect(user.details).toMatchObject({ name: "HOME" });
    // So is one that carries a sign-in variable or names another session's provider directory.
    expect(caught(() => launch("linux", { env: { ...envOf("linux"), OPENAI_API_KEY: "k" } })).code).toBe("SANDBOX_ENV");
    expect(caught(() => launch("linux", { env: { ...envOf("linux"), CODEX_HOME: "/srv/data/agents/OTHER/provider" } })).code).toBe("SANDBOX_ENV");
    expect(caught(() => launch("linux", { env: { ...envOf("linux"), RUST_LOG: "trace" } })).code).toBe("SANDBOX_ENV");
  });

  test("resume_shares_common_settings", () => {
    const pin = { version: "0.167.0", resumeRestoresDynamicTools: false };
    for (const os of ["linux", "darwin", "win32"] as const) {
      const l = launch(os, { tools: ["Read", "Edit"] });
      const s = l.startParams;
      expect(threadResumeParams(l, "thr-1", pin)).toEqual({
        threadId: "thr-1",
        cwd: s.cwd,
        model: s.model,
        sandbox: s.sandbox,
        approvalPolicy: s.approvalPolicy,
        approvalsReviewer: s.approvalsReviewer,
        developerInstructions: s.developerInstructions,
        config: s.config,
      });
    }
    expect(caught(() => threadResumeParams(launch("linux", { tools: [] }), "", pin)).code).toBe("INVALID_REQUEST");
  });

  test("start_only_fields_absent_on_resume", () => {
    const START_ONLY = ["dynamicTools", "ephemeral", "serviceName"];
    // The pin's schema has no such fields on resume (Decision 4): checked against the generated type.
    const resumeKeys = new Set(Object.keys(ThreadResumeParams.fields));
    for (const k of START_ONLY) expect(resumeKeys.has(k), k).toBe(false);
    for (const l of [launch("linux", { tools: [] }), launch("linux", {}, {}, undefined)]) {
      const r = threadResumeParams(l, "thr-1", { version: "0.167.0", resumeRestoresDynamicTools: true });
      for (const k of Object.keys(r)) expect(resumeKeys.has(k), k).toBe(true);
      for (const k of START_ONLY) expect(r).not.toHaveProperty(k);
      expect(Schema.decodeUnknownSync(ThreadResumeParams)(r, { onExcessProperty: "error" })).toEqual(r);
    }
  });

  test("resume_with_shell_refused_until_pin_says_restored", () => {
    // Today's pin has not recorded the fact (Task 0 Step 3 is signed-in): absent is false.
    expect(codexPinFacts()).toEqual({ version: "0.167.0", resumeRestoresDynamicTools: false });
    const l = launch("linux");
    for (const pin of [undefined, { version: "0.167.0", resumeRestoresDynamicTools: false }]) {
      const e = caught(() => (pin === undefined ? threadResumeParams(l, "thr-1") : threadResumeParams(l, "thr-1", pin)));
      expect(e.code).toBe("PROVIDER_VERSION");
      expect(e.details).toMatchObject({ harness: "codex", version: "0.167.0", missing: "resumeDynamicTools" });
      expect(e.fix).toBe("start a new session");
    }
    const r = threadResumeParams(l, "thr-1", { version: "0.167.0", resumeRestoresDynamicTools: true });
    expect(r.threadId).toBe("thr-1");
    expect(r).not.toHaveProperty("dynamicTools");
  });

  test("no_forced_login_override", () => {
    // Nothing in the launch depends on the sign-in: a ChatGPT plan and an API key get the same
    // argv and thread parameters, and neither restricts the other (Decision 4).
    for (const os of ["linux", "darwin", "win32"] as const) {
      const a = launch(os);
      const b = launch(os);
      expect(b).toEqual(a);
      const all = JSON.stringify(a);
      expect(all).not.toMatch(/forced_login_method|forcedLoginMethod|forced_chatgpt_workspace_id/);
      expect(overrides(a.argv).filter((o) => o.startsWith("cli_auth_credentials_store="))).toEqual(['cli_auth_credentials_store="file"']);
      expect(a.startParams.config).toEqual({});
    }
  });

  test("granular_fallback_params", () => {
    const GRANULAR = { granular: { sandbox_approval: true, rules: true, mcp_elicitations: false, request_permissions: false, skill_approval: false } };
    const l = launch("linux", {}, { sandbox: "danger-full-access", approval: "granular" });
    expect(l.startParams.sandbox).toBe("danger-full-access");
    expect(l.startParams.approvalPolicy).toEqual(GRANULAR);
    expect(l.startParams.approvalsReviewer).toBe("user");
    expect(Schema.decodeUnknownSync(ThreadStartParams)(l.startParams, { onExcessProperty: "error" })).toEqual(l.startParams);
    // The argv is the same as the read-only launch's: the fallback is thread parameters only.
    expect(l.argv).toEqual(launch("linux").argv);
    const r = threadResumeParams(l, "thr-1", { version: "0.167.0", resumeRestoresDynamicTools: true });
    expect(r.sandbox).toBe("danger-full-access");
    expect(r.approvalPolicy).toEqual(GRANULAR);
    // Read-only with granular approvals is a valid pairing (patches still ask).
    expect(launch("linux", {}, { approval: "granular" }).startParams).toMatchObject({ sandbox: "read-only", approvalPolicy: GRANULAR });
    // Full access with `on-request` would let patches through unasked: never launched.
    const e = caught(() => launch("linux", {}, { sandbox: "danger-full-access", approval: "on-request" }));
    expect(e.code).toBe("PROVIDER_VERSION");
    expect(e.details).toMatchObject({ harness: "codex", missing: "patchApproval" });
  });
});
