// Claude Code crash and resume (plan 7D Task 10; Decisions 12, 17; Review Focus 5): the orphan
// sweep at boot (a CLI left running by a SIGKILLed `finisherd` in `direct` mode is killed before
// recovery ends; a recycled pid is left alone), the idle close and the lazy `--resume`, the
// resume reference, a server killed at four points of a CLI tool call (one commit wherever it
// landed, never run again), a CLI killed mid-turn, a resume refused after the end, rollback
// without the capability, and the init check on a resumed process. Everything runs `fake-claude`.
import { type ChildProcess, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import type { Node, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { sessionUuid } from "../src/agent/claude/adapter.ts";
import { claudePermissions } from "../src/agent/claude/permissions.ts";
import { decodeLine } from "../src/agent/claude/protocol.ts";
import { interruptedNote, resumeState } from "../src/agent/claude/resume.ts";
import { forgetProcess, lastEnd, PROVIDER_PID, recordProcess, sweepProviderProcesses } from "../src/agent/provider-pid.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import { processStart } from "../src/sandbox/process-start.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const LINUX = process.platform === "linux";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(WIN || (!existsSync(FAKE) && !REQUIRE));
const CHILD = fileURLToPath(new URL("./support/claude-crash-child.ts", import.meta.url));
const VERSION = "2.1.321";
const ACCOUNT = "tester@example.invalid";
const CONSOLE_KEY = { init: { apiKeySource: "/login managed key" } };
const UUID_VAR = "$session_id";

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
const children: ChildProcess[] = [];
const pids: number[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-resume-")));
});
afterEach(async () => {
  for (const c of children.splice(0)) c.kill("SIGKILL");
  for (const p of pids.splice(0)) {
    try {
      process.kill(p, "SIGKILL");
    } catch {
      // Gone.
    }
  }
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });

const assistant = (content: unknown[]) => ({ type: "assistant", session_id: UUID_VAR, parent_tool_use_id: null, message: { id: "msg_1", model: "claude-opus-5-5", role: "assistant", content } });
const toolUse = (id: string, command: string) => assistant([{ type: "tool_use", id, name: "Bash", input: { command } }]);
const toolResult = (id: string, content: unknown) => ({ type: "user", session_id: UUID_VAR, parent_tool_use_id: null, message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content, is_error: false }] } });
const PLAIN = { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] };

/** The fake installed as `<tmp>/cli/claude` with its sidecar and script; the observations in the session's config dir. */
function install(script: Record<string, unknown>): string {
  const dir = join(tmp, "cli");
  mkdirSync(dir, { recursive: true });
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION, account: ACCOUNT }));
  rescript(binary, script);
  return binary;
}
/** What the next fake process plays. */
const rescript = (binary: string, script: Record<string, unknown>) => writeFileSync(join(dirname(binary), "script.json"), JSON.stringify({ ...CONSOLE_KEY, ...script }));

type Observed = { pid: number; argv: string[]; resume?: { found: boolean; dangling?: string[] }; userLines?: { content: { type: string; text?: string }[] }[]; reran?: string[]; rerunDenied?: string[] }[];
const observed = (serverData: string, sessionId: string): Observed => {
  const p = join(serverData, "agents", sessionId, "provider", "observed.json");
  return existsSync(p) ? (JSON.parse(readFileSync(p, "utf8")) as Observed) : [];
};

interface Running {
  readonly binary: string;
  readonly local: NdjsonClient;
  readonly projectId: string;
  readonly serverData: string;
  readonly projectDir: string;
}

function settings(serverData: string, binary: string, extra: Record<string, unknown> = {}): void {
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary, ...extra } } }));
}

/** A server in this process over `serverData` (direct mode); with `project`, a new project in it. */
async function boot(serverData: string, binary: string, project = true): Promise<Running> {
  const previous = setSandboxStatusSource(NOT_READY);
  restore ??= previous;
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const projectDir = join(tmp, "P");
  let projectId = "";
  if (project) {
    projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  }
  return { binary, local, projectId, serverData, projectDir };
}

async function claudeServer(script: Record<string, unknown>, extra: Record<string, unknown> = {}): Promise<Running> {
  const binary = install(script);
  const serverData = join(tmp, "server-data");
  settings(serverData, binary, extra);
  return boot(serverData, binary);
}

const start = async (r: Running, prompt = "go") => ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt })).session;

async function until<T>(f: () => T | undefined | Promise<T | undefined>, ms = 25_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

const get = async (r: Running, sessionId: string) => ok<{ session: Session }>(await r.local.call("agent.session.get", { sessionId })).session;
const settle = (r: Running, sessionId: string) =>
  until(async () => {
    const s = await get(r, sessionId);
    return s.status === "idle" || s.status === "interrupted" || s.status === "ended" ? s : undefined;
  });
/** `agent.session.resume`, then a prompt (its run reopens the provider with `--resume=`). */
async function resumeWith(r: Running, sessionId: string, text: string): Promise<void> {
  ok(await r.local.call("agent.session.resume", { sessionId }));
  ok(await r.local.call("agent.session.prompt", { sessionId, text }));
}
const nodes = (sessionId: string): Node[] => server!.core.store.nodes(sessionId, 1, 1000) as Node[];
const errors = (ns: Node[]) => ns.filter((n) => n.kind === "error").map((n) => n.payload as { code: string; details: Record<string, unknown> });
const notices = (ns: Node[], k: string) => ns.filter((n) => n.kind === "notice" && (n.payload as Record<string, unknown>)[k] !== undefined).map((n) => (n.payload as Record<string, unknown>)[k]);
const alive = (pid: number) => processStart(pid) !== null;

/** A `sleep` of our own, its own group leader (a "recycled pid"). */
function sleeper(): number {
  const c = spawn("sleep", ["60"], { detached: true, stdio: "ignore" });
  c.unref();
  pids.push(c.pid!);
  return c.pid!;
}

// ---- units ----

describe("the provider.pid record and its sweep", () => {
  test.skipIf(!LINUX)("a recorded live process is killed; a recycled pid (another start time) is left alone", async () => {
    const dir = join(tmp, "s");
    mkdirSync(dir);
    const a = sleeper();
    const b = sleeper();
    recordProcess(dir, "01INSTANCE", a);
    const rec = JSON.parse(readFileSync(join(dir, PROVIDER_PID), "utf8")) as { processes: { pid: number; start: string }[] };
    expect(rec.processes).toEqual([{ instance: "01INSTANCE", pid: a, start: processStart(a) }]);
    // `b` stands for a pid the OS gave another process since: its start time is not the recorded one.
    writeFileSync(join(dir, PROVIDER_PID), JSON.stringify({ v: 1, processes: [...rec.processes, { instance: "01OLD", pid: b, start: "linux:1" }] }));
    const swept = sweepProviderProcesses(dir);
    expect(swept).toEqual({ killed: [a], left: [b] });
    expect(alive(a)).toBe(false);
    expect(alive(b)).toBe(true);
    expect(lastEnd(dir)).toBe("killed");
    // An ended process leaves the record, which keeps how it ended.
    recordProcess(dir, "01X", b);
    forgetProcess(dir, b, "clean");
    expect(JSON.parse(readFileSync(join(dir, PROVIDER_PID), "utf8"))).toEqual({ v: 1, processes: [], last: "clean" });
    // A recorded process gone by the sweep ended unwatched.
    recordProcess(dir, "01Y", b);
    process.kill(b, "SIGKILL");
    await until(() => (alive(b) ? undefined : true));
    expect(sweepProviderProcesses(dir)).toEqual({ killed: [], left: [b] });
    expect(lastEnd(dir)).toBe("unseen");
  });

  test("the resume state: calls announced, and the synthetic results after the CLI's last line", () => {
    const n = (seq: number, kind: string, extra: Record<string, unknown>) => ({ id: `n${seq}`, sessionId: "s", runId: "r", seq, kind, createdAt: seq, ...extra }) as unknown as Node;
    const synthetic = { ok: false, code: "SESSION_INTERRUPTED", reason: "serverRestart", synthetic: true };
    const ns = [
      n(1, "toolCall", { providerId: "toolu_1", callId: "c1", payload: { tool: "Bash", input: {} } }),
      n(2, "toolResult", { callId: "c1", payload: synthetic }),
      n(3, "assistantMessage", { payload: {} }),
      n(4, "toolCall", { providerId: "toolu_2", callId: "c2", payload: { tool: "Bash", input: {} } }),
      n(5, "toolCall", { providerId: "toolu_3", callId: "c3", payload: { tool: "Read", input: {} } }),
      n(6, "toolResult", { callId: "c3", payload: { ok: true, output: "x" } }),
      n(7, "toolResult", { callId: "c2", payload: synthetic }),
    ];
    const st = resumeState(ns);
    expect([...st.known]).toEqual(["toolu_1", "toolu_2", "toolu_3"]);
    expect(st.interrupted).toEqual([{ providerId: "toolu_2", tool: "Bash", code: "SESSION_INTERRUPTED", reason: "serverRestart" }]);
    expect(interruptedNote(st.interrupted)).toContain("toolu_2 (Bash): SESSION_INTERRUPTED (serverRestart)");
    expect(interruptedNote([])).toBeUndefined();
  });

  test("the interrupt mid-tool's result line decodes (verified 2.1.321, Task 0: `result: null`)", () => {
    const line = { type: "result", subtype: "error_during_execution", is_error: true, terminal_reason: "aborted_tools", stop_reason: "tool_use", result: null, num_turns: 1, session_id: "u", total_cost_usd: 0.01, usage: { input_tokens: 1, output_tokens: 1, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 } };
    expect(decodeLine(JSON.stringify(line))).toMatchObject({ kind: "result", is_error: true, result: null });
  });

  test("a prompt for an earlier process's call is denied at once and never reaches the runtime", () => {
    const port = claudePermissions({ rerun: (id) => id === "toolu_old" });
    const old = port.request("r1", { subtype: "can_use_tool", tool_name: "Bash", input: { command: "fin bin add" }, tool_use_id: "toolu_old" });
    expect(old.events).toEqual([{ type: "notice", name: "rerunRefused", details: { harness: "claudeCode", toolUseId: "toolu_old", tool: "Bash" } }]);
    expect(JSON.parse(old.answer!)).toMatchObject({ response: { subtype: "success", request_id: "r1", response: { behavior: "deny" } } });
    const fresh = port.request("r2", { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" }, tool_use_id: "toolu_new" });
    expect(fresh.answer).toBeUndefined();
    expect(fresh.events).toEqual([{ type: "request", id: "r2#1", kind: "approval", tool: "Bash", input: { command: "ls" } }]);
  });
});

// ---- a SIGKILLed finisherd ----

interface Crashed {
  readonly serverData: string;
  readonly projectDir: string;
  readonly binary: string;
  readonly sessionId: string;
}

/** `finisherd` in a child process runs `script`'s session to `point`, then is SIGKILLed. */
async function crashAt(script: Record<string, unknown>, point: string): Promise<Crashed> {
  const binary = install(script);
  const serverData = join(tmp, "server-data");
  settings(serverData, binary);
  const projectDir = join(tmp, "P");
  const child = spawn(process.execPath, [CHILD, serverData, projectDir, point], { cwd: dirname(CHILD), env: { ...process.env, FIN_TEST_HOOKS: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  children.push(child);
  let stderr = "";
  child.stderr!.on("data", (b: Buffer) => void (stderr = (stderr + b.toString()).slice(-4000)));
  const line = await new Promise<string>((resolve, reject) => {
    const rl = createInterface({ input: child.stdout! });
    rl.on("line", (l) => (l.startsWith("{") ? resolve(l) : undefined));
    child.once("exit", (code) => reject(new Error(`the child server ended (${code}): ${stderr}`)));
  });
  const { sessionId } = JSON.parse(line) as { sessionId: string };
  const gone = new Promise((r) => child.once("exit", r));
  child.kill("SIGKILL");
  await gone;
  return { serverData, projectDir, binary, sessionId };
}

const fakePid = (c: Crashed) => observed(c.serverData, c.sessionId)[0]!.pid;

/** After the crash: a server in this process over the same server data (its boot recovers). */
async function restart(c: Crashed): Promise<Running> {
  const r = await boot(c.serverData, c.binary, false);
  const projectId = (await get(r, c.sessionId)).projectId;
  return { ...r, projectId, projectDir: c.projectDir };
}

describe("crash and resume", () => {
  withFake("orphan_killed_at_boot: the fake's group is gone before recovery finishes; a recycled pid is left alone; the resume runs one process", async () => {
    const c = await crashAt({ steps: [{ expect: "user", emit: [toolUse("toolu_A", "sleep 600")] }, { sleep: 60_000 }] }, "toolCall");
    // Decision 17: in direct mode the CLI outlives a SIGKILLed finisherd (stdin at its end, mid-turn).
    // Verified 2.1.321, Task 0 (s5C): the CLI plays its turn out with stdin at EOF (here the
    // fake's 60 s sleep), so it is still running at the restart.
    const orphan = fakePid(c);
    expect(alive(orphan)).toBe(true);
    // A pid the OS gave another process since: recorded with another start time.
    const recycled = sleeper();
    const file = join(c.serverData, "agents", c.sessionId, PROVIDER_PID);
    const rec = JSON.parse(readFileSync(file, "utf8")) as { v: 1; processes: unknown[] };
    expect(rec.processes).toEqual([expect.objectContaining({ pid: orphan, start: processStart(orphan) })]);
    writeFileSync(file, JSON.stringify({ ...rec, processes: [...rec.processes, { instance: "01OLD", pid: recycled, start: "linux:1" }] }));

    rescript(c.binary, PLAIN);
    const r = await restart(c);
    // Recovery ran in the boot, before the server listened: the orphan is gone by now.
    expect(alive(orphan)).toBe(false);
    expect(alive(recycled)).toBe(true);
    expect((await get(r, c.sessionId)).status).toBe("interrupted");
    await resumeWith(r, c.sessionId, "continue");
    expect((await settle(r, c.sessionId)).status).toBe("idle");
    const obs = observed(r.serverData, c.sessionId);
    expect(obs).toHaveLength(2);
    expect(obs[1]!.argv).toContain(`--resume=${sessionUuid(c.sessionId)}`);
  });

  // The 7A fixture's four points, for a call the CLI runs: the server dies after the call is
  // announced, while its shell runs, after its commit landed, and after its result.
  const POINTS = {
    announce: { steps: [{ expect: "user", emit: [toolUse("toolu_A", "fin bin add")] }, { sleep: 60_000 }] },
    started: { steps: [{ expect: "user", emit: [toolUse("toolu_A", "fin bin add")] }, { spawn: { argv: ["/bin/sh", "-c", `touch started; sleep 60; '${FIN}' bin add --name late`] } }] },
    commit: { steps: [{ expect: "user", emit: [toolUse("toolu_A", "fin bin add")] }, { spawn: { argv: ["/bin/sh", "-c", `'${FIN}' bin add --name crash > bin.json`] } }, { sleep: 60_000 }] },
    result: { steps: [{ expect: "user", emit: [toolUse("toolu_A", "fin bin add")] }, { spawn: { argv: ["/bin/sh", "-c", `'${FIN}' bin add --name crash > bin.json`] }, emit: [toolResult("toolu_A", "ok")] }, { sleep: 60_000 }] },
  } as const;
  const WAIT = { announce: "toolCall", started: "started", commit: "tx", result: "toolResult" } as const;

  test.each(["announce", "started", "commit", "result"] as const)("server_killed_mid_tool_no_rerun (%s)", async (point) => {
    if (WIN || (!existsSync(FAKE) && !REQUIRE)) return;
    const c = await crashAt(POINTS[point], WAIT[point]);
    // The resumed CLI asks again for the call its transcript left without a result (modelled), and is refused.
    rescript(c.binary, { ...PLAIN, onResumeDangling: "reask" });
    const r = await restart(c);
    const committed = server!.core.store.txsOfSession(c.sessionId).length;
    expect(committed).toBe(point === "commit" || point === "result" ? 1 : 0);
    const call = nodes(c.sessionId).find((n) => n.kind === "toolCall")!;
    expect(call.providerId).toBe("toolu_A");
    const results = nodes(c.sessionId).filter((n) => n.kind === "toolResult" && n.callId === call.callId);
    expect(results).toHaveLength(1);
    expect((results[0]!.payload as { synthetic?: boolean }).synthetic === true).toBe(point !== "result");

    await resumeWith(r, c.sessionId, "continue");
    expect((await settle(r, c.sessionId)).status).toBe("idle");
    // Never run again: no new commit, the late shell of the dead process never reached a server.
    expect(server!.core.store.txsOfSession(c.sessionId).length).toBe(committed);
    const obs = observed(c.serverData, c.sessionId);
    expect(obs).toHaveLength(2);
    const resumed = obs[1]!;
    expect(resumed.argv).toContain(`--resume=${sessionUuid(c.sessionId)}`);
    expect(resumed.reran ?? []).toEqual([]);
    const first = resumed.userLines![0]!.content.map((b) => b.text ?? "").join("\n");
    if (point === "result") {
      // The CLI saw the result itself: nothing dangling, nothing to tell.
      expect(resumed.resume?.dangling).toEqual([]);
      expect(first).not.toContain("toolu_A");
    } else {
      // Its transcript holds the call; the runtime's synthetic result is the next user line.
      expect(resumed.resume?.dangling).toEqual(["toolu_A"]);
      expect(first).toContain("toolu_A (Bash): SESSION_INTERRUPTED (serverRestart)");
      expect(resumed.rerunDenied).toEqual(["toolu_A"]);
      expect(notices(nodes(c.sessionId), "rerunRefused")).toEqual([{ harness: "claudeCode", toolUseId: "toolu_A", tool: "Bash" }]);
    }
  }, 120_000);
});

describe("a tail nobody saw", () => {
  // Verified 2.1.321, Task 0 (s5C): a CLI left by a SIGKILLed host in direct mode ran its allowed
  // tool, made the next API call, finished the turn into its transcript and exited ~14 s later.
  withFake("server_killed_mid_tool_no_rerun (the CLI finished the turn unwatched): no re-run, a notice, the tail's cost charged", async () => {
    const c = await crashAt(
      {
        steps: [
          { expect: "user", emit: [toolUse("toolu_A", "fin bin add")] },
          // The shell runs on after the server died: its `fin` reaches no server.
          { spawn: { argv: ["/bin/sh", "-c", `touch started; sleep 1; '${FIN}' bin add --name late > late.json 2>&1`] }, emit: [toolResult("toolu_A", "failed"), assistant([{ type: "text", text: "done anyway" }])], cumulativeUsage: { inputTokens: 40, outputTokens: 4, costUsd: 0.04 } },
        ],
      },
      "started",
    );
    const orphan = fakePid(c);
    await until(() => (alive(orphan) ? undefined : true), 20_000);
    rescript(c.binary, { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1, costUsd: 0.001 } }] });
    const r = await restart(c);
    expect(server!.core.store.txsOfSession(c.sessionId)).toEqual([]);
    expect(lastEnd(join(c.serverData, "agents", c.sessionId))).toBe("unseen");
    await resumeWith(r, c.sessionId, "continue");
    const done = await settle(r, c.sessionId);
    expect(done.status).toBe("idle");
    const ns = nodes(c.sessionId);
    expect(notices(ns, "unseenTail")).toEqual([{ harness: "claudeCode", reason: "exitedWhileServerDown" }]);
    // The runtime closed the call itself; the CLI's transcript holds its real result, so it asks nothing again.
    const resumed = observed(c.serverData, c.sessionId)[1]!;
    expect(resumed.resume?.dangling).toEqual([]);
    expect(resumed.reran ?? []).toEqual([]);
    const first = resumed.userLines![0]!.content.map((b) => b.text ?? "").join("\n");
    expect(first).toContain("toolu_A (Bash): SESSION_INTERRUPTED");
    expect(first).toContain("ended unwatched");
    // An end nobody saw counts as a kill: all the resumed CLI reports is charged, the unseen tail included.
    expect(done.usage.moneyMicros).toBeGreaterThanOrEqual(41_000);
    expect(server!.core.store.txsOfSession(c.sessionId)).toEqual([]);
  }, 120_000);
});

describe("the resumed instance", () => {
  withFake("idle_close_then_lazy_resume: the idle process is closed, the session stays idle, the next prompt resumes it", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }, { expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }, { idleCloseS: 1 });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    const first = observed(r.serverData, s.id)[0]!.pid;
    await until(() => (alive(first) ? undefined : true), 10_000);
    // Closed for idleness, not exited: the session is still idle, with no error.
    expect((await get(r, s.id)).status).toBe("idle");
    expect(errors(nodes(s.id))).toEqual([]);
    expect(lastEnd(join(r.serverData, "agents", s.id))).toBe("clean");
    ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "again" }));
    await until(() => (nodes(s.id).filter((n) => n.kind === "usage").length === 2 ? true : undefined));
    expect((await settle(r, s.id)).status).toBe("idle");
    const obs = observed(r.serverData, s.id);
    expect(obs).toHaveLength(2);
    expect(obs[0]!.argv).toContain("--session-id");
    expect(obs[1]!.argv).toContain(`--resume=${sessionUuid(s.id)}`);
    expect(new Set(nodes(s.id).filter((n) => n.kind === "usage").map((n) => n.runId)).size).toBe(2);
  });

  withFake("resume_uses_provider_ref: --resume=<uuid> in the same provider/ directory", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", emit: [toolUse("toolu_1", "sleep 600")] }, { sleep: 60_000 }] });
    const s = await start(r);
    await until(() => (nodes(s.id).some((n) => n.kind === "toolCall") ? true : undefined));
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    expect((await settle(r, s.id)).status).toBe("interrupted");
    const ref = server!.core.store.providerRef(s.id);
    expect(JSON.parse(ref!)).toEqual({ kind: "claudeCode", id: sessionUuid(s.id) });
    rescript(r.binary, PLAIN);
    await resumeWith(r, s.id, "again");
    expect((await settle(r, s.id)).status).toBe("idle");
    // Both records are in the one config dir (`agents/<id>/provider/`): the transcript was found there.
    const obs = observed(r.serverData, s.id);
    expect(obs).toHaveLength(2);
    expect(obs[1]!.argv).toContain(`--resume=${sessionUuid(s.id)}`);
    // Verified 2.1.321, Task 0 (s1b): the interrupt gave the call the CLI's own error result in
    // its transcript, so the resumed process finds nothing dangling and no note is due.
    expect(obs[1]!.resume).toMatchObject({ found: true, dangling: [] });
  });

  withFake("cli_killed_mid_turn: PROVIDER_EXITED, interrupted, resumable", async () => {
    const r = await claudeServer({ steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "partial" }])], crash: { signal: "KILL" } }] });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("interrupted");
    expect(errors(nodes(s.id))).toEqual([expect.objectContaining({ code: "PROVIDER_EXITED", details: { code: null, signal: "SIGKILL" } })]);
    rescript(r.binary, PLAIN);
    await resumeWith(r, s.id, "again");
    expect((await settle(r, s.id)).status).toBe("idle");
    expect(observed(r.serverData, s.id)[1]!.argv).toContain(`--resume=${sessionUuid(s.id)}`);
  });

  withFake("a lost transcript is refused as such, not as a crash", async () => {
    const r = await claudeServer(PLAIN);
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    rmSync(join(r.serverData, "agents", s.id, "provider", "projects"), { recursive: true, force: true });
    ok(await r.local.call("agent.session.resume", { sessionId: s.id }));
    const e = await r.local.call("agent.session.prompt", { sessionId: s.id, text: "again" });
    expect(e.ok ? undefined : e.error).toMatchObject({ code: "PROVIDER_EXITED", details: { reason: "transcriptLost" } });
    expect((await settle(r, s.id)).status).toBe("interrupted");
    expect(errors(nodes(s.id)).at(-1)).toMatchObject({ code: "PROVIDER_EXITED", details: { reason: "transcriptLost", detail: expect.stringContaining("No conversation found") } });
  });

  withFake("resume_after_end_refused: no process starts", async () => {
    const r = await claudeServer(PLAIN);
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    ok(await r.local.call("agent.session.end", { sessionId: s.id }));
    const e = await r.local.call("agent.session.resume", { sessionId: s.id });
    expect(e.ok ? undefined : e.error.code).toBe("SESSION_ENDED");
    expect(observed(r.serverData, s.id)).toHaveLength(1);
  });

  withFake("rollback_capability_false: transactions undone, the CLI's history kept, a notice says so", async () => {
    const bin = (name: string) => ({ expect: "user", spawn: { argv: ["/bin/sh", "-c", `'${FIN}' bin add --name ${name} > ${name}.json`] }, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } });
    const r = await claudeServer({ steps: [bin("one"), bin("two")] });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "more" }));
    await until(() => (server!.core.store.txsOfSession(s.id).length === 2 ? true : undefined));
    expect((await settle(r, s.id)).status).toBe("idle");
    const rolled = ok<{ undone: string[]; conversation: string }>(await r.local.call("agent.session.rollback", { sessionId: s.id, toRun: 1 }));
    expect(rolled.undone).toHaveLength(1);
    expect(rolled.conversation).toBe("kept");
    expect(nodes(s.id).at(-1)).toMatchObject({ kind: "notice", payload: { rollback: { toRun: 1, conversation: "kept", reason: "noProviderRollback" } } });
  });

  withFake("resume_rechecks_init: Task 5's predicate refuses a resumed process", async () => {
    const r = await claudeServer(PLAIN);
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    // The resumed "version" brings a tool Finisher did not pass.
    rescript(r.binary, { init: { ...CONSOLE_KEY.init, tools: ["Read", "Task"] }, steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "never" }])] }] });
    await resumeWith(r, s.id, "again");
    expect((await settle(r, s.id)).status).toBe("interrupted");
    expect(errors(nodes(s.id)).at(-1)).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation", item: "tool:Task" } });
    expect(nodes(s.id).some((n) => n.kind === "assistantMessage")).toBe(false);
  });

  withFake("resume_rechecks_init after an idle close", async () => {
    const r = await claudeServer(PLAIN, { idleCloseS: 1 });
    const s = await start(r);
    expect((await settle(r, s.id)).status).toBe("idle");
    const first = observed(r.serverData, s.id)[0]!.pid;
    await until(() => (alive(first) ? undefined : true), 10_000);
    rescript(r.binary, { init: { ...CONSOLE_KEY.init, tools: ["Read", "Task"] }, steps: [{ expect: "user", emit: [assistant([{ type: "text", text: "never" }])] }] });
    ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "again" }));
    expect((await settle(r, s.id)).status).toBe("interrupted");
    expect(errors(nodes(s.id)).at(-1)).toMatchObject({ code: "PROVIDER_VERSION", details: { missing: "isolation" } });
  });
});
