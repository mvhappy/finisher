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
import { ClaudeCodeAdapter, sessionUuid } from "../src/agent/claude/adapter.ts";
import { accountCheck, accountKey, ClaudeAccounts } from "../src/agent/claude/account.ts";
import { CLAUDE_FLOOR } from "../src/agent/claude/detect.ts";
import { ClaudeModelCache } from "../src/agent/claude/models.ts";
import { readProbeCache } from "../src/agent/claude/probe.ts";
import * as fs from "node:fs";
import type { KeyFs } from "../src/auth/keys.ts";
import { PROVIDER_PID } from "../src/agent/provider-pid.ts";
import { processStart } from "../src/sandbox/process-start.ts";
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

/** A Console API-key sign-in's init shape (`apiKeySource` names the key's source); a claude.ai sign-in reports `none` (the fake's default). */
const CONSOLE_KEY = { init: { apiKeySource: "/login managed key" } };
/** The account the fake reports signed in (`auth status` and `initialize.account`). */
const ACCOUNT = "tester@example.invalid";
const UUID_VAR = "$session_id";

/** The fake installed as a CLI of its own: the binary, its sidecar and script; the observations in the session's config dir. */
function install(script: Record<string, unknown>, observedAt = "@config/observed.json", account = ACCOUNT): string {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify({ ...CONSOLE_KEY, ...script }));
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
    writeFileSync(join(dirname(r.binary), "script.json"), JSON.stringify({ ...CONSOLE_KEY, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }));
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
    const r = await claudeServer({ init: { ...CONSOLE_KEY.init, tools: ["Read", "Task"] }, steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "never" }])] }, { sleep: 30_000 }] });
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
  readonly spawns: { argv: readonly string[]; opts: SpawnOptions; pid: number; at: number; exitedAt?: number }[];
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
  /** A kill reaches the child this much later (the launchers' SIGHUP and reap). */
  readonly killDelayMs?: number;
  readonly idleCloseMs?: number;
  readonly closeGraceMs?: number;
  readonly exitWaitMs?: number;
  readonly pidIo?: KeyFs;
  /** The session's nodes (a resumed session's synthetic results). */
  readonly nodes?: readonly Node[];
  /** A user line's write reports an asynchronous failure (EPIPE) after its bytes were passed on. */
  readonly failUserLines?: boolean;
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
        const spawn: Watched["spawns"][number] = { argv, opts: o, pid: child.pid, at: Date.now() };
        spawns.push(spawn);
        // The bytes go to the CLI, but a user line's write reports failure (the stream stays usable).
        const epipe = () => Object.assign(new Error("write EPIPE"), { code: "EPIPE" });
        const failing = (chunk: unknown, a?: unknown, b?: unknown) => {
          const done = (typeof a === "function" ? a : b) as ((e?: Error | null) => void) | undefined;
          const user = String(chunk).includes('"type":"user"');
          return child.stdin.write(chunk as string, (e) => done?.(user ? epipe() : e));
        };
        const stdin =
          opts.failUserLines === true
            ? new Proxy(child.stdin, {
                get: (t, k) => {
                  if (k === "write") return failing;
                  const v = Reflect.get(t, k, t) as unknown;
                  return typeof v === "function" ? (v as (...x: unknown[]) => unknown).bind(t) : v;
                },
              })
            : child.stdin;
        return {
          pid: child.pid,
          stdin,
          stdout: child.stdout,
          stderr: child.stderr,
          exited: child.exited.then((e) => ((spawn.exitedAt = Date.now()), e)),
          kill: (signal = "SIGTERM") => {
            kills.push({ signal, at: Date.now() });
            if (opts.keepAlive === true) return;
            if (opts.killDelayMs === undefined) child.kill(signal);
            else setTimeout(() => child.kill(signal), opts.killDelayMs).unref();
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
    ...(opts.idleCloseMs === undefined ? {} : { idleCloseMs: opts.idleCloseMs }),
    ...(opts.closeGraceMs === undefined ? {} : { closeGraceMs: opts.closeGraceMs }),
    ...(opts.exitWaitMs === undefined ? {} : { exitWaitMs: opts.exitWaitMs }),
    ...(opts.pidIo === undefined ? {} : { pidIo: opts.pidIo }),
    ...(opts.nodes === undefined ? {} : { nodes: () => opts.nodes! }),
    enabled: () => opts.enabled ?? true,
    detect: () => Effect.succeed({ path: binary, version: VERSION, floor: CLAUDE_FLOOR, signedIn: opts.signedIn ?? true, ...(opts.accountHash === undefined ? {} : { accountHash: opts.accountHash }) }),
  });
  const ref = opts.resume === true ? { kind: "claudeCode", id: sessionUuid(s.id) } : null;
  const spec: SessionSpec = { sessionId: s.id, projectId: s.projectId, model: "default", role: "editor", mode: "draft", system: "You edit.", cwd: box.workspace, env: box.env, providerRef: ref, tools: opts.tools ?? ["Read", "Bash"], sandbox: watched };
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

describe("review of Task 7", () => {
  const TURN = { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] };

  withFake("a claude.ai sign-in (apiKeySource none, a signed-in account) passes the gate and completes the probe", async () => {
    const w = await openWatched({ ...TURN, init: { apiKeySource: "none" } });
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
    const w = await openWatched({ ...TURN, init: { ...CONSOLE_KEY.init, tools: ["Read", "Task"] } }, { tools: ["Read", "Bash", "NotATool"] });
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

  withFake("a closed instance gets its grace: a process that exits on stdin's end is not killed", async () => {
    const w = await openWatched(TURN);
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "turnCompleted");
    await w.close();
    expect(w.kills).toEqual([]);
  });

  withFake("an interrupt after close sends nothing", async () => {
    const w = await openWatched(TOOL_THEN_WAIT({ onInterrupt: "ignore", onSigint: "ignore" }), { keepAlive: true, interruptWaitMs: 5000, exitWaitMs: 300 });
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

describe("review of Task 10", () => {
  const TURN = { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] };
  /** A turn, then a process that does not notice stdin's end for a minute (the close's grace runs out). */
  const TURN_THEN_DEAF = { steps: [...TURN.steps, { sleep: 60_000 }] };
  const turns = (w: Watched) => w.events.filter((e) => e.type === "turnCompleted").length;
  const killGroup = (pid: number) => {
    try {
      process.kill(-pid, "SIGKILL");
    } catch {
      // Gone.
    }
  };

  withFake("a process that cannot be recorded in provider.pid is killed and the open fails", async () => {
    // An injected write failure: the record's path is a directory the rename cannot replace.
    const failed = await openWatched(TURN, { mayFail: true, before: (serverData) => mkdirSync(join(serverData, "agents", SESSION_ID, PROVIDER_PID, "x"), { recursive: true }) });
    expect(failed.failure).toMatchObject({ code: "INTERNAL", details: { harness: "claudeCode", reason: "processNotRecorded" } });
    expect(failed.spawns).toHaveLength(1);
    const pid = failed.spawns[0]!.pid;
    // No process left: the group was killed and its exit awaited before the failure.
    expect(failed.spawns[0]!.exitedAt).toBeDefined();
    expect(processStart(pid)).toBeNull();
  });

  /** A file layer whose writes take at most `limit` bytes per call. */
  const shortWrites = (limit: number): KeyFs => ({ ...fs, writeSync: (fd, data) => fs.writeSync(fd, data, 0, Math.min(limit, data.byteLength)) });

  withFake("second review: a short write of provider.pid is completed; one that writes nothing kills the process and fails the open", async () => {
    const w = await openWatched(TURN, { pidIo: shortWrites(10) });
    const rec = JSON.parse(readFileSync(join(w.serverData, "agents", SESSION_ID, PROVIDER_PID), "utf8")) as { processes: { pid: number }[] };
    expect(rec.processes.map((p) => p.pid)).toEqual([w.spawns[0]!.pid]);
    await w.close();
    const failed = await openWatched(TURN, { mayFail: true, pidIo: shortWrites(0) });
    expect(failed.failure).toMatchObject({ code: "INTERNAL", details: { reason: "processNotRecorded" } });
    expect(failed.spawns).toHaveLength(1);
    expect(failed.spawns[0]!.exitedAt).toBeDefined();
    expect(processStart(failed.spawns[0]!.pid)).toBeNull();
  });

  withFake("second review: a note whose line never reached the CLI is not marked told, and the next resume tells it", async () => {
    const n = (seq: number, kind: string, extra: Record<string, unknown>) => ({ id: `n${seq}`, sessionId: SESSION_ID, runId: "r", seq, kind, createdAt: seq, ...extra }) as unknown as Node;
    const ns = [n(1, "toolCall", { providerId: "toolu_X", callId: "c1", payload: { tool: "Bash", input: {} } }), n(2, "toolResult", { callId: "c1", payload: { ok: false, code: "SESSION_INTERRUPTED", reason: "serverRestart", synthetic: true } })];
    const told = (w: Watched) => w.events.filter((e) => e.type === "notice" && e.name === "interruptedDelivered");
    const first = await openWatched(TURN);
    await run(first.provider.startTurn({ runId: "r0", text: "go" }));
    await waitFor(first, "turnCompleted");
    await first.close();
    // The write reports an asynchronous failure (EPIPE): no delivery is recorded, though the
    // turn ran (the gate opened, so a delivery notice would have reached the runtime).
    const broken = await openWatched(TURN, { resume: true, nodes: ns, failUserLines: true, closeGraceMs: 200 });
    await run(broken.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(broken, "turnCompleted");
    await new Promise((r) => setTimeout(r, 200));
    expect(told(broken)).toEqual([]);
    await broken.close();
    // The crash's recovery resumes again from the same nodes: the note is sent, and now told.
    const resumed = await openWatched(TURN, { resume: true, nodes: ns });
    await run(resumed.provider.startTurn({ runId: "r2", text: "again" }));
    await until(() => (told(resumed).length === 1 ? true : undefined));
    expect(told(resumed)[0]).toMatchObject({ details: { toolUseIds: ["toolu_X"] } });
    const lines = observed(resumed.serverData, SESSION_ID).at(-1)!.userLines as { content: { text?: string }[] }[];
    expect(lines[0]!.content.map((b) => b.text ?? "").join("\n")).toContain("toolu_X (Bash): SESSION_INTERRUPTED");
  });

  withFake("the lazy resume starts only once the idle-closed process has exited", async () => {
    // The kill reaches the process 1.5 s after it is asked for (the launchers' SIGHUP, then the reap).
    const w = await openWatched(TURN_THEN_DEAF, { idleCloseMs: 200, closeGraceMs: 100, killDelayMs: 1500, exitWaitMs: 5000 });
    await run(w.provider.startTurn({ runId: "r1", text: "go" }));
    await waitFor(w, "turnCompleted");
    await until(() => (w.kills.length === 1 ? true : undefined));
    expect(w.kills[0]!.signal).toBe("SIGKILL");
    await run(w.provider.startTurn({ runId: "r2", text: "again" }));
    expect(w.spawns).toHaveLength(2);
    expect(w.spawns[1]!.argv.some((a) => a.startsWith("--resume="))).toBe(true);
    // Never two processes on one transcript: the first one's exit came before the second spawn.
    expect(w.spawns[0]!.exitedAt).toBeDefined();
    expect(w.spawns[1]!.at).toBeGreaterThanOrEqual(w.spawns[0]!.exitedAt!);
    await until(() => (turns(w) === 2 ? true : undefined));
    killGroup(w.spawns[1]!.pid);
  });

  withFake("a closed process that never exits fails the reopen instead of overlapping it", async () => {
    const w = await openWatched(TURN_THEN_DEAF, { idleCloseMs: 200, closeGraceMs: 100, keepAlive: true, exitWaitMs: 300 });
    const pid = () => w.spawns[0]!.pid;
    try {
      await run(w.provider.startTurn({ runId: "r1", text: "go" }));
      await waitFor(w, "turnCompleted");
      await until(() => (w.kills.length === 1 ? true : undefined));
      const reopened = await Effect.runPromiseExit(w.provider.startTurn({ runId: "r2", text: "again" }));
      expect(Exit.isFailure(reopened)).toBe(true);
      expect(JSON.stringify(reopened)).toContain("previousProcessAlive");
      expect(w.spawns).toHaveLength(1);
      expect(processStart(pid())).not.toBeNull();
    } finally {
      killGroup(pid());
    }
    // Gone at last: the next turn resumes.
    await until(async () => (Exit.isSuccess(await Effect.runPromiseExit(w.provider.startTurn({ runId: "r3", text: "now" }))) ? true : undefined));
    expect(w.spawns).toHaveLength(2);
    await until(() => (turns(w) === 2 ? true : undefined));
    killGroup(w.spawns[1]!.pid);
  });
});
