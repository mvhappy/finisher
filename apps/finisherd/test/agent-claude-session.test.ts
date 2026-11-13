// Claude Code sessions (plan 7D Task 7; Decisions 12, 13, 16; Review Focus 1, 3, 5): the real
// runtime, store and `control.db` over `fake-claude` started through the session sandbox, in
// `direct` mode (the `agent` profile not ready) and, where Landlock enforces it, in `launcher`
// mode. Adapter-level cases (the interrupt's order, a replaced instance, the spawn's options)
// wrap the sandbox's `spawn` to see every signal and option.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, Node, ProviderEvent, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope, Stream } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { CallGate, ProviderSession, SessionSpec } from "../src/agent/adapter.ts";
import { ClaudeCodeAdapter, sessionUuid } from "../src/agent/claude/adapter.ts";
import { accountCheck, accountKey, ClaudeAccounts } from "../src/agent/claude/account.ts";
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

// The fake's defaults are a claude.ai sign-in as 2.1.321 reports it (7D Task 0): `system/init`
// with `apiKeySource: "none"`, `initialize.account {subscriptionType, apiProvider}` plus the
// `account` knob's `email`, and an `auth status` naming no account.
/** The account the fake's `initialize.account.email` names. */
const ACCOUNT = "tester@example.invalid";
const UUID_VAR = "$session_id";

/** The fake installed as a CLI of its own: the binary, its sidecar and script; the observations in the session's config dir. */
function install(script: Record<string, unknown>, observedAt = "@config/observed.json", account = ACCOUNT): string {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify(script));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: observedAt, version: VERSION, account }));
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
    expect(notices(ns, "retry")).toEqual([{ attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 800 }]);
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

  // Verified on 2.1.321 (7D Task 0, s1b/s1d): a steer never joins the running turn, whose
  // `result` has `queued_turn_count: 0` (as the fake's now). The session holds the steer and
  // writes it at that `result` as the next CLI turn of the same run.
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

  withFake("steer_stays_in_run: two steers in one turn run as one next turn (two results in all)", async () => {
    const r = await claudeServer({
      steps: [
        { expect: "user", emit: [assistant([{ type: "text", text: "first" }])] },
        { sleep: 1500, cumulativeUsage: { inputTokens: 5, outputTokens: 1, costUsd: 0.001 } },
        { expect: "user", emit: [assistant([{ type: "text", text: "second" }])], cumulativeUsage: { inputTokens: 4, outputTokens: 2, costUsd: 0.001 } },
      ],
    });
    const s = await start(r);
    await until(() => (nodes(s.id).some((n) => n.kind === "assistantMessage") ? true : undefined));
    ok(await r.local.call("agent.session.steer", { sessionId: s.id, text: "also this" }));
    ok(await r.local.call("agent.session.steer", { sessionId: s.id, text: "and this" }));
    await settle(r, s.id);
    // A third turn (the steers played apart) would show by now.
    await new Promise((res) => setTimeout(res, 600));
    const ns = nodes(s.id);
    expect(new Set(ns.map((n) => n.runId)).size).toBe(1);
    expect(kinds(ns.filter((n) => n.kind !== "notice"))).toEqual(["userMessage", "assistantMessage", "userMessage", "userMessage", "usage", "assistantMessage", "usage"]);
    expect(runOf(s.id, ns[0]!.runId!)).toMatchObject({ status: "completed" });
    // Both steers went as one user line, after the first turn's `result`.
    const lines = observed(r.serverData, s.id)[0]!.userLines as { content: { text: string }[] }[];
    expect(lines).toHaveLength(2);
    expect(lines[1]!.content.map((b) => b.text)).toEqual(["also this", "and this"]);
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
    writeFileSync(join(dirname(r.binary), "script.json"), JSON.stringify({ steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }));
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
    const r = await claudeServer({ init: { tools: ["Read", "Task"] }, steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "never" }])] }, { sleep: 30_000 }] });
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
  withFake("registration_is_live: nothing runs at boot; the setting and detection are read per plan", async () => {
    // Off at boot: the CLI is never run (its observations would land beside it), the start is refused.
    const runs = join(tmp, "boot-observed.json");
    const binary = install({}, runs);
    let r = await claudeServer({}, { enabled: false, path: binary });
    expect(existsSync(runs)).toBe(false);
    const off = await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go" });
    expect(off.ok ? undefined : off.error.code).toBe("HARNESS_UNKNOWN");
    // Turned on while the server runs: the next start plans and opens, no HARNESS_UNKNOWN.
    writeFileSync(join(r.serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary } } }));
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    await server!.stop();
    rmSync(join(tmp, "server-data"), { recursive: true, force: true });
    rmSync(join(tmp, "P"), { recursive: true, force: true });
    // On but not installed: refused at planning.
    r = await claudeServer({}, { path: join(tmp, "nowhere", "claude") });
    const missing = await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go" });
    expect(missing.ok ? undefined : missing.error.code).toBe("HARNESS_UNKNOWN");
  });

  withFake("an open after the setting was turned off is refused", async () => {
    const failed = await openWatched({}, { enabled: false, mayFail: true });
    expect(failed.failure).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { reason: "disabled" } });
    expect(failed.spawns).toEqual([]);
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
interface WatchOptions {
  readonly keepAlive?: boolean;
  readonly interruptWaitMs?: number;
  readonly sigintGraceMs?: number;
  readonly enabled?: boolean;
  /** The role's tool allow-set (default `Read`, `Bash`). */
  readonly tools?: readonly string[];
  /** Detection's answer: signed in (default) or not. */
  readonly signedIn?: boolean;
  /** A resumed session (its provider reference set). */
  readonly resume?: boolean;
  /** Before the open (the account state, say). */
  readonly before?: (serverData: string) => void;
  /** The account the fake reports (default `ACCOUNT`). */
  readonly account?: string;
  /** Detection's account hash (its `auth status` named one). */
  readonly accountHash?: string;
  /** Detection's sign-in method (default none: not a plan sign-in). */
  readonly authMethod?: string;
  /** The runtime's budget gate for the session (Decision 15). */
  readonly gate?: CallGate;
  /** How long the `initialize` answer may take. */
  readonly initializeTimeoutMs?: number;
}

/** `openWatched` whose open may fail: then its error and what was spawned. */
async function openWatched(script: Record<string, unknown>, opts: WatchOptions & { mayFail: true }): Promise<{ failure?: { code: string; details: Record<string, unknown> }; spawns: Watched["spawns"]; w?: Watched }>;
async function openWatched(script: Record<string, unknown>, opts?: WatchOptions): Promise<Watched>;
async function openWatched(script: Record<string, unknown>, opts: WatchOptions & { mayFail?: true } = {}): Promise<Watched | { failure?: { code: string; details: Record<string, unknown> }; spawns: Watched["spawns"]; w?: Watched }> {
  const binary = install(script, undefined, opts.account);
  const serverData = join(tmp, "data");
  const projectDir = join(tmp, "P");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  mkdirSync(projectDir, { recursive: true });
  const sandboxDeps: SessionSandboxDeps = { serverData, projectDirOf: () => projectDir, finPath: "/bin/sh", localEndpoint: join(serverData, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, hostRoot: join(tmp, "hosts") };
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  opts.before?.(serverData);
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
    enabled: () => opts.enabled ?? true,
    ...(opts.initializeTimeoutMs === undefined ? {} : { initializeTimeoutMs: opts.initializeTimeoutMs }),
    detect: () => Effect.succeed({ path: binary, version: VERSION, floor: CLAUDE_FLOOR, signedIn: opts.signedIn ?? true, ...(opts.accountHash === undefined ? {} : { accountHash: opts.accountHash }), ...(opts.authMethod === undefined ? {} : { authMethod: opts.authMethod }) }),
  });
  const ref = opts.resume === true ? { kind: "claudeCode", id: sessionUuid(s.id) } : null;
  const spec: SessionSpec = { sessionId: s.id, projectId: s.projectId, model: "default", role: "editor", mode: "draft", system: "You edit.", cwd: box.workspace, env: box.env, providerRef: ref, tools: opts.tools ?? ["Read", "Bash"], sandbox: watched, ...(opts.gate === undefined ? {} : { gate: opts.gate }) };
  const exit = await Effect.runPromiseExit(Scope.provide(scope)(adapter.open(spec)));
  if (Exit.isFailure(exit)) {
    await Effect.runPromise(Scope.close(scope, Exit.void));
    if (opts.mayFail !== true) throw new Error(`open failed: ${JSON.stringify(exit.cause)}`);
    const m = /"code":"([A-Z_]+)".*?"details":(\{[^{}]*\})/.exec(JSON.stringify(exit.cause));
    return { failure: { code: m?.[1] ?? "", details: m === null ? {} : (JSON.parse(m[2]!) as Record<string, unknown>) }, spawns };
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
    // The instance closes; its process plays its turn out in the close's grace (writing "late")
    // and exits on stdin's end (the kill, were one needed, is swallowed; its sandbox stays open).
    await w.close();
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
    // Under the account's keyed hash (Task 11), the one detection reports for the same label.
    const hash = accountCheck({ signedIn: true, label: ACCOUNT }, accountKey(w.serverData), null).hash;
    expect((await w.models.models(VERSION, hash)).map((m) => m.id)).toEqual(["default", "opus", "fable", "sonnet", "haiku"]);
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

describe("steers held host-side (Decision 13 as 7D Task 0 amended it)", () => {
  const usage = { inputTokens: 1, outputTokens: 1 };
  const lines = (w: Watched) => ((observed(w.serverData, SESSION_ID).at(-1)?.userLines ?? []) as { content: { text?: string }[] }[]).map((l) => l.content.map((b) => b.text));
  const types = (w: Watched) => w.events.map((e) => e.type);
  const dropped = (w: Watched) => w.events.filter((e) => e.type === "notice" && e.name === "steerDropped").map((e) => (e as { details: unknown }).details);
  const quiet = () => new Promise((r) => setTimeout(r, 400));

  withFake("a steer right after startTurn, before the turn's init, is the run's next CLI turn", async () => {
    const w = await openWatched({ steps: [{ expect: "user", cumulativeUsage: usage }, { expect: "user", emit: [assistant([{ type: "text", text: "second" }])], cumulativeUsage: usage }] });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await run(w.provider.steer({ text: "held" }));
    await waitFor(w, "turnCompleted");
    await quiet();
    expect(types(w).filter((t) => t === "turnCompleted" || t === "usage")).toEqual(["usage", "usage", "turnCompleted"]);
    expect(JSON.stringify(w.events)).toContain("second");
    expect(lines(w)).toEqual([["one"], ["held"]]);
  });

  withFake("two steers in one turn go as one user line", async () => {
    const w = await openWatched({ steps: [{ expect: "user", sleep: 500, cumulativeUsage: usage }, { expect: "user", cumulativeUsage: usage }] });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await run(w.provider.steer({ text: "a" }));
    await run(w.provider.steer({ text: "b" }));
    await waitFor(w, "turnCompleted");
    await quiet();
    expect(types(w).filter((t) => t === "turnCompleted")).toHaveLength(1);
    expect(lines(w)).toEqual([["one"], ["a", "b"]]);
  });

  withFake("a steer reaching an instance whose run ended is refused SESSION_BUSY and never written", async () => {
    const w = await openWatched({ steps: [{ expect: "user", cumulativeUsage: usage }, { expect: "user", cumulativeUsage: usage }] });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await waitFor(w, "turnCompleted");
    const exit = await Effect.runPromiseExit(w.provider.steer({ text: "late" }));
    expect(JSON.stringify(exit)).toMatch(/SESSION_BUSY.*runEnded|runEnded.*SESSION_BUSY/);
    await quiet();
    expect(lines(w)).toEqual([["one"]]);
  });

  withFake("a failed result drops the held steer with a notice", async () => {
    const w = await openWatched({ steps: [{ expect: "user", sleep: 500, notLoggedIn: true }, { expect: "user", cumulativeUsage: usage }] });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await run(w.provider.steer({ text: "x" }));
    await waitFor(w, "turnFailed");
    await quiet();
    expect(dropped(w)).toEqual([{ harness: "claudeCode", count: 1 }]);
    // In the run: before its end.
    expect(w.events.findIndex((e) => e.type === "notice" && e.name === "steerDropped")).toBeLessThan(types(w).indexOf("turnFailed"));
    expect(lines(w)).toEqual([["one"]]);
  });

  withFake("an interrupt drops the held steer with a notice", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({}), { interruptWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    await run(w.provider.steer({ text: "x" }));
    await run(w.provider.interrupt());
    await waitFor(w, "turnFailed");
    await quiet();
    expect(dropped(w)).toEqual([{ harness: "claudeCode", count: 1 }]);
    expect(lines(w)).toEqual([["go"]]);
    expect(w.kills).toEqual([]);
  });

  withFake("an interrupt crossing a successful result drops the held steer: the run ends there", async () => {
    const w = await openWatched({ onInterrupt: "ignore", steps: [{ expect: "user", sleep: 700, cumulativeUsage: usage }, { expect: "user", cumulativeUsage: usage }] }, { interruptWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await run(w.provider.steer({ text: "x" }));
    await run(w.provider.interrupt());
    await waitFor(w, "turnCompleted");
    await quiet();
    expect(dropped(w)).toEqual([{ harness: "claudeCode", count: 1 }]);
    expect(lines(w)).toEqual([["one"]]);
    expect(w.kills).toEqual([]);
  });

  withFake("each steer turn asks the budget gate; a refusal ends the run and drops the texts", async () => {
    let calls = 0;
    const refuse = { code: "BUDGET_EXCEEDED", message: "over", details: { kind: "money" } } as unknown as FinError;
    const gate: CallGate = { beforeCall: () => (++calls === 2 ? { ok: false, error: refuse } : { ok: true }), release: () => undefined };
    const w = await openWatched({ steps: [{ expect: "user", sleep: 400, cumulativeUsage: usage }, { expect: "user", cumulativeUsage: usage }] }, { gate });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await run(w.provider.steer({ text: "x" }));
    await waitFor(w, "turnFailed");
    await quiet();
    expect(calls).toBe(2);
    expect(dropped(w)).toEqual([{ harness: "claudeCode", count: 1 }]);
    expect(w.events.find((e) => e.type === "turnFailed")).toMatchObject({ error: { code: "BUDGET_EXCEEDED" } });
    expect(types(w)).not.toContain("turnCompleted");
    expect(lines(w)).toEqual([["one"]]);
  });

  withFake("an interrupt answer still naming queued lines is a notice", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({ interruptStillQueued: [{ uuid: "q1" }] }), { interruptWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    await run(w.provider.interrupt());
    await waitFor(w, "turnFailed");
    expect(w.events.filter((e) => e.type === "notice" && e.name === "stillQueued")).toEqual([{ type: "notice", name: "stillQueued", details: { harness: "claudeCode", count: 1 } }]);
  });
});

describe("review of Task 7", () => {
  const TURN = { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] };

  withFake("a claude.ai sign-in (apiKeySource none, a signed-in account) passes the gate and completes the probe", async () => {
    // As 2.1.321 reports it on a first start (7D Task 0, s1a): `initialize.account
    // {subscriptionType, apiProvider}`, no e-mail yet, `apiKeySource: "none"`.
    const w = await openWatched({ ...TURN, init: { apiKeySource: "none" }, initialize: { account: { email: "$remove" } } }, { authMethod: "claude.ai" });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "turnCompleted");
    expect(w.events.some((e) => e.type === "turnFailed")).toBe(false);
    await until(async () => ((await readProbeCache(w.serverData))[0]?.result.initPending === undefined ? true : undefined));
  });

  withFake("an ignored sign-in (signed in, initialize exactly unsigned, apiKeySource none) is refused oauth", async () => {
    // Unsigned as the probe sees it too (its one turn is the "Not logged in" one).
    const w = await openWatched({ steps: [{ expect: "user", notLoggedIn: true }], init: { apiKeySource: "none" }, initialize: { account: { tokenSource: "none", apiProvider: "firstParty" } } }, { accountHash: "0123abcd" });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "exited");
    expect(w.events.map((e) => e.type)).toEqual(["turnFailed", "exited"]);
    expect(w.events[0]).toMatchObject({ error: { code: "PROVIDER_VERSION", details: { missing: "oauth" } } });
  });

  withFake("a plan sign-in whose initialize.account names no plan is refused oauth", async () => {
    const w = await openWatched({ ...TURN, initialize: { account: { subscriptionType: "$remove" } } }, { authMethod: "claude.ai" });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "exited");
    expect(w.events.map((e) => e.type)).toEqual(["turnFailed", "exited"]);
    expect(w.events[0]).toMatchObject({ error: { code: "PROVIDER_VERSION", details: { missing: "oauth", item: "account" } } });
  });

  withFake("init_every_turn_rechecked: each turn's init passes once; a later one that differs stops the instance", async () => {
    const usage = { inputTokens: 1, outputTokens: 1 };
    const w = await openWatched({
      steps: [
        { expect: "user", cumulativeUsage: usage },
        { expect: "user", cumulativeUsage: usage },
        { expect: "user", init: { agents: ["claude", "Explore"] }, emit: [assistant([{ type: "text", text: "never" }])], cumulativeUsage: usage },
      ],
    });
    const completed = (n: number) => until(() => (w.events.filter((e) => e.type === "turnCompleted").length === n ? true : undefined));
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await completed(1);
    await run(w.provider.startTurn({ runId: "r2", text: "two" }));
    await completed(2);
    // Nothing held or released twice: the untested-version notice once, each turn's events once.
    expect(w.events.map((e) => e.type)).toEqual(["turnStarted", "notice", "usage", "notice", "turnCompleted", "turnStarted", "usage", "notice", "turnCompleted"]);
    await run(w.provider.startTurn({ runId: "r3", text: "three" }));
    await waitFor(w, "exited");
    expect(w.events.slice(9).map((e) => e.type)).toEqual(["turnStarted", "turnFailed", "exited"]);
    expect(w.events[10]).toMatchObject({ error: { code: "PROVIDER_VERSION", details: { missing: "isolation", item: "init:agents" } } });
    expect(JSON.stringify(w.events)).not.toContain("never");
  });

  withFake("init_every_turn_rechecked: a later init naming another model is a notice, not a refusal", async () => {
    const usage = { inputTokens: 1, outputTokens: 1 };
    const w = await openWatched({ steps: [{ expect: "user", cumulativeUsage: usage }, { expect: "user", init: { model: "claude-sonnet-5-5" }, cumulativeUsage: usage }] });
    await run(w.provider.startTurn({ runId: "r1", text: "one" }));
    await waitFor(w, "turnCompleted");
    await run(w.provider.startTurn({ runId: "r2", text: "two" }));
    await until(() => (w.events.filter((e) => e.type === "turnCompleted").length === 2 ? true : undefined));
    expect(w.events.some((e) => e.type === "turnFailed")).toBe(false);
    expect(w.events.filter((e) => e.type === "notice" && e.name === "modelChanged")).toEqual([
      { type: "notice", name: "modelChanged", details: { harness: "claudeCode", from: expect.any(String), to: "claude-sonnet-5-5" } },
    ]);
  });

  withFake("an initialize that never answers fails the open closed (no sign-in or account check could run)", async () => {
    // The probe (cached per version) answered once; the instance's CLI then stays silent.
    await (await openWatched({ steps: [] })).close();
    const refused = await openWatched({ initializeSilent: true, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }, { mayFail: true, initializeTimeoutMs: 600 });
    expect(refused.failure).toMatchObject({ code: "PROVIDER_PROTOCOL", details: { reason: "initializeTimeout" } });
    expect(refused.spawns).toHaveLength(1);
  });

  withFake("can_use_tool is never answered on the adapter's own: it waits for the runtime's decision", async () => {
    const w = await openWatched({
      steps: [
        {
          expect: "user",
          ask: { tool: "Bash", input: { command: "ls" } },
          onAllow: [toolResult("toolu_fake_1", "ran")],
          onDeny: [toolResult("toolu_fake_1", "denied", true)],
          cumulativeUsage: { inputTokens: 1, outputTokens: 1 },
        },
      ],
    });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "request");
    const answered = () =>
      ((observed(w.serverData, SESSION_ID).at(-1) as { controlResponses?: { raw: string }[] } | undefined)?.controlResponses ?? [])
        .map((a) => JSON.parse(a.raw) as { response: { response?: { behavior?: string } } })
        .find((a) => a.response.response?.behavior !== undefined);
    // Nothing answered while the runtime has not decided (Task 8's port; Task 7's interim one denied at once).
    await new Promise((r) => setTimeout(r, 300));
    expect(answered()).toBeUndefined();
    expect(w.events.some((e) => e.type === "toolCompleted")).toBe(false);
    const req = w.events.find((e) => e.type === "request") as { id: string };
    await run(w.provider.respond(req.id, { kind: "approval", allow: false, message: "denied by the runtime" }));
    await waitFor(w, "turnCompleted");
    expect(answered()?.response.response?.behavior).toBe("deny");
    expect(w.events.filter((e) => e.type === "toolCompleted")).toEqual([{ type: "toolCompleted", providerId: "toolu_fake_1", ok: false, output: "denied" }]);
  });

  withFake("a refused init lets none of the adapter's own events through", async () => {
    // `NotATool` maps to no CLI tool: a `toolDropped` notice the gate holds.
    const w = await openWatched({ ...TURN, init: { tools: ["Read", "Task"] } }, { tools: ["Read", "Bash", "NotATool"] });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "exited");
    expect(w.events.map((e) => e.type)).toEqual(["turnFailed", "exited"]);
    expect(w.events[0]).toMatchObject({ error: { code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } } });
  });

  withFake("account: a new session is refused while a change waits; a running session's resume is not", async () => {
    // The first session runs on ACCOUNT (recorded as the account) and leaves its transcript.
    const first = await openWatched(TURN);
    await run(first.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(first, "turnCompleted");
    await first.close();
    const recorded = new ClaudeAccounts(first.serverData).state().current?.hash;
    expect(recorded).toBe(accountCheck({ signedIn: true, label: ACCOUNT }, accountKey(first.serverData), null).hash);
    // The CLI is signed in to another account: the session's resume opens (the change is recorded) ...
    const resumed = await openWatched(TURN, { account: "other@example.invalid", resume: true });
    expect(resumed.spawns[0]!.argv.some((a) => a.startsWith("--resume="))).toBe(true);
    const waiting = new ClaudeAccounts(first.serverData).pending();
    expect(waiting).toBe(accountCheck({ signedIn: true, label: "other@example.invalid" }, accountKey(first.serverData), null).hash);
    await resumed.close();
    // ... a new session is refused until a person confirms.
    const refused = await openWatched(TURN, { account: "other@example.invalid", mayFail: true });
    expect(refused.failure).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { accountHash: waiting } });
  });

  withFake("account: a first start naming no account opens and records nothing; the resumed process records it", async () => {
    // 2.1.321 (7D Task 0): a fresh config dir's first `initialize` often lacks `email`, and
    // detection's `auth status` names none (no hash): no refusal, nothing recorded.
    const first = await openWatched({ ...TURN, initialize: { account: { email: "$remove" } } }, { authMethod: "claude.ai" });
    await run(first.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(first, "turnCompleted");
    await first.close();
    expect(new ClaudeAccounts(first.serverData).state()).toEqual({ v: 1 });
    // The CLI holds the profile by its next process: `initialize.account.email` names it.
    const resumed = await openWatched(TURN, { resume: true, authMethod: "claude.ai" });
    await resumed.close();
    expect(new ClaudeAccounts(first.serverData).state().current?.hash).toBe(accountCheck({ signedIn: true, label: ACCOUNT }, accountKey(first.serverData), null).hash);
  });

  withFake("account: naming no account, a new session is still refused while a change waits", async () => {
    let waiting = "";
    const refused = await openWatched(
      { ...TURN, initialize: { account: { email: "$remove" } } },
      {
        authMethod: "claude.ai",
        mayFail: true,
        before: (data) => {
          const accounts = new ClaudeAccounts(data);
          accounts.observe({ signedIn: true, authMethod: "claude.ai", label: ACCOUNT }, accountKey(data));
          waiting = accounts.observe({ signedIn: true, authMethod: "claude.ai", label: "other@example.invalid" }, accountKey(data)).hash;
        },
      },
    );
    expect(refused.failure).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { accountHash: waiting } });
  });

  withFake("a closed instance gets its grace: a process that exits on stdin's end is not killed", async () => {
    const w = await openWatched(TURN);
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "turnCompleted");
    await w.close();
    expect(w.kills).toEqual([]);
  });

  withFake("an interrupt after close sends nothing", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({ onInterrupt: "ignore", onSigint: "ignore" }), { keepAlive: true, interruptWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "toolStarted");
    const interrupted = run(w.provider.interrupt());
    await new Promise((r) => setTimeout(r, 100));
    const t0 = Date.now();
    await w.close();
    await interrupted;
    // The wait ended with the close (no SIGINT); only the close's own kill after its grace.
    expect(w.kills.map((k) => k.signal)).toEqual(["SIGKILL"]);
    expect(Date.now() - t0).toBeLessThan(4000);
  });
});
