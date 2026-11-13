// Claude Code sessions (plan 7D Task 7; Decisions 12, 13, 16; Review Focus 1, 3, 5): the real
// runtime, store and `control.db` over `fake-claude` started through the session sandbox, in
// `direct` mode (the `agent` profile not ready) and, where Landlock enforces it, in `launcher`
// mode. Adapter-level cases (the interrupt's order, a replaced instance, the spawn's options)
// wrap the sandbox's `spawn` to see every signal and option.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Node, ProviderEvent, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope, Stream } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { ProviderSession, SessionSpec } from "../src/agent/adapter.ts";
import { ClaudeCodeAdapter, claudeAdapterFor, PENDING_ACCOUNT } from "../src/agent/claude/adapter.ts";
import { CLAUDE_FLOOR } from "../src/agent/claude/detect.ts";
import { ClaudeModelCache } from "../src/agent/claude/models.ts";
import { readProbeCache } from "../src/agent/claude/probe.ts";
import { openSessionSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps, type SpawnOptions } from "../src/agent/sandbox.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(WIN || (!existsSync(FAKE) && !REQUIRE));
const VERSION = "2.1.321";

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
let server: AgentServer | undefined;
let restore: (() => SandboxStatus) | undefined;
const cleanups: (() => Promise<void>)[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-session-")));
});
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const withAgent = (agent: SandboxStatus["agent"]): (() => SandboxStatus) => () => ({ ...sandboxStatus(), agent });
const NOT_READY = withAgent({ kind: "disabled", reason: "profileNotReady" });
/** Whether the real `agent` profile is enforced here; else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
function enforced(): boolean {
  const m = sandboxStatus().agent;
  if (m.kind === "enforced" && existsSync(FIN)) return true;
  expect(process.env.FIN_REQUIRE_LANDLOCK === "1", `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(m)}`).toBe(false);
  return false;
}

/** A signed-in CLI's init (Decision 8: `apiKeySource: "none"` while signed in is an ignored sign-in). */
const SIGNED = { init: { apiKeySource: "/login managed key" } };
const UUID_VAR = "$session_id";

/** The fake installed as a CLI of its own: the binary, its sidecar and script; the observations in the session's config dir. */
function install(script: Record<string, unknown>): string {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify({ ...SIGNED, ...script }));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION }));
  return binary;
}

type Observed = { argv: string[]; controlRequests?: { subtype: string }[]; userLines?: unknown[] }[];
const observed = (serverData: string, sessionId: string): Observed => {
  const p = join(serverData, "agents", sessionId, "provider", "observed.json");
  return existsSync(p) ? (JSON.parse(readFileSync(p, "utf8")) as Observed) : [];
};

const assistant = (content: unknown[], parent: string | null = null) => ({ type: "assistant", session_id: UUID_VAR, parent_tool_use_id: parent, message: { id: "msg_1", model: "claude-opus-5-5", role: "assistant", content } });
const toolResult = (id: string, content: unknown, isError = false) => ({ type: "user", session_id: UUID_VAR, parent_tool_use_id: null, message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content, is_error: isError }] } });
const delta = (text: string) => ({ type: "stream_event", session_id: UUID_VAR, parent_tool_use_id: null, event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } } });

// ---- the runtime over the fake ----

interface Running {
  readonly binary: string;
  readonly local: NdjsonClient;
  readonly projectId: string;
  readonly serverData: string;
  readonly projectDir: string;
}

async function claudeServer(script: Record<string, unknown>, opts: { status?: () => SandboxStatus; enabled?: boolean; path?: string } = {}): Promise<Running> {
  const previous = setSandboxStatusSource(opts.status ?? NOT_READY);
  restore ??= previous;
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const binary = opts.path ?? install(script);
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: opts.enabled ?? true, path: binary } } }));
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const projectDir = join(tmp, "P");
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  return { binary, local, projectId, serverData, projectDir };
}

async function start(r: Running, prompt = "go"): Promise<Session> {
  return ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt })).session;
}

async function until<T>(f: () => T | undefined | Promise<T | undefined>, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

async function settle(r: Running, sessionId: string): Promise<Session> {
  return until(async () => {
    const { session } = ok<{ session: Session }>(await r.local.call("agent.session.get", { sessionId }));
    return session.status === "idle" || session.status === "interrupted" || session.status === "ended" ? session : undefined;
  });
}

const nodes = (sessionId: string): Node[] => server!.core.store.nodes(sessionId, 1, 1000) as Node[];
const kinds = (ns: Node[]) => ns.map((n) => n.kind);
const notices = (ns: Node[], k: string) => ns.filter((n) => n.kind === "notice" && (n.payload as Record<string, unknown>)[k] !== undefined).map((n) => (n.payload as Record<string, unknown>)[k]);
const errors = (ns: Node[]) => ns.filter((n) => n.kind === "error").map((n) => n.payload as { code: string; fix?: string; details: Record<string, unknown> });
const runOf = (sessionId: string, runId: string) => server!.core.store.run(runId);

describe("a turn through the runtime", () => {
  const TURN = {
    steps: [
      {
        expect: "user",
        emit: [
          delta("Hel"),
          delta("lo"),
          assistant([
            { type: "thinking", thinking: "Look first.", signature: "c2ln" },
            { type: "text", text: "Hello" },
            { type: "tool_use", id: "toolu_1", name: "Read", input: { file_path: "cuts.txt" } },
          ]),
          toolResult("toolu_1", "00:01 00:04"),
          { type: "system", subtype: "api_retry", attempt: 1, retry_delay_ms: 800, error_status: 529, error: "overloaded_error", session_id: UUID_VAR },
          assistant([{ type: "text", text: "Done." }]),
        ],
        cumulativeUsage: { inputTokens: 30, outputTokens: 7, cacheReadInputTokens: 100, costUsd: 0.0042 },
        text: "Done.",
      },
    ],
  };

  async function promptToNodes(status?: () => SandboxStatus) {
    const r = await claudeServer(TURN, status === undefined ? {} : { status });
    const s = await start(r);
    const done = await settle(r, s.id);
    expect(done.status, JSON.stringify(errors(nodes(s.id)))).toBe("idle");
    const ns = nodes(s.id);
    const content = ns.filter((n) => n.kind !== "notice");
    expect(kinds(content)).toEqual(["userMessage", "reasoning", "assistantMessage", "toolCall", "toolResult", "assistantMessage", "usage"]);
    expect(content[1]!.payload).toEqual({ text: "Look first.", encrypted: { signature: "c2ln" } });
    expect(content[2]!.payload).toEqual({ role: "assistant", blocks: [{ type: "text", text: "Hello" }] });
    expect(content[3]!.payload).toEqual({ tool: "Read", input: { file_path: "cuts.txt" } });
    expect(content[4]).toMatchObject({ callId: content[3]!.callId, payload: { ok: true, output: "00:01 00:04" } });
    // The fake's sign-in is a plan (`claude.ai`): its money shown, not charged (Task 9).
    expect(content[6]!.payload).toMatchObject({ input: 30, output: 7, cacheRead: 100, moneyMicros: 4200, costBasis: "subscription" });
    expect(runOf(s.id, content[0]!.runId!)).toMatchObject({ status: "completed" });
    // api_retry_is_notice; the version is untested (no transcript suite yet, spec §4.5).
    expect(notices(ns, "retry")).toEqual([{ attempt: 1, code: 529, delayMs: 800 }]);
    expect(notices(ns, "untestedVersion")).toEqual([{ harness: "claudeCode", version: VERSION }]);
    // deltas_streamed_not_stored: the partial text is in no node.
    expect(JSON.stringify(ns)).not.toMatch(/"Hel"|"lo"/);
    // The process ran with the session's UUID and the launch plan's flags.
    const obs = observed(r.serverData, s.id);
    expect(obs).toHaveLength(1);
    expect(obs[0]!.argv).toContain("--session-id");
    expect(obs[0]!.controlRequests?.map((c) => c.subtype)).toEqual(["initialize"]);
    return { r, s };
  }

  withFake("prompt_to_nodes (direct)", async () => {
    await promptToNodes();
  });

  withFake("prompt_to_nodes (launcher)", async () => {
    if (!enforced()) return;
    await promptToNodes(withAgent(sandboxStatus().agent));
  });

  withFake("steer_stays_in_run: two results, one Finisher run", async () => {
    const r = await claudeServer({
      steps: [
        // A step sleeps before it emits: the steer arrives during the second one.
        { expect: "user", emit: [assistant([{ type: "text", text: "first" }])] },
        { sleep: 1500, cumulativeUsage: { inputTokens: 5, outputTokens: 1, costUsd: 0.001 } },
        { expect: "user", emit: [assistant([{ type: "text", text: "second" }])], cumulativeUsage: { inputTokens: 4, outputTokens: 2, costUsd: 0.001 } },
      ],
    });
    const s = await start(r);
    await until(() => (nodes(s.id).some((n) => n.kind === "assistantMessage") ? true : undefined));
    expect(ok<{ queued: boolean }>(await r.local.call("agent.session.steer", { sessionId: s.id, text: "also this" }))).toEqual({ queued: false });
    await settle(r, s.id);
    const ns = nodes(s.id);
    expect(new Set(ns.map((n) => n.runId)).size).toBe(1);
    expect(kinds(ns.filter((n) => n.kind !== "notice"))).toEqual(["userMessage", "assistantMessage", "userMessage", "usage", "assistantMessage", "usage"]);
    expect(runOf(s.id, ns[0]!.runId!)).toMatchObject({ status: "completed" });
    expect(observed(r.serverData, s.id)[0]!.userLines).toHaveLength(2);
  });

  withFake("interrupt: the tool in flight gets the synthetic result; the next prompt resumes", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", emit: [assistant([{ type: "tool_use", id: "toolu_9", name: "Bash", input: { command: "sleep 600" } }])] }, { sleep: 60_000 }] });
    const s = await start(r);
    await until(() => (nodes(s.id).some((n) => n.kind === "toolCall") ? true : undefined));
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    const after = await settle(r, s.id);
    expect(after.status).toBe("interrupted");
    const result = nodes(s.id).find((n) => n.kind === "toolResult");
    expect(result?.payload).toMatchObject({ ok: false, synthetic: true, code: "SESSION_INTERRUPTED" });
    // Interrupted through the request (the fake answers with its `result`), never a signal first.
    expect(observed(r.serverData, s.id)[0]!.controlRequests?.map((c) => c.subtype)).toEqual(["initialize", "interrupt"]);
    // The resumed process plays a plain turn.
    writeFileSync(join(dirname(r.binary), "script.json"), JSON.stringify({ ...SIGNED, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }));
    ok(await r.local.call("agent.session.resume", { sessionId: s.id }));
    ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "again" }));
    await until(() => (nodes(s.id).filter((n) => n.kind === "usage").length === 1 ? true : undefined));
    expect((await settle(r, s.id)).status).toBe("idle");
    const runs = observed(r.serverData, s.id);
    expect(runs).toHaveLength(2);
    expect(runs[1]!.argv.find((a) => a.startsWith("--resume="))).toBeDefined();
  });

  withFake("exit_mid_turn_is_provider_exited", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", emit: [delta("Partial")], crash: { signal: "KILL" } }] });
    const s = await start(r);
    const done = await settle(r, s.id);
    expect(done.status).toBe("interrupted");
    expect(errors(nodes(s.id))).toEqual([expect.objectContaining({ code: "PROVIDER_EXITED", details: { code: null, signal: "SIGKILL" } })]);
  });

  withFake("unknown_line_is_notice and unexpected_subagent_line_is_notice", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", unknownLine: true, emit: [assistant([{ type: "text", text: "from a subagent" }], "toolu_parent")], cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    const ns = nodes(s.id);
    expect(notices(ns, "unknownEvent")).toEqual(["fake_future_type"]);
    expect(notices(ns, "unexpectedSubagent")).toEqual([{ parentToolUseId: "toolu_parent" }]);
    expect(ns.some((n) => n.kind === "assistantMessage")).toBe(false);
  });

  withFake("not_logged_in_is_provider_auth_with_fix", async () => {
    const r = await claudeServer({ auth: { loggedIn: false }, init: { apiKeySource: "none" }, initialize: { account: { tokenSource: "none", apiProvider: "firstParty" } }, steps: [{ expect: "user", notLoggedIn: true }] });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("interrupted");
    const ns = nodes(s.id);
    expect(errors(ns)).toEqual([expect.objectContaining({ code: "PROVIDER_AUTH", fix: "finisherd provider signin claudeCode" })]);
    // The CLI's synthetic error message is not the agent's.
    expect(ns.some((n) => n.kind === "assistantMessage")).toBe(false);
  });

  withFake("init_mismatch_stops_instance: nothing of it reaches the runtime", async () => {
    const r = await claudeServer({ init: { ...SIGNED.init, tools: ["Read", "Task"] }, steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "never" }])] }, { sleep: 30_000 }] });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("interrupted");
    const ns = nodes(s.id);
    expect(kinds(ns.filter((n) => n.kind !== "notice" || notices([n], "sandbox").length === 0))).toEqual(["userMessage", "error"]);
    expect(errors(ns)[0]).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
  });

  withFake("fin_inside_reaches_server: the fake's shell reaches the agent listener and writes the session's root branch", async () => {
    if (!enforced()) return;
    const script = `'${FIN}' whoami > whoami.json; '${FIN}' bin add --name agentbin > bin.json`;
    const r = await claudeServer({ steps: [{ expect: "user", spawn: { argv: ["/bin/sh", "-c", script] }, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }, { status: withAgent(sandboxStatus().agent) });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    const ws = join(r.projectDir, "workspace", s.id);
    expect(readFileSync(join(ws, "whoami.json"), "utf8")).toMatch(/"kind":"agent"/);
    expect(JSON.parse(readFileSync(join(ws, "bin.json"), "utf8"))).toHaveProperty("txId");
    expect(server!.core.store.txsOfSession(s.id).length).toBeGreaterThan(0);
  });
});

describe("registration", () => {
  withFake("claude_registered_only_when_detected_and_enabled", async () => {
    // Off: the harness is unknown, the CLI never run.
    let r = await claudeServer({}, { enabled: false });
    expect(server!.agents.adapters.has("claudeCode")).toBe(false);
    const off = await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go" });
    expect(off.ok ? undefined : off.error.code).toBe("HARNESS_UNKNOWN");
    await server!.stop();
    rmSync(join(tmp, "server-data"), { recursive: true, force: true });
    rmSync(join(tmp, "P"), { recursive: true, force: true });
    // On but not detected.
    r = await claudeServer({}, { path: join(tmp, "nowhere", "claude") });
    expect(server!.agents.adapters.has("claudeCode")).toBe(false);
    await server!.stop();
    rmSync(join(tmp, "server-data"), { recursive: true, force: true });
    rmSync(join(tmp, "P"), { recursive: true, force: true });
    // On and detected.
    r = await claudeServer({});
    expect(server!.agents.adapters.has("claudeCode")).toBe(true);
  });

  test("claude_adapter_for_follows_the_provider_line", () => {
    const deps = { sandbox: {} as SessionSandboxDeps, sessions: { get: () => undefined }, models: new ClaudeModelCache(tmp) };
    expect(claudeAdapterFor({ detected: false, enabled: true }, deps)).toBeUndefined();
    expect(claudeAdapterFor({ detected: true, enabled: false }, deps)).toBeUndefined();
    expect(claudeAdapterFor({ detected: true }, deps)).toBeUndefined();
    expect(claudeAdapterFor({ detected: true, enabled: true }, deps)?.harness).toBe("claudeCode");
  });
});

// ---- the adapter over a watched sandbox ----

const SESSION_ID = "01S0000000000000000000000A";

interface Watched {
  readonly provider: ProviderSession;
  readonly events: ProviderEvent[];
  readonly kills: { signal: string; at: number }[];
  readonly spawns: { argv: readonly string[]; opts: SpawnOptions }[];
  readonly serverData: string;
  readonly binary: string;
  readonly models: ClaudeModelCache;
  close(): Promise<void>;
}

/** The adapter opened over a direct-mode sandbox whose children's signals and options are recorded (and, with `keepAlive`, never delivered). */
async function openWatched(script: Record<string, unknown>, opts: { keepAlive?: boolean; interruptWaitMs?: number; sigintGraceMs?: number; initPending?: boolean } = {}): Promise<Watched> {
  const binary = install(script);
  const serverData = join(tmp, "data");
  const projectDir = join(tmp, "P");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  mkdirSync(projectDir, { recursive: true });
  const sandboxDeps: SessionSandboxDeps = { serverData, projectDirOf: () => projectDir, finPath: "/bin/sh", localEndpoint: join(serverData, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, hostRoot: join(tmp, "hosts") };
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  const s = { id: SESSION_ID, projectId: "01P0000000000000000000000A", harness: "claudeCode", mode: "draft", role: "editor", model: "default" } as unknown as Session;
  const kills: Watched["kills"] = [];
  const spawns: Watched["spawns"] = [];
  const boxScope = await Effect.runPromise(Scope.make());
  const scope = await Effect.runPromise(Scope.make());
  cleanups.push(() => Effect.runPromise(Scope.close(boxScope, Exit.void)));
  const box = await Effect.runPromise(Scope.provide(boxScope)(openSessionSandbox(s, "claudeCode", sandboxDeps)));
  const watched: SessionSandbox = {
    ...box,
    spawn: (argv, o) =>
      Effect.map(box.spawn(argv, o), (child): SandboxedChild => {
        spawns.push({ argv, opts: o });
        return {
          pid: child.pid,
          stdin: child.stdin,
          stdout: child.stdout,
          stderr: child.stderr,
          exited: child.exited,
          kill: (signal = "SIGTERM") => {
            kills.push({ signal, at: Date.now() });
            if (opts.keepAlive !== true) child.kill(signal);
          },
        };
      }),
  };
  // No probe file there: only what the instances record answers.
  const models = new ClaudeModelCache(join(tmp, "models-only"));
  const adapter = ClaudeCodeAdapter({
    sandbox: sandboxDeps,
    sessions: { get: (id) => (id === s.id ? s : undefined) },
    models,
    interruptWaitMs: opts.interruptWaitMs ?? 300,
    sigintGraceMs: opts.sigintGraceMs ?? 300,
    detect: () => Effect.succeed({ path: binary, version: VERSION, floor: CLAUDE_FLOOR, signedIn: true }),
  });
  const spec: SessionSpec = { sessionId: s.id, projectId: s.projectId, model: "default", role: "editor", mode: "draft", system: "You edit.", cwd: box.workspace, env: box.env, providerRef: null, tools: ["Read", "Bash"], sandbox: watched };
  const exit = await Effect.runPromiseExit(Scope.provide(scope)(adapter.open(spec)));
  if (Exit.isFailure(exit)) {
    await Effect.runPromise(Scope.close(scope, Exit.void));
    throw new Error(`open failed: ${JSON.stringify(exit.cause)}`);
  }
  const provider = exit.value;
  const events: ProviderEvent[] = [];
  const pump = Effect.runPromise(Stream.runForEach(provider.events, (b) => Effect.sync(() => events.push(b.event))));
  let closed = false;
  const close = async () => {
    if (closed) return;
    closed = true;
    await Effect.runPromise(Scope.close(scope, Exit.void));
    await Promise.race([pump, new Promise((r) => setTimeout(r, 3000))]);
  };
  cleanups.push(close);
  return { provider, events, kills, spawns, serverData, binary, models, close };
}

const run = <A>(e: Effect.Effect<A, unknown>) => Effect.runPromise(e as Effect.Effect<A>);
const waitFor = (w: Watched, type: ProviderEvent["type"]) => until(() => (w.events.some((e) => e.type === type) ? true : undefined));
const TOOL_THEN_WAIT = (over: Record<string, unknown>) => ({ ...over, steps: [{ expect: "user", emit: [assistant([{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "sleep 600" } }])] }, { sleep: 60_000 }] });

describe("the interrupt (Decision 16)", () => {
  withFake("interrupt_order: the request, the result's wait, SIGINT, then the group kill; never SIGTERM", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({ onInterrupt: "ignore", onSigint: "ignore" }), { interruptWaitMs: 400, sigintGraceMs: 300 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    const t0 = Date.now();
    await run(w.provider.interrupt());
    expect(w.kills.map((k) => k.signal)).toEqual(["SIGINT", "SIGKILL"]);
    expect(w.kills[0]!.at - t0).toBeGreaterThanOrEqual(390);
    expect(w.kills[1]!.at - w.kills[0]!.at).toBeGreaterThanOrEqual(290);
    const obs = observed(w.serverData, SESSION_ID);
    expect(obs.at(-1)!.controlRequests?.map((c) => c.subtype)).toEqual(["initialize", "interrupt"]);
    await waitFor(w, "exited");
    expect(w.events.at(-1)).toEqual({ type: "exited", code: null, signal: "SIGKILL" });
  });

  withFake("an answered interrupt sends no signal: the turn fails SESSION_INTERRUPTED", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({}), { interruptWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    await run(w.provider.interrupt());
    expect(w.kills).toEqual([]);
    await waitFor(w, "turnFailed");
    expect(w.events.find((e) => e.type === "turnFailed")).toMatchObject({ error: { code: "SESSION_INTERRUPTED" } });
  });

  withFake("SIGINT ends a turn the request did not: no group kill", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({ onInterrupt: "ignore" }), { interruptWaitMs: 300, sigintGraceMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    await run(w.provider.interrupt());
    expect(w.kills.map((k) => k.signal)).toEqual(["SIGINT"]);
  });
});

describe("the instance", () => {
  withFake("replaced_instance_lines_dropped: a closed instance's later lines never reach the runtime", async () => {
    const w = await openWatched({ steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "early" }])] }, { sleep: 800, emit: [assistant([{ type: "text", text: "late" }])] }] }, { keepAlive: true });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "message");
    // The instance closes, but its process goes on (the kill is swallowed; its sandbox stays open).
    await w.close();
    expect(w.kills.map((k) => k.signal)).toContain("SIGKILL");
    const count = w.events.length;
    await new Promise((r) => setTimeout(r, 1500));
    expect(w.events.length).toBe(count);
    expect(JSON.stringify(w.events)).toContain("early");
    expect(JSON.stringify(w.events)).not.toContain("late");
    expect(w.provider.instance).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
  });

  withFake("spawn_only_through_sandbox: the gate passes, extraRead is the resolved install tree", async () => {
    // The spawn gate (scripts/check-agent-spawn.mjs) finds nothing in agent/claude/.
    const { checked, findings } = (await import(new URL("../../../scripts/check-agent-spawn.mjs", import.meta.url).href)) as { checked: (p: string) => boolean; findings: (p: string, text: string) => string[] };
    for (const f of ["adapter.ts", "session.ts", "map.ts"]) {
      const path = `apps/finisherd/src/agent/claude/${f}`;
      expect(checked(path)).toBe(true);
      expect(findings(path, readFileSync(fileURLToPath(new URL(`../src/agent/claude/${f}`, import.meta.url)), "utf8"))).toEqual([]);
    }
    const w = await openWatched({ steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    expect(w.spawns).toHaveLength(1);
    expect(w.spawns[0]!.argv[0]).toBe(realpathSync(w.binary));
    expect(w.spawns[0]!.opts.extraRead).toEqual([dirname(realpathSync(w.binary))]);
    expect(w.spawns[0]!.opts.env).toMatchObject({ CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1" });
    // Without its sandbox the adapter starts nothing.
    const adapter = ClaudeCodeAdapter({ sandbox: {} as SessionSandboxDeps, sessions: { get: () => undefined }, models: w.models });
    const spec = { sessionId: SESSION_ID, projectId: "p", model: "default", role: "editor", mode: "draft", system: "", cwd: tmp, env: {}, providerRef: null } as SessionSpec;
    const exit = await Effect.runPromiseExit(Effect.scoped(adapter.open(spec)));
    expect(JSON.stringify(exit)).toMatch(/SANDBOX_UNAVAILABLE/);
  });

  withFake("models recorded from initialize; a pending probe completed by the first init", async () => {
    const w = await openWatched({ steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    await until(async () => ((await w.models.models(VERSION)).length > 0 ? true : undefined));
    expect((await w.models.models(VERSION, PENDING_ACCOUNT)).map((m) => m.id)).toEqual(["default", "opus", "fable", "sonnet", "haiku"]);
    // The fake's `initialize` reports a sign-in: the probe sent no prompt and waits for an init.
    expect((await readProbeCache(w.serverData))[0]?.result.initPending).toBe(true);
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "turnCompleted");
    await until(async () => ((await readProbeCache(w.serverData))[0]?.result.initPending === undefined ? true : undefined));
    // The gate held `turnStarted` back until the init passed, then released it first; the
    // result's per-model split is a notice (Task 9).
    expect(w.events.map((e) => e.type)).toEqual(["turnStarted", "notice", "usage", "notice", "turnCompleted"]);
  });
});
