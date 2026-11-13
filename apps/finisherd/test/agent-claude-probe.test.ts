// The Claude Code capability probe and the per-instance init check (plan 7D Task 5, Decision 8):
// free and unsigned, never a prompt when the CLI is signed in by other means, cached per
// `(version, flags hash, OS)` with no account data, the `stdio`/`mcp` permission path, the init
// predicate item by item, and the first-turn gate that stops an instance whose init fails before
// any event reaches the runtime. Processes are `fake-claude` through a host sandbox (`direct`).
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { FinError } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { type Detection, NO_CAPABILITIES } from "../src/agent/adapter.ts";
import { CLI_TOOLS, cliTools, instancePlan, writeLaunchFiles } from "../src/agent/claude/launch.ts";
import { checkInit, completeProbe, firstTurnGate, flagsHashOf, hostDirs, InstanceLines, type ProbeDeps, probe, probeCachePath, probeFlagsHash, readProbeCache, untestedVersionNotices } from "../src/agent/claude/probe.ts";
import { CANARY_STYLE, checkInitialize, mcpReply, signInIgnored } from "../src/agent/claude/probe-check.ts";
import { type CliLine, decodeLine, encodeControl, encodeUser, type SystemInit } from "../src/agent/claude/protocol.ts";
import { openHostSandbox, type SessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(!existsSync(FAKE) && !REQUIRE_FAKE);

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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-probe-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8" };
const VERSION = "2.1.321";
const UNSIGNED = {
  auth: { loggedIn: false },
  initialize: { account: { tokenSource: "none", apiProvider: "firstParty" } },
  steps: [{ expect: "user", notLoggedIn: true }],
};

const data = () => join(tmp, "data");
function sandboxDeps(): SessionSandboxDeps {
  mkdirSync(data(), { recursive: true, mode: 0o700 });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data(), projectDirOf: () => undefined, finPath: "/bin/sh", localEndpoint: join(data(), "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts") };
}

/** The fake as an installed CLI of its own: the binary and its sidecar in a fresh directory. */
function install(script: unknown, version = VERSION): { binary: string; observed: string } {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, WIN ? "claude.exe" : "claude");
  copyFileSync(FAKE, binary);
  const observed = join(tmp, "observed.json");
  writeFileSync(join(dir, "script.json"), JSON.stringify(script));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed, version }));
  return { binary, observed };
}

type Observed = { pid: number; argv: string[]; refused?: string; flags: Record<string, unknown>; envNames: string[]; settings: string; controlRequests: { subtype: string }[]; userLines?: unknown[] }[];
const observed = (path: string): Observed => (existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as Observed) : []);

const detection = (version = VERSION): Detection => ({ installed: true, version, supported: null, signedIn: false, capabilities: NO_CAPABILITIES });
const probeDeps = (binary: string): ProbeDeps => ({ sandbox: sandboxDeps(), binary, timeoutMs: 10_000 });

async function run<A>(e: Effect.Effect<A, FinError>): Promise<A> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) return exit.value;
  const s = JSON.stringify(exit.cause);
  throw new Error(`failed: ${s}`);
}

/** The failure of `e`: its code and details. */
async function failure(e: Effect.Effect<unknown, FinError>): Promise<{ code: string; details: Record<string, unknown> }> {
  const exit = await Effect.runPromiseExit(e);
  expect(Exit.isFailure(exit)).toBe(true);
  const s = JSON.stringify(Exit.isFailure(exit) ? exit.cause : null);
  const m = /"code":"([A-Z_]+)".*?"details":(\{[^{}]*\})/.exec(s);
  return { code: m?.[1] ?? "", details: m === null ? {} : (JSON.parse(m[2]!) as Record<string, unknown>) };
}

// ---- the init predicate (unit) ----

const UUID = "019311f2-dc74-254d-63e1-194e81be77df";
const PLAN = { argv: ["claude", "-p", "--session-id", UUID], tools: ["Read", "Edit", "Bash"], mcpTools: [] as string[] };
const BUILTIN = (name: string) => ({ name, path: "builtin", source: `${name}@builtin` });
function init(over: Record<string, unknown> = {}): SystemInit {
  const base: Record<string, unknown> = {
    type: "system",
    subtype: "init",
    session_id: UUID,
    tools: ["Read", "Edit", "Bash"],
    model: "claude-opus-5-5",
    permissionMode: "default",
    apiKeySource: "none",
    claude_code_version: VERSION,
    mcp_servers: [{ name: "finisher", status: "connected", source: "sdk" }],
    plugins: [BUILTIN("cc-plugin-agents-md"), BUILTIN("cc-plugin-privacy")],
    skills: [],
    slash_commands: [],
    agents: ["claude", "Explore", "general-purpose", "Plan", "statusline-setup"],
    output_style: "default",
  };
  for (const [k, v] of Object.entries(over)) {
    if (v === "$remove") delete base[k];
    else base[k] = v;
  }
  const l = decodeLine(JSON.stringify(base));
  if (l instanceof FinError || l.kind !== "init") throw new Error(`bad fixture: ${JSON.stringify(l)}`);
  return l;
}
const verdict = (i: SystemInit, opts: Parameters<typeof checkInit>[2] = {}) => {
  const e = checkInit(i, PLAN, opts);
  return e === null ? null : { code: e.code, ...(e.details as { missing: string; item?: string }) };
};

describe("the init predicate", () => {
  test("oauth_ignored_refused: from the account, never from apiKeySource", () => {
    // Verified on 2.1.321 (7D Task 0, s1a, s3A–C): a claude.ai sign-in's init says
    // `apiKeySource: "none"`; such a session is never refused for it.
    expect(verdict(init({ apiKeySource: "none" }))).toBeNull();
    expect(verdict(init({ apiKeySource: "none" }), { signInIgnored: false })).toBeNull();
    expect(verdict(init({ apiKeySource: "none" }), { signInIgnored: true })).toMatchObject({ code: "PROVIDER_VERSION", missing: "oauth", item: "account" });
    expect(verdict(init({ apiKeySource: "/login managed key" }), { signInIgnored: true })).toMatchObject({ missing: "oauth" });
  });

  test("sign_in_ignored_reads_initialize_account", () => {
    // The signed-in shapes of 2.1.321 (7D Task 0, s1a): no `tokenSource`; `email` once the CLI
    // has the profile; the `subscriptionType` text varies for one account.
    expect(signInIgnored({ subscriptionType: "Claude Max", apiProvider: "firstParty" }, "claude.ai")).toBe(false);
    expect(signInIgnored({ email: "a@example.invalid", subscriptionType: "Claude Ultra", apiProvider: "firstParty" }, "claude.ai")).toBe(false);
    // Unsigned, or a plan sign-in without its plan: ignored.
    expect(signInIgnored({ tokenSource: "none", apiProvider: "firstParty" }, "claude.ai")).toBe(true);
    expect(signInIgnored({ apiProvider: "firstParty" }, "claude.ai")).toBe(true);
    expect(signInIgnored({ subscriptionType: "", apiProvider: "firstParty" }, "claude.ai")).toBe(true);
    expect(signInIgnored(undefined, "claude.ai")).toBe(true);
    expect(signInIgnored(null, "console")).toBe(true);
    // Another method (a Console sign-in's shape is unverified): only an unsigned account.
    expect(signInIgnored({ apiProvider: "firstParty" }, "console")).toBe(false);
    expect(signInIgnored({ tokenSource: "none", apiProvider: "firstParty" }, "other")).toBe(true);
  });

  test("session_uuid_mismatch_refused", () => {
    expect(verdict(init({ session_id: "01890a5d-ac96-774b-bcce-b302099a8057" }))).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item: "session_id" });
    // A resumed instance passes its UUID as `--resume=<uuid>`.
    expect(checkInit(init(), { ...PLAN, argv: ["claude", `--resume=${UUID}`] })).toBeNull();
  });

  test("builtin_plugins_accepted_others_refused", () => {
    expect(verdict(init({ plugins: [] }))).toBeNull();
    expect(verdict(init({ plugins: [BUILTIN("cc-plugin-telemetry")] }))).toBeNull();
    expect(verdict(init({ plugins: [{ name: "evil", path: "/home/u/.claude/plugins/evil", source: "evil@market" }] }))).toMatchObject({ missing: "isolation", item: "plugin:evil" });
    expect(verdict(init({ plugins: [{ name: "half", path: "builtin", source: "half@market" }] }))).toMatchObject({ item: "plugin:half" });
    expect(verdict(init({ plugins: [{ name: "half", path: "/x", source: "half@builtin" }] }))).toMatchObject({ item: "plugin:half" });
  });

  test("foreign_mcp_server_refused", () => {
    expect(verdict(init({ mcp_servers: [{ name: "finisher", status: "connected" }, { name: "github", status: "connected" }] }))).toMatchObject({ missing: "isolation", item: "mcp_server:github" });
    // A foreign server's tool is outside the allow-set too.
    expect(verdict(init({ tools: ["Read", "mcp__github__issues"] }))).toMatchObject({ missing: "isolation", item: "tool:mcp__github__issues" });
  });

  test("unconnected_finisher_server_refused", () => {
    expect(verdict(init({ mcp_servers: [{ name: "finisher", status: "failed", source: "sdk" }] }))).toMatchObject({ missing: "mcp", item: "finisher" });
    expect(verdict(init({ mcp_servers: [] }))).toMatchObject({ missing: "mcp", item: "finisher" });
  });

  test("skills_or_slash_commands_refused", () => {
    expect(verdict(init({ skills: [{ name: "deploy" }] }))).toMatchObject({ missing: "isolation", item: "skill:deploy" });
    expect(verdict(init({ skills: ["deploy"] }))).toMatchObject({ item: "skill:deploy" });
    expect(verdict(init({ slash_commands: ["review"] }))).toMatchObject({ missing: "isolation", item: "slash_command:review" });
    // `initialize` lists the commands before any prompt: one there refuses too.
    const body = { models: [{ value: "default" }], commands: [{ name: "review" }], capabilities: [], claude_code_version: VERSION, account: { tokenSource: "none" }, output_style: CANARY_STYLE };
    expect(checkInitialize(body, VERSION)).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "command:review" } });
  });

  test("memory_paths_refused", () => {
    expect(verdict(init())).toBeNull(); // absent (2.1.321 with auto memory off)
    expect(verdict(init({ memory_paths: null }))).toBeNull();
    expect(verdict(init({ memory_paths: { auto: "/cfg/projects/x/memory/" } }))).toMatchObject({ missing: "isolation", item: "memory_paths" });
    expect(verdict(init({ memory_paths: [] }))).toMatchObject({ item: "memory_paths" });
  });

  test("foreign_agent_refused", () => {
    expect(verdict(init({ agents: ["claude", "Plan"] }))).toBeNull();
    expect(verdict(init({ agents: ["claude", "Explore", "general-purpose", "Plan", "statusline-setup", "reviewer"] }))).toMatchObject({ missing: "isolation", item: "agent:reviewer" });
  });

  test("settings_canary_missing_refused", () => {
    expect(verdict(init({ output_style: "Explanatory" }), { canary: true })).toBeNull();
    expect(verdict(init(), { canary: true })).toMatchObject({ missing: "settings", item: "outputStyle" });
    // Sessions run without the canary.
    expect(verdict(init())).toBeNull();
  });

  test("additional_directories_refused", () => {
    expect(verdict(init({ additional_directories: [] }))).toBeNull();
    expect(verdict(init({ additional_directories: ["/home/u/src"] }))).toMatchObject({ missing: "isolation", item: "additional_directories" });
  });

  test("unsigned_instance_needs_no_api_key_source", () => {
    // The probe's unsigned turn: any `apiKeySource` but `none` signs it in.
    expect(verdict(init({ output_style: CANARY_STYLE }), { unsigned: true, canary: true })).toBeNull();
    expect(verdict(init({ apiKeySource: "ANTHROPIC_API_KEY", output_style: CANARY_STYLE }), { unsigned: true, canary: true })).toMatchObject({ missing: "unsigned", item: "apiKeySource" });
  });

  test("initialize_account_must_be_exactly_unsigned", () => {
    const body = (account: unknown, over: Record<string, unknown> = {}) => ({
      models: [{ value: "default" }],
      commands: [],
      capabilities: [],
      claude_code_version: VERSION,
      output_style: CANARY_STYLE,
      ...(account === undefined ? {} : { account }),
      ...over,
    });
    const unsigned = (account: unknown) => (checkInitialize(body(account), VERSION) as { unsigned?: boolean }).unsigned;
    expect(unsigned({ tokenSource: "none", apiProvider: "firstParty" })).toBe(true);
    // A claude.ai sign-in (2.1.321, 7D Task 0): no `tokenSource` key.
    expect(unsigned({ subscriptionType: "Claude Max", apiProvider: "firstParty" })).toBe(false);
    expect(unsigned({ email: "a@example.invalid", subscriptionType: "Claude Ultra", apiProvider: "firstParty" })).toBe(false);
    // What 2.1.321 answers with an API key (in the env or a settings file's `env`), an
    // `apiKeyHelper`, Bedrock: each is signed in, as is anything else.
    expect(unsigned({ tokenSource: "none", apiKeySource: "ANTHROPIC_API_KEY", apiProvider: "firstParty" })).toBe(false);
    expect(unsigned({ tokenSource: "apiKeyHelper", apiProvider: "firstParty" })).toBe(false);
    expect(unsigned({ apiProvider: "bedrock" })).toBe(false);
    expect(unsigned({ apiProvider: "vertex" })).toBe(false);
    expect(unsigned({ tokenSource: "none", apiProvider: "bedrock" })).toBe(false);
    expect(unsigned({ tokenSource: "none" })).toBe(false);
    expect(unsigned({})).toBe(false);
    expect(checkInitialize(body(undefined), VERSION)).toMatchObject({ details: { missing: "account" } });
    expect(checkInitialize(body("none"), VERSION)).toMatchObject({ details: { missing: "account" } });
    // The settings canary shows in `initialize` too: a dropped file answers `default`.
    expect(checkInitialize(body({ tokenSource: "none", apiProvider: "firstParty" }, { output_style: "default" }), VERSION)).toMatchObject({ details: { missing: "settings", item: "outputStyle" } });
  });

  test("missing_init_key_names_it", () => {
    for (const k of ["permissionMode", "apiKeySource", "mcp_servers", "plugins", "skills", "slash_commands", "agents"]) {
      expect(verdict(init({ [k]: "$remove" })), k).toMatchObject({ code: "PROVIDER_VERSION", missing: k });
    }
    expect(verdict(init({ permissionMode: "acceptEdits" }))).toMatchObject({ missing: "isolation", item: "permissionMode:acceptEdits" });
    expect(verdict(init({ tools: ["Read", "Edit", "Bash", "Task"] }))).toMatchObject({ missing: "isolation", item: "tool:Task" });
    expect(verdict(init({ tools: ["Read", "Edit"] }))).toMatchObject({ missing: "tools", item: "Bash" });
  });

  test("untested_version_notice_once", () => {
    const notice = untestedVersionNotices(["2.1.300"]);
    expect(notice("2.1.300")).toBeNull();
    expect(notice(VERSION)).toEqual({ type: "notice", name: "untestedVersion", details: { harness: "claudeCode", version: VERSION } });
    expect(notice(VERSION)).toBeNull();
    expect(notice("2.1.322")).toMatchObject({ name: "untestedVersion" });
    // A fresh source (another server start) notices again; the default list is Task 15's.
    expect(untestedVersionNotices()(VERSION)).not.toBeNull();
  });

  test("mcp_handshake_answers", () => {
    expect(mcpReply("finisher", { method: "initialize", id: 0, params: { protocolVersion: "2025-11-25" } }, [])).toMatchObject({ mcp_response: { id: 0, result: { protocolVersion: "2025-11-25" } } });
    expect(mcpReply("finisher", { method: "tools/list", id: 1 }, ["mcp__finisher__approve"])).toMatchObject({ mcp_response: { result: { tools: [{ name: "approve" }] } } });
    expect(mcpReply("finisher", { method: "tools/list", id: 1 }, [])).toMatchObject({ mcp_response: { result: { tools: [] } } });
    expect(mcpReply("other", { method: "initialize", id: 0 }, [])).toHaveProperty("error");
  });
});

// ---- the probe against fake-claude ----

describe("the probe", () => {
  withFake("probe_is_free_and_unsigned", async () => {
    const { binary, observed: obs } = install(UNSIGNED);
    const r = await run(probe(detection(), "h1", probeDeps(binary)));
    expect(r).toMatchObject({ version: VERSION, permissionPath: "stdio" });
    expect(r.initPending).toBeUndefined();
    expect([...r.tools].sort()).toEqual(cliTools([...CLI_TOOLS], false).tools.sort());
    expect(r.models.map((m) => m.id)).toContain("default");
    const [rec, ...more] = observed(obs);
    expect(more).toEqual([]);
    // A throwaway config dir, never the sign-in store; the settings (with the canary) applied.
    expect(rec!.envNames).toContain("CLAUDE_CONFIG_DIR");
    expect(rec!.envNames).not.toContain("CLAUDE_SECURESTORAGE_CONFIG_DIR");
    expect(rec!.envNames).not.toContain("CLAUDE_CODE_OAUTH_TOKEN");
    expect(rec!.settings).toBe("applied");
    expect(rec!.userLines).toHaveLength(1); // the one unsigned turn
    expect(existsSync(join(data(), "providers"))).toBe(false);
    // The host sandbox's directory is gone; only the cache is left.
    expect(readdirSync(join(data(), "agents"))).toEqual(["claude-probe.json"]);
    // A turn that was answered or charged is no probe result: the CLI was signed in after all.
    const { binary: paid } = install({ ...UNSIGNED, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 5, outputTokens: 1, costUsd: 0.001 } }] });
    expect(await failure(probe(detection(), "h2", probeDeps(paid)))).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "unsigned" } });
    // An error result that cost something, or one that is not the CLI's "Not logged in", too.
    const errorResult = (over: Record<string, unknown>) => ({ type: "result", subtype: "success", is_error: true, session_id: "$session_id", usage: { input_tokens: 0, output_tokens: 0 }, total_cost_usd: 0, result: "Not logged in · Please run /login", ...over });
    for (const [n, over] of [[3, { total_cost_usd: 0.002 }], [4, { result: "Credit balance is too low" }], [5, { result: "$remove" }]] as const) {
      const r = { ...errorResult(over) } as Record<string, unknown>;
      if (r.result === "$remove") delete r.result;
      const { binary: b } = install({ ...UNSIGNED, steps: [{ expect: "user", emit: [r] }] });
      expect(await failure(probe(detection(), `h${n}`, probeDeps(b))), JSON.stringify(over)).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "unsigned" } });
    }
    // The control: the same line with the CLI's own text passes.
    const { binary: ok } = install({ ...UNSIGNED, steps: [{ expect: "user", emit: [errorResult({})] }] });
    expect((await run(probe(detection(), "h6", probeDeps(ok)))).initPending).toBeUndefined();
  });

  withFake("probe_prompt_has_cent_budget", async () => {
    const { binary, observed: obs } = install(UNSIGNED);
    await run(probe(detection(), "h1", probeDeps(binary)));
    const [rec] = observed(obs);
    expect(rec!.flags["--max-budget-usd"]).toBe("0.01");
    expect(rec!.flags["--permission-prompt-tool"]).toBe("stdio");
    expect(rec!.flags["--model"]).toBe("default");
  });

  withFake("probe_never_prompts_when_signed_in_by_other_means", async () => {
    // Every account but exactly `{tokenSource: "none", apiProvider: "firstParty"}` is signed in
    // (the first is the fake's default; the others what 2.1.321 answers for an API key, an
    // `apiKeyHelper`, Bedrock): no user line, ever. A user line would write the marker.
    const marker = join(tmp, "prompted");
    const accounts = [
      undefined,
      { tokenSource: "none", apiKeySource: "ANTHROPIC_API_KEY", apiProvider: "firstParty" },
      { tokenSource: "apiKeyHelper", apiProvider: "firstParty" },
      { apiProvider: "bedrock" },
      { apiProvider: "vertex" },
    ];
    for (const [n, account] of accounts.entries()) {
      const { binary, observed: obs } = install({ ...(account === undefined ? {} : { initialize: { account } }), steps: [{ expect: "user", notLoggedIn: true, writeFile: { path: marker, contents: "x" } }] });
      const r = await run(probe(detection(), `a${n}`, probeDeps(binary)));
      expect(r, JSON.stringify(account)).toMatchObject({ initPending: true, tools: [], permissionPath: "stdio" });
      const recs = observed(obs); // one file for every install: one record per probe so far
      expect(recs).toHaveLength(n + 1);
      const rec = recs[n];
      expect(rec!.controlRequests.map((c) => c.subtype)).toEqual(["initialize"]);
      expect(rec!.userLines ?? []).toEqual([]);
      expect(existsSync(marker)).toBe(false);
    }
    // The settings canary is required on this path too.
    const { binary: dropped } = install({ initialize: { output_style: "default" } });
    expect(await failure(probe(detection(), "a9", probeDeps(dropped)))).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "settings", item: "outputStyle" } });
    rmSync(probeCachePath(data()));
    const { binary } = install({ steps: [{ expect: "user", writeFile: { path: marker, contents: "x" } }] });
    const key = { version: VERSION, flagsHash: "h1", os: process.platform };
    const r = await run(probe(detection(), "h1", probeDeps(binary)));
    expect(r).toMatchObject({ initPending: true, tools: [], permissionPath: "stdio" });
    // The first real instance's init completes it; a failing init does not.
    const bad = await failure(completeProbe(data(), key, init({ tools: ["Read", "Task"] }), PLAN));
    expect(bad).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
    expect((await readProbeCache(data()))[0]!.result.initPending).toBe(true);
    const done = await run(completeProbe(data(), key, init({ apiKeySource: "/login managed key", capabilities: ["c1"] }), PLAN));
    expect(done.initPending).toBeUndefined();
    // `tools` is the CLI's set seen unsigned: a session's subset is never recorded there.
    expect(done.tools).toEqual([]);
    expect(done.capabilities).toContain("c1");
    expect((await readProbeCache(data()))[0]!.result).toEqual(done);
  });

  withFake("probe_cached_per_version_flags_os", async () => {
    const { binary, observed: obs } = install(UNSIGNED);
    const deps = probeDeps(binary);
    const first = await run(probe(detection(), "h1", deps));
    expect(first.models.find((m) => m.id === "opus")?.resolvedModel).toBe("claude-opus-5-5");
    expect(await run(probe(detection(), "h1", deps))).toEqual(first);
    expect(observed(obs)).toHaveLength(1); // the second started nothing
    await run(probe(detection(), "h2", deps));
    expect(observed(obs)).toHaveLength(2); // a new flag set probes again
    const { binary: newer } = install(UNSIGNED, "2.1.322");
    await run(probe(detection("2.1.322"), "h2", { ...deps, binary: newer }));
    expect(observed(obs)).toHaveLength(3); // a new version too
    const entries = await readProbeCache(data());
    expect(entries.map((e) => [e.version, e.flagsHash, e.os])).toEqual([
      [VERSION, "h1", process.platform],
      [VERSION, "h2", process.platform],
      ["2.1.322", "h2", process.platform],
    ]);
    // No account data in the file.
    const text = readFileSync(probeCachePath(data()), "utf8");
    for (const word of ["account", "tokenSource", "apiProvider", "firstParty", "email", "loggedIn"]) expect(text).not.toContain(word);
    // Another OS's entry is not this one's; an entry with account data is invalid and probed over.
    const file = JSON.parse(text) as { entries: { os: string; result: Record<string, unknown> }[] };
    writeFileSync(probeCachePath(data()), JSON.stringify({ entries: [{ ...file.entries[0], os: "plan9" }] }));
    await run(probe(detection(), "h1", deps));
    expect(observed(obs)).toHaveLength(4);
    writeFileSync(probeCachePath(data()), JSON.stringify({ entries: [{ ...file.entries[0], result: { ...file.entries[0]!.result, account: { tokenSource: "none" } } }] }));
    expect(await readProbeCache(data())).toEqual([]);
    await run(probe(detection(), "h1", deps));
    expect(observed(obs)).toHaveLength(5);
    // The flags hash follows the launch flags.
    expect(probeFlagsHash("linux")).toMatch(/^[0-9a-f]{16}$/);
    expect(probeFlagsHash("linux")).toBe(probeFlagsHash("linux"));
    expect(probeFlagsHash("linux")).not.toBe(probeFlagsHash("win32"));
  });

  test("flags_hash_covers_argv_settings_and_env", () => {
    const plan = instancePlan({ binary: "claude", dirs: hostDirs({ env: { TMPDIR: "/fin/agents/host-0/tmp" } }, "linux").dirs, os: "linux", uuid: UUID, resume: false, tools: ["Read"], shell: false, probe: { permissionPath: "stdio" }, model: "default", system: "s", retentionDays: 1, budgetUsd: "0.01", canary: true });
    const h = flagsHashOf(plan);
    expect(flagsHashOf({ ...plan, env: Object.fromEntries(Object.entries(plan.env).reverse()) })).toBe(h); // order is not a change
    const [k, v] = Object.entries(plan.env)[0]!;
    expect(flagsHashOf({ ...plan, env: { ...plan.env, [k]: `${v}x` } })).not.toBe(h);
    expect(flagsHashOf({ ...plan, env: { ...plan.env, CLAUDE_CODE_NEW: "1" } })).not.toBe(h);
    const settings = plan.files.findIndex((f) => f.body.includes(CANARY_STYLE));
    expect(settings).toBeGreaterThanOrEqual(0);
    const files = plan.files.map((f, i) => (i === settings ? { ...f, body: f.body.replace(CANARY_STYLE, "default") } : f));
    expect(flagsHashOf({ ...plan, files })).not.toBe(h);
    expect(flagsHashOf({ ...plan, argv: [...plan.argv, "--verbose"] })).not.toBe(h);
  });

  withFake("concurrent_probes_keep_every_entry", async () => {
    const { binary } = install(UNSIGNED);
    const deps = probeDeps(binary);
    await Promise.all(["c1", "c2", "c3"].map((h) => run(probe(detection(), h, deps))));
    expect((await readProbeCache(data())).map((e) => e.flagsHash).sort()).toEqual(["c1", "c2", "c3"]);
  });

  withFake("probe_timeout_kills_and_cleans_up", async () => {
    // The init comes, the result never does in time.
    const { binary, observed: obs } = install({ ...UNSIGNED, steps: [{ expect: "user", sleep: 30_000, notLoggedIn: true }] });
    const started = Date.now();
    expect(await failure(probe(detection(), "h1", { ...probeDeps(binary), timeoutMs: 1_000 }))).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "answer" } });
    expect(Date.now() - started).toBeLessThan(15_000);
    const [rec] = observed(obs);
    const alive = (pid: number) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch {
        return false;
      }
    };
    for (let i = 0; i < 50 && alive(rec!.pid); i++) await new Promise((r) => setTimeout(r, 100));
    expect(alive(rec!.pid)).toBe(false);
    expect(existsSync(join(data(), "agents")) ? readdirSync(join(data(), "agents")) : []).toEqual([]);
    expect(readdirSync(join(tmp, "hosts")).flatMap((p) => (existsSync(join(tmp, "hosts", p, "workspace")) ? readdirSync(join(tmp, "hosts", p, "workspace")) : []))).toEqual([]);
  });

  withFake("missing_capability_names_item", async () => {
    const missing = async (script: unknown) => {
      rmSync(join(data(), "agents", "claude-probe.json"), { force: true });
      const { binary } = install(script);
      return failure(probe(detection(), "h1", probeDeps(binary)));
    };
    expect(await missing({ ...UNSIGNED, initialize: { ...UNSIGNED.initialize, models: "$remove" } })).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "models" } });
    expect(await missing({ ...UNSIGNED, initialize: { ...UNSIGNED.initialize, capabilities: "$remove" } })).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "capabilities" } });
    expect(await missing({ ...UNSIGNED, init: { tools: "$remove" } })).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "tools" } });
    expect(await missing({ ...UNSIGNED, init: { plugins: "$remove" } })).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "plugins" } });
    expect(await missing({ ...UNSIGNED, refuseFlagValues: { "--permission-prompt-tool": ["stdio", "mcp__finisher__approve"] } })).toMatchObject({
      code: "PROVIDER_VERSION",
      details: { missing: "permissionPrompt" },
    });
    // The settings canary: a file the CLI dropped shows the default style.
    expect(await missing({ ...UNSIGNED, init: { output_style: "default" } })).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "settings", item: "outputStyle" } });
    // Another version than detected.
    const { binary } = install(UNSIGNED, "2.1.322");
    expect(await failure(probe(detection(), "h9", probeDeps(binary)))).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "version" } });
  });

  withFake("stdio_absent_selects_mcp_path", async () => {
    const { binary, observed: obs } = install({ ...UNSIGNED, refuseFlagValues: { "--permission-prompt-tool": ["stdio"] } });
    const r = await run(probe(detection(), "h1", probeDeps(binary)));
    expect(r.permissionPath).toBe("mcp");
    expect(r.tools).toContain("mcp__finisher__approve");
    const recs = observed(obs);
    // The first run was refused at its flags (before it answered `initialize`).
    expect(recs.map((x) => x.argv[x.argv.indexOf("--permission-prompt-tool") + 1])).toEqual(["stdio", "mcp__finisher__approve"]);
    expect(recs[0]!.refused).toContain("stdio");
    expect((await readProbeCache(data()))[0]!.result.permissionPath).toBe("mcp");
  });
});

// ---- the first-turn gate on a real instance ----

describe("the first-turn gate", () => {
  /** Runs `f` with a probe host sandbox opened in its own scope, closed after. */
  async function withBox<A>(f: (box: SessionSandbox) => Promise<A>): Promise<A> {
    const scope = await Effect.runPromise(Scope.make());
    try {
      const box = await Effect.runPromise(Scope.provide(scope)(openHostSandbox("probe", "claudeCode", sandboxDeps())) as Effect.Effect<SessionSandbox>);
      return await f(box);
    } finally {
      await Effect.runPromise(Scope.close(scope, Exit.void));
    }
  }

  withFake("init_mismatch_stops_instance", async () => {
    const assistant = { type: "assistant", session_id: "$session_id", parent_tool_use_id: null, message: { id: "m1", model: "claude-opus-5-5", content: [{ type: "text", text: "leaked" }] } };
    const { binary, observed: obs } = install({ init: { tools: ["Read", "Task"] }, steps: [{ expect: "user", sleep: 200, emit: [assistant] }] });
    await withBox(async (box) => {
      const { dirs } = hostDirs(box, process.platform);
      const plan = instancePlan({
        binary,
        dirs,
        os: process.platform,
        uuid: UUID,
        resume: false,
        tools: ["Read"],
        shell: false,
        probe: { permissionPath: "stdio" },
        model: "default",
        system: "role",
        retentionDays: 1,
        budgetUsd: null,
      });
      await writeLaunchFiles(plan);
      const child = await Effect.runPromise(box.spawn(plan.argv, { cwd: box.workspace, stdio: "pipe", env: plan.env }));
      const io = new InstanceLines(child, plan.mcpTools);
      const gate = firstTurnGate(plan);
      io.write(encodeControl("init-1", { subtype: "initialize" }));
      io.write(encodeUser(["hello"], UUID));
      const forwarded: CliLine[] = [];
      let refusal: FinError | null = null;
      let seen = 0;
      while (refusal === null) {
        const l = await io.next((x): x is CliLine => io.lines.indexOf(x) >= seen, 10_000);
        if ("exited" in l) throw new Error("the fake exited");
        seen++;
        const step = gate.push(l);
        if (step.kind === "forward") forwarded.push(...step.lines);
        if (step.kind === "refuse") {
          refusal = step.error;
          expect(step.kill).toBe(true);
          child.kill("SIGKILL");
        }
      }
      expect(refusal).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
      expect(gate.state).toBe("refused");
      // The instance is killed; nothing it wrote reached the caller but control lines.
      const exit = await child.exited;
      expect(exit.signal ?? exit.code).not.toBe(0);
      for (const l of io.lines.slice(seen)) {
        const step = gate.push(l);
        if (step.kind === "forward") forwarded.push(...step.lines);
      }
      expect(forwarded.every((l) => l.kind === "controlRequest" || l.kind === "controlResponse")).toBe(true);
      expect(io.lines.some((l) => l.kind === "assistant")).toBe(false);
      expect(observed(obs)[0]!.controlRequests.map((r) => r.subtype)).toEqual(["initialize"]);
    });
  });

  test("gate_releases_held_lines_after_a_passing_init", () => {
    const gate = firstTurnGate(PLAN);
    const status = decodeLine(JSON.stringify({ type: "system", subtype: "status", status: "requesting" })) as CliLine;
    expect(gate.push(status)).toEqual({ kind: "hold" });
    const i = init();
    expect(gate.push(i)).toEqual({ kind: "forward", lines: [status, i] });
    expect(gate.push(status)).toEqual({ kind: "forward", lines: [status] });
    // A result before any init: an error result passes alone, a success refuses.
    const result = (is_error: boolean) =>
      decodeLine(JSON.stringify({ type: "result", subtype: "error_during_execution", is_error, usage: { input_tokens: 0, output_tokens: 0 }, total_cost_usd: 0 })) as CliLine;
    expect(firstTurnGate(PLAN).push(result(true))).toMatchObject({ kind: "forward" });
    expect(firstTurnGate(PLAN).push(result(false))).toMatchObject({ kind: "refuse", kill: true });
  });

  const control = (subtype: string, extra: Record<string, unknown> = {}) =>
    decodeLine(JSON.stringify({ type: "control_request", request_id: `r-${subtype}`, request: { subtype, ...extra } })) as CliLine;
  const canUseTool = () => control("can_use_tool", { tool_name: "Read", input: { file_path: "/w/a" }, tool_use_id: "t1" });

  test("gate_holds_permission_and_ui_requests_until_init", () => {
    const gate = firstTurnGate(PLAN);
    const ask = canUseTool();
    const ui = control("ui_invalidate");
    expect(ask.kind).toBe("controlRequest");
    expect(gate.push(ask)).toEqual({ kind: "hold" });
    expect(gate.push(ui)).toEqual({ kind: "hold" });
    // The MCP handshake is never held.
    expect(gate.push(control("mcp_message", { server_name: "finisher", message: { method: "initialize", id: 0 } }))).toMatchObject({ kind: "forward" });
    const i = init();
    expect(gate.push(i)).toEqual({ kind: "forward", lines: [ask, ui, i] });
  });

  test("gate_checks_every_init", () => {
    const gate = firstTurnGate(PLAN);
    expect(gate.push(init())).toMatchObject({ kind: "forward" });
    // Every turn has its init: a later one that fails refuses as the first would.
    expect(gate.push(init())).toMatchObject({ kind: "forward" });
    expect(gate.push(init({ tools: ["Read", "Edit", "Bash", "Task"] }))).toMatchObject({ kind: "refuse", kill: true, error: { details: { missing: "isolation", item: "tool:Task" } } });
    expect(gate.state).toBe("refused");
    expect(gate.push(init())).toEqual({ kind: "drop" });
  });

  test("init_every_turn_rechecked: a later init equals the first, forwarded alone", () => {
    // Verified on 2.1.321 (7D Task 0, s1a/s1b/s1d): an init leads every turn, the same each time.
    const gate = firstTurnGate(PLAN);
    const status = decodeLine(JSON.stringify({ type: "system", subtype: "status", status: "requesting" })) as CliLine;
    const first = init();
    expect(gate.push(status)).toEqual({ kind: "hold" });
    expect(gate.push(first)).toEqual({ kind: "forward", lines: [status, first] });
    // The next turn's lines: nothing held twice, nothing held at all.
    const second = init();
    expect(gate.push(status)).toEqual({ kind: "forward", lines: [status] });
    expect(gate.push(second)).toEqual({ kind: "forward", lines: [second] });
    // A later init that passes the predicate but differs from the first (a model, a server's
    // status, the tool set, the mode) refuses.
    for (const [over, item] of [
      [{ model: "claude-sonnet-5-5" }, "init:model"],
      [{ tools: ["Bash", "Edit", "Read"] }, "init:tools"],
      [{ output_style: "Explanatory" }, "init:output_style"],
      [{ cwd: "/elsewhere" }, "init:cwd"],
    ] as const) {
      const g = firstTurnGate(PLAN);
      g.push(first);
      expect(g.push(init(over)), item).toMatchObject({ kind: "refuse", kill: true, error: { code: "PROVIDER_VERSION", details: { missing: "isolation", item } } });
      expect(g.state).toBe("refused");
    }
  });

  test("gate_error_result_before_init_drops_held_lines", () => {
    const gate = firstTurnGate(PLAN);
    const status = decodeLine(JSON.stringify({ type: "system", subtype: "status", status: "requesting" })) as CliLine;
    const ask = canUseTool();
    gate.push(status);
    gate.push(ask);
    const error = decodeLine(JSON.stringify({ type: "result", subtype: "error_during_execution", is_error: true, usage: { input_tokens: 0, output_tokens: 0 }, total_cost_usd: 0 })) as CliLine;
    expect(gate.push(error)).toEqual({ kind: "forward", lines: [error] });
    // Nothing held before it comes out with a later init.
    const i = init();
    expect(gate.push(i)).toEqual({ kind: "forward", lines: [i] });
  });
});
