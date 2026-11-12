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
import { checkInit, completeProbe, firstTurnGate, hostDirs, InstanceLines, type ProbeDeps, probe, probeCachePath, probeFlagsHash, readProbeCache, untestedVersionNotices } from "../src/agent/claude/probe.ts";
import { checkInitialize, mcpReply } from "../src/agent/claude/probe-check.ts";
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

type Observed = { argv: string[]; refused?: string; flags: Record<string, unknown>; envNames: string[]; settings: string; controlRequests: { subtype: string }[] }[];
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
  test("oauth_ignored_refused", () => {
    expect(verdict(init())).toBeNull();
    expect(verdict(init(), { signedIn: true })).toMatchObject({ code: "PROVIDER_VERSION", missing: "oauth" });
    expect(verdict(init({ apiKeySource: "/login managed key" }), { signedIn: true })).toBeNull();
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
    const body = { models: [{ value: "default" }], commands: [{ name: "review" }], capabilities: [], claude_code_version: VERSION, account: { tokenSource: "none" } };
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
    expect(existsSync(join(data(), "providers"))).toBe(false);
    // The host sandbox's directory is gone; only the cache is left.
    expect(readdirSync(join(data(), "agents"))).toEqual(["claude-probe.json"]);
    // A turn that was answered or charged is no probe result: the CLI was signed in after all.
    const { binary: paid } = install({ ...UNSIGNED, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 5, outputTokens: 1, costUsd: 0.001 } }] });
    expect(await failure(probe(detection(), "h2", probeDeps(paid)))).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "unsigned" } });
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
    // The fake's `initialize` reports `tokenSource: "claude.ai"` by default; a user line would write the marker.
    const marker = join(tmp, "prompted");
    const { binary } = install({ steps: [{ expect: "user", writeFile: { path: marker, contents: "x" } }] });
    const key = { version: VERSION, flagsHash: "h1", os: process.platform };
    const r = await run(probe(detection(), "h1", probeDeps(binary)));
    expect(r).toMatchObject({ initPending: true, tools: [], permissionPath: "stdio" });
    expect(existsSync(marker)).toBe(false);
    // The first real instance's init completes it; a failing init does not.
    const bad = await failure(completeProbe(data(), key, init({ tools: ["Read", "Task"] }), PLAN, { signedIn: true }));
    expect(bad).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
    expect((await readProbeCache(data()))[0]!.result.initPending).toBe(true);
    const done = await run(completeProbe(data(), key, init({ apiKeySource: "/login managed key" }), PLAN, { signedIn: true }));
    expect(done.initPending).toBeUndefined();
    expect(done.tools).toEqual(["Read", "Edit", "Bash"]);
    expect((await readProbeCache(data()))[0]!.result).toEqual(done);
  });

  withFake("probe_cached_per_version_flags_os", async () => {
    const { binary, observed: obs } = install(UNSIGNED);
    const deps = probeDeps(binary);
    const first = await run(probe(detection(), "h1", deps));
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
      const gate = firstTurnGate(plan, { signedIn: true });
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
          io.write(encodeControl("stop-1", { subtype: "interrupt" }));
        }
      }
      expect(refusal).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
      expect(gate.state).toBe("refused");
      // The interrupt is answered; nothing the instance wrote reached the caller but control lines.
      const answered = await io.next((x): x is CliLine => x.kind === "controlResponse" && x.response.request_id === "stop-1", 10_000);
      expect("exited" in answered).toBe(false);
      for (const l of io.lines.slice(seen)) {
        const step = gate.push(l);
        if (step.kind === "forward") forwarded.push(...step.lines);
      }
      expect(forwarded.every((l) => l.kind === "controlRequest" || l.kind === "controlResponse")).toBe(true);
      expect(io.lines.some((l) => l.kind === "assistant")).toBe(false);
      expect(observed(obs)[0]!.controlRequests.map((r) => r.subtype)).toEqual(["initialize", "interrupt"]);
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
    expect(firstTurnGate(PLAN).push(result(false))).toMatchObject({ kind: "refuse", interrupt: true });
  });
});
