// Claude Code usage, cost and budgets (plan 7D Task 9; Decision 15; Review Focus 4): the
// adapter's usage memory (`claude/usage.ts`) on its own, then the real runtime, store and
// `control.db` over `fake-claude` (direct mode). The CLI's shapes are fake-claude's model of
// them (TODO(7D Task 0 Step 1) in `usage.ts`): cumulative `result` totals, restored or reset on
// `--resume`, Messages-API `stream_event` usage, `error_max_budget_usd`.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Node, ProviderEvent, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Scope, Stream } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { CallGate, ProjectedUsage, SessionSpec } from "../src/agent/adapter.ts";
import { ClaudeCodeAdapter } from "../src/agent/claude/adapter.ts";
import { CLAUDE_FLOOR } from "../src/agent/claude/detect.ts";
import { resultError } from "../src/agent/claude/map.ts";
import { ClaudeModelCache } from "../src/agent/claude/models.ts";
import { decodeLine, type ResultMsg, type StreamPayload } from "../src/agent/claude/protocol.ts";
import { ClaudeUsage, remainingIn, turnProjection, ZERO_TOTALS } from "../src/agent/claude/usage.ts";
import { finError } from "../src/errors.ts";
import { openSessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { readAgentSettings } from "../src/agent/settings.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(WIN || (!existsSync(FAKE) && !REQUIRE));
const VERSION = "2.1.321";
const UUID_VAR = "$session_id";

// ---- the usage memory on its own ----

const result = (input: number, output: number, costUsd: number, over: Record<string, unknown> = {}): ResultMsg => {
  const l = decodeLine(JSON.stringify({ type: "result", subtype: "success", is_error: false, session_id: "s", total_cost_usd: costUsd, usage: { input_tokens: input, output_tokens: output }, ...over }));
  if (!("kind" in l) || l.kind !== "result") throw new Error("not a result");
  return l;
};
const stream = (event: Record<string, unknown>): StreamPayload => {
  const l = decodeLine(JSON.stringify({ type: "stream_event", session_id: "s", parent_tool_use_id: null, event }));
  if (!("kind" in l) || l.kind !== "stream") throw new Error("not a stream event");
  return l.event;
};
const usageOf = (events: ProviderEvent[]) => events.filter((e) => e.type === "usage");
const ev = (input: number, output: number, costMicros: number, costBasis: "reported" | "subscription" = "reported") => ({ type: "usage", input, output, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros, costBasis });

describe("usage memory", () => {
  test("usage_is_delta_per_turn: cumulative totals become what each result added", () => {
    const u = new ClaudeUsage();
    expect(usageOf(u.usage(result(100, 10, 0.01)))).toEqual([ev(100, 10, 10_000)]);
    expect(usageOf(u.usage(result(150, 30, 0.03)))).toEqual([ev(50, 20, 20_000)]);
    expect(usageOf(u.usage(result(150, 30, 0.03)))).toEqual([]);
  });

  test("live stream usage is charged at once; the result adds only the rest", () => {
    const u = new ClaudeUsage();
    expect(u.stream(stream({ type: "message_start", message: { id: "m1", usage: { input_tokens: 40, output_tokens: 1, cache_read_input_tokens: 3 } } }))).toEqual({ ...ev(40, 1, 0), cacheRead: 3 });
    expect(u.stream(stream({ type: "message_delta", usage: { output_tokens: 9 } }))).toEqual(ev(0, 8, 0));
    // A cumulative delta that repeats the input side charges it once.
    expect(u.stream(stream({ type: "message_delta", usage: { input_tokens: 40, output_tokens: 9 } }))).toBeNull();
    expect(u.stream(stream({ type: "message_stop" }))).toBeNull();
    expect(u.stream(stream({ type: "message_start", message: { id: "m2", usage: { input_tokens: 5 } } }))).toEqual(ev(5, 0, 0));
    // The result's totals hold 2 tokens the stream never showed, and the turn's money.
    expect(usageOf(u.usage(result(47, 9, 0.004, { usage: { input_tokens: 47, output_tokens: 9, cache_read_input_tokens: 3 } })))).toEqual([ev(2, 0, 4_000)]);
  });

  const BASE = { input: 1000, output: 100, cacheRead: 0, cacheWrite: 0, costMicros: 50_000 };

  test("delta_after_resume: restored totals (pinned) charge only what the turn added", () => {
    const u = new ClaudeUsage({ baseline: BASE, resumed: true, resumeTotals: "restored" });
    expect(u.stream(stream({ type: "message_start", message: { usage: { input_tokens: 30 } } }))).toEqual(ev(30, 0, 0));
    expect(usageOf(u.usage(result(1030, 105, 0.06)))).toEqual([ev(0, 5, 10_000)]);
    expect(usageOf(u.usage(result(1050, 110, 0.07)))).toEqual([ev(20, 5, 10_000)]);
  });

  test("delta_after_resume: reset totals count from zero", () => {
    const u = new ClaudeUsage({ baseline: BASE, resumed: true });
    expect(u.stream(stream({ type: "message_start", message: { usage: { input_tokens: 30 } } }))).toEqual(ev(30, 0, 0));
    expect(usageOf(u.usage(result(30, 5, 0.01)))).toEqual([ev(0, 5, 10_000)]);
    expect(usageOf(u.usage(result(50, 10, 0.02)))).toEqual([ev(20, 5, 10_000)]);
  });

  test("delta_after_resume fails closed: a resumed instance's totals are charged whole until Task 0 pins the mode", () => {
    // The review's repro: a reset turn that reports exactly the earlier totals, no live usage.
    // Read as restored it charged nothing, and every repeat of the resume was free.
    let baseline = BASE;
    let spent = 0;
    for (let resume = 0; resume < 3; resume++) {
      const charged = usageOf(new ClaudeUsage({ baseline, resumed: true }).usage(result(1000, 100, 0.05)));
      expect(charged).toEqual([ev(1000, 100, 50_000)]);
      for (const e of charged) spent += e.type === "usage" ? (e.costMicros ?? 0) : 0;
      baseline = { ...baseline, input: baseline.input + 1000, output: baseline.output + 100, costMicros: baseline.costMicros + 50_000 };
    }
    expect(spent).toBe(150_000);
    // A turn larger than everything before: charged whole, not the turn minus the baseline.
    expect(usageOf(new ClaudeUsage({ baseline: BASE, resumed: true }).usage(result(2000, 200, 0.2)))).toEqual([ev(2000, 200, 200_000)]);
    // Live usage first: the result adds only what the stream did not show.
    const live = new ClaudeUsage({ baseline: BASE, resumed: true });
    expect(live.stream(stream({ type: "message_start", message: { usage: { input_tokens: 1200 } } }))).toEqual(ev(1200, 0, 0));
    expect(usageOf(live.usage(result(1000, 100, 0.05)))).toEqual([ev(0, 100, 50_000)]);
    // Pinned (Task 0 decides): restored counts from the baseline.
    expect(usageOf(new ClaudeUsage({ baseline: BASE, resumed: true, resumeTotals: "restored" }).usage(result(2000, 200, 0.2)))).toEqual([ev(1000, 100, 150_000)]);
    expect(usageOf(new ClaudeUsage({ baseline: BASE, resumed: true, resumeTotals: "restored" }).usage(result(30, 5, 0.01)))).toEqual([]);
    // A fresh transcript (`--session-id`) counts from zero whatever the session spent.
    expect(usageOf(new ClaudeUsage({ baseline: BASE, resumed: false }).usage(result(30, 5, 0.01)))).toEqual([ev(30, 5, 10_000)]);
  });

  test("per_model_split: modelUsage against the last one seen", () => {
    const u = new ClaudeUsage();
    const mu = (a: number, b: number) => ({ modelUsage: { "claude-opus-5-5": { inputTokens: a, outputTokens: 1, costUSD: a / 1000 }, "claude-haiku-5-5": { inputTokens: b, outputTokens: 0 } } });
    const split = (events: ProviderEvent[]) => events.find((e) => e.type === "notice" && e.name === "modelUsage");
    expect(split(u.usage(result(15, 1, 0.01, mu(10, 5))))).toEqual({ type: "notice", name: "modelUsage", details: { "claude-opus-5-5": { input: 10, output: 1, cacheRead: 0, cacheWrite: 0, costMicros: 10_000 }, "claude-haiku-5-5": { input: 5, output: 0, cacheRead: 0, cacheWrite: 0, costMicros: 0 } } });
    expect(split(u.usage(result(25, 1, 0.02, mu(20, 5))))).toEqual({ type: "notice", name: "modelUsage", details: { "claude-opus-5-5": { input: 10, output: 0, cacheRead: 0, cacheWrite: 0, costMicros: 10_000 } } });
    // A restored instance's first split with several models is unknown: none rather than a wrong one.
    const r = new ClaudeUsage({ baseline: { ...ZERO_TOTALS, input: 15, output: 1, costMicros: 10_000 }, resumed: true, resumeTotals: "restored" });
    expect(split(r.usage(result(25, 1, 0.02, mu(20, 5))))).toBeUndefined();
  });

  test("cost basis and the per-turn projection", () => {
    expect(usageOf(new ClaudeUsage({ basis: "subscription" }).usage(result(10, 1, 0.01)))).toEqual([ev(10, 1, 10_000, "subscription")]);
    expect(turnProjection(null)).toMatchObject({ output: 8192, input: 0 });
    expect(turnProjection(500)).toMatchObject({ output: 500 });
    expect(turnProjection(0)).toMatchObject({ output: 0 });
    const ledger = { read: (_: string, k: "tokens" | "money") => (k === "tokens" ? { limit: 1000, used: 300, reserved: 200 } : { limit: null, used: 5, reserved: 0 }) };
    expect(remainingIn(() => ledger)("s")).toEqual({ tokens: 500, moneyMicros: null });
  });

  test("max_budget_backstop_maps_to_budget_exceeded (the result's own error)", () => {
    expect(resultError(result(1, 1, 0.02, { subtype: "error_max_budget_usd", is_error: true }), false)).toMatchObject({ code: "BUDGET_EXCEEDED", details: { kind: "money" } });
  });

  test("agents.claudeCode.chargePlanMoney is read", () => {
    const dir = realpathSync(mkdtempSync(join(tmpdir(), "claude-budget-settings-")));
    try {
      writeFileSync(join(dir, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, chargePlanMoney: true } } }));
      expect(readAgentSettings(dir).settings.claudeCode).toEqual({ enabled: true, chargePlanMoney: true });
      writeFileSync(join(dir, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, chargePlanMoney: "yes" } } }));
      const bad = readAgentSettings(dir);
      expect(bad.settings.claudeCode).toEqual({ enabled: true });
      expect(bad.error).toMatch(/chargePlanMoney/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

// ---- over fake-claude ----

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
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-budget-")));
});
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = () => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } }) as SandboxStatus;
const SIGNED = { init: { apiKeySource: "/login managed key" } };
/** An API-key sign-in (`console`): money reported and charged, with the backstop. */
const CONSOLE = { auth: { authMethod: "console" } };

function install(script: Record<string, unknown>): string {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify({ ...SIGNED, ...script }));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION }));
  return binary;
}
const rescript = (binary: string, script: Record<string, unknown>) => writeFileSync(join(dirname(binary), "script.json"), JSON.stringify({ ...SIGNED, ...script }));

type Observed = { argv: string[]; controlRequests?: { subtype: string }[]; userLines?: unknown[] }[];
const observed = (serverData: string, sessionId: string): Observed => {
  const p = join(serverData, "agents", sessionId, "provider", "observed.json");
  return existsSync(p) ? (JSON.parse(readFileSync(p, "utf8")) as Observed) : [];
};
const flag = (argv: string[]) => {
  const i = argv.indexOf("--max-budget-usd");
  return i < 0 ? undefined : argv[i + 1];
};

interface Running {
  readonly binary: string;
  readonly local: NdjsonClient;
  readonly projectId: string;
  readonly serverData: string;
}

async function claudeServer(script: Record<string, unknown>, claudeCode: Record<string, unknown> = {}): Promise<Running> {
  const previous = setSandboxStatusSource(NOT_READY);
  restore ??= previous;
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const binary = install(script);
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary, ...claudeCode } } }));
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  return { binary, local, projectId, serverData };
}

async function start(r: Running, budget?: Record<string, number>): Promise<Session> {
  return ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go", ...(budget === undefined ? {} : { budget }) })).session;
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

const get = (sessionId: string): Session => server!.core.store.get(sessionId)!;
async function settle(sessionId: string): Promise<Session> {
  return until(() => {
    const s = get(sessionId);
    return s.status === "idle" || s.status === "interrupted" || s.status === "ended" ? s : undefined;
  });
}
const nodes = (sessionId: string): Node[] => server!.core.store.nodes(sessionId, 1, 1000) as Node[];
const usages = (sessionId: string) => nodes(sessionId).filter((n) => n.kind === "usage").map((n) => n.payload as Record<string, unknown>);
const ledger = (sessionId: string, kind: "tokens" | "money") => server!.core.store.ledger().read(sessionId, kind);
const errors = (ns: Node[]) => ns.filter((n) => n.kind === "error").map((n) => n.payload as { code: string; details: Record<string, unknown> });

/** Interrupts `s` (its instance closes), resumes it and prompts again: a second instance with `--resume=`. */
async function restart(r: Running, s: Session, script: Record<string, unknown>): Promise<Session> {
  const before = get(s.id).status;
  if (before !== "interrupted") ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
  await until(() => (get(s.id).status === "interrupted" ? true : undefined));
  rescript(r.binary, script);
  ok(await r.local.call("agent.session.resume", { sessionId: s.id }));
  ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "again" }));
  await until(() => (observed(r.serverData, s.id).length === 2 && get(s.id).status !== "running" ? true : undefined));
  return settle(s.id);
}

describe("through the runtime", () => {
  withFake("usage_is_delta_per_turn: two turns on one process", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 100, outputTokens: 10, costUsd: 0.01 } }, { expect: "user", cumulativeUsage: { inputTokens: 50, outputTokens: 20, costUsd: 0.02 } }] });
    const s = await start(r);
    await settle(s.id);
    ok(await r.local.call("agent.session.prompt", { sessionId: s.id, text: "more" }));
    await until(() => (usages(s.id).length === 2 ? true : undefined));
    const done = await settle(s.id);
    expect(usages(s.id)).toEqual([expect.objectContaining({ input: 100, output: 10, moneyMicros: 10_000, costBasis: "reported" }), expect.objectContaining({ input: 50, output: 20, moneyMicros: 20_000, costBasis: "reported" })]);
    expect(done.usage).toMatchObject({ input: 150, output: 30, moneyMicros: 30_000 });
    expect(ledger(s.id, "money")).toMatchObject({ used: 30_000, reserved: 0 });
    expect(observed(r.serverData, s.id)).toHaveLength(1);
  });

  // Until Task 0 pins the mode a resume fails closed: a CLI that restores its totals has the
  // earlier usage (100/10/$0.01) charged again; one that resets them is charged exactly.
  const RESUMED = { restored: { input: 250, output: 30, moneyMicros: 40_000, tokens: 280 }, reset: { input: 150, output: 20, moneyMicros: 30_000, tokens: 170 } };
  for (const mode of ["restored", "reset"] as const) {
    withFake(`delta_after_resume (${mode}): never under-charged`, async () => {
      const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 100, outputTokens: 10, costUsd: 0.01 } }] });
      const s = await start(r);
      await settle(s.id);
      const done = await restart(r, s, { ...CONSOLE, resumeTotals: mode, steps: [{ expect: "user", liveUsage: { inputTokens: 30, outputTokens: 4 }, cumulativeUsage: { inputTokens: 20, outputTokens: 6, costUsd: 0.02 } }] });
      expect(done.status).toBe("idle");
      const runs = observed(r.serverData, s.id);
      expect(runs[1]!.argv.some((a) => a.startsWith("--resume="))).toBe(true);
      const want = RESUMED[mode];
      expect(done.usage).toMatchObject({ input: want.input, output: want.output, moneyMicros: want.moneyMicros });
      expect(ledger(s.id, "tokens").used).toBe(want.tokens);
      expect(ledger(s.id, "money")).toMatchObject({ used: want.moneyMicros, reserved: 0 });
    });
  }

  withFake("interrupted_turn_usage_charged: an interrupt's result is charged to the run it ended", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", hiddenUsage: { inputTokens: 500, outputTokens: 50, costUsd: 0.02 } }, { sleep: 60_000 }] });
    const s = await start(r);
    await until(() => (get(s.id).status === "running" ? true : undefined));
    await until(() => (observed(r.serverData, s.id)[0]?.userLines?.length === 1 ? true : undefined));
    await new Promise((res) => setTimeout(res, 200));
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    await until(() => (usages(s.id).length === 1 ? true : undefined));
    const runId = nodes(s.id).find((n) => n.kind === "userMessage")!.runId!;
    expect(nodes(s.id).find((n) => n.kind === "usage")!.runId).toBe(runId);
    expect(get(s.id).usage).toMatchObject({ input: 500, output: 50, moneyMicros: 20_000 });
    expect(ledger(s.id, "money").used).toBe(20_000);
    // The next instance (totals reset) does not charge it again.
    const done = await restart(r, s, { ...CONSOLE, resumeTotals: "reset", steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1, costUsd: 0.001 } }] });
    expect(done.usage).toMatchObject({ input: 501, output: 51, moneyMicros: 21_000 });
  }, 30_000);

  withFake("live_tokens_stop_mid_turn: stream usage past the limit interrupts the turn, BUDGET_EXCEEDED, a budget card", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", liveUsage: { inputTokens: 1500, outputTokens: 10 } }, { sleep: 60_000, cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] });
    const s = await start(r, { tokens: 1000 });
    const t0 = Date.now();
    const done = await settle(s.id);
    // Well within the 30 s grace: no tool call was in flight.
    expect(Date.now() - t0).toBeLessThan(15_000);
    expect(done).toMatchObject({ status: "interrupted", statusReason: "BUDGET_EXCEEDED" });
    expect(errors(nodes(s.id))).toEqual([expect.objectContaining({ code: "BUDGET_EXCEEDED" })]);
    expect(server!.core.store.approvals(s.id)).toMatchObject([{ kind: "budget", status: "open", subject: { kind: "tokens", limit: 1000 } }]);
    expect(observed(r.serverData, s.id)[0]!.controlRequests?.map((c) => c.subtype)).toEqual(["initialize", "interrupt"]);
    // The interrupt's result added nothing the stream had not shown: charged once.
    await new Promise((res) => setTimeout(res, 300));
    expect(get(s.id).usage).toMatchObject({ input: 1500, output: 10 });
    expect(ledger(s.id, "tokens")).toMatchObject({ used: 1510, reserved: 0 });
  });

  withFake("cost_basis_subscription_vs_reported: a plan's tokens are charged, its money shown, not charged", async () => {
    const TURN = { steps: [{ expect: "user", cumulativeUsage: { inputTokens: 30, outputTokens: 7, costUsd: 0.0042 } }] };
    // The fake's default sign-in is a plan (`claude.ai`).
    let r = await claudeServer(TURN);
    let s = await start(r, { moneyMicros: 1_000_000 });
    await settle(s.id);
    expect(usages(s.id)).toEqual([expect.objectContaining({ input: 30, output: 7, moneyMicros: 4200, costBasis: "subscription" })]);
    expect(get(s.id).usage.moneyMicros).toBe(4200);
    expect(ledger(s.id, "money").used).toBe(0);
    expect(ledger(s.id, "tokens").used).toBe(37);
    expect(flag(observed(r.serverData, s.id)[0]!.argv)).toBeUndefined();
    await server!.stop();
    rmSync(tmp, { recursive: true, force: true });
    mkdirSync(tmp);
    // The owner asked for a plan's money to count.
    r = await claudeServer(TURN, { chargePlanMoney: true });
    s = await start(r, { moneyMicros: 1_000_000 });
    await settle(s.id);
    expect(usages(s.id)).toEqual([expect.objectContaining({ moneyMicros: 4200, costBasis: "reported" })]);
    expect(ledger(s.id, "money").used).toBe(4200);
    expect(flag(observed(r.serverData, s.id)[0]!.argv)).toBe("1");
    await server!.stop();
    rmSync(tmp, { recursive: true, force: true });
    mkdirSync(tmp);
    // An API-key sign-in: reported and charged.
    r = await claudeServer({ ...CONSOLE, ...TURN });
    s = await start(r, { moneyMicros: 1_000_000 });
    await settle(s.id);
    expect(usages(s.id)).toEqual([expect.objectContaining({ moneyMicros: 4200, costBasis: "reported" })]);
    expect(ledger(s.id, "money").used).toBe(4200);
  });

  withFake("max_budget_backstop_maps_to_budget_exceeded: the CLI's own stop interrupts the session", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 10, outputTokens: 1, costUsd: 0.02 } }] });
    const s = await start(r, { moneyMicros: 10_000 });
    const done = await settle(s.id);
    expect(flag(observed(r.serverData, s.id)[0]!.argv)).toBe("0.01");
    expect(done).toMatchObject({ status: "interrupted", statusReason: "BUDGET_EXCEEDED" });
    expect(errors(nodes(s.id)).map((e) => e.code)).toContain("BUDGET_EXCEEDED");
    expect(ledger(s.id, "money").used).toBe(20_000);
  });

  withFake("budget_flag_tracks_remaining_on_restart: the remainder on each start (fail closed on resume)", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 10, outputTokens: 1, costUsd: 0.25 } }] });
    const s = await start(r, { moneyMicros: 1_000_000 });
    await settle(s.id);
    // A person raises the money limit while the session is interrupted: the next process gets the new remainder.
    ok(await r.local.call("agent.session.interrupt", { sessionId: s.id }));
    await until(() => (get(s.id).status === "interrupted" ? true : undefined));
    server!.core.store.ledger().setLimit(s.id, "money", 2_000_000);
    const done = await restart(r, s, { ...CONSOLE, steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1, costUsd: 0.5 } }] });
    const [first, second] = observed(r.serverData, s.id);
    expect(flag(first!.argv)).toBe("1");
    // 1.75 left; the 0.25 a restoring CLI counts again is not added until Task 0 pins
    // `restored` (a restoring CLI stops early, as its restored 0.25 is charged again).
    expect(flag(second!.argv)).toBe("1.75");
    expect(done.status).toBe("idle");
    expect(ledger(s.id, "money").used).toBe(1_000_000);
  });

  withFake("per_model_split_recorded: modelUsage as a notice per result", async () => {
    const r = await claudeServer({ ...CONSOLE, steps: [{ expect: "user", liveUsage: { inputTokens: 40, outputTokens: 9, costUsd: 0.001 }, usageModel: "claude-haiku-5-5" }, { cumulativeUsage: { inputTokens: 5, outputTokens: 2, costUsd: 0.002 } }] });
    const s = await start(r);
    await settle(s.id);
    const split = nodes(s.id).filter((n) => n.kind === "notice" && (n.payload as Record<string, unknown>).modelUsage !== undefined);
    expect(split.map((n) => n.payload)).toEqual([
      { modelUsage: { "claude-haiku-5-5": { input: 40, output: 9, cacheRead: 0, cacheWrite: 0, costMicros: 1000 }, "claude-opus-5-5": { input: 5, output: 2, cacheRead: 0, cacheWrite: 0, costMicros: 2000 } } },
    ]);
    expect(get(s.id).usage).toMatchObject({ input: 45, output: 11, moneyMicros: 3000 });
  });
});

// ---- the gate, at the adapter ----

describe("the gate (Decision 15)", () => {
  const SESSION_ID = "01S0000000000000000000000B";

  async function open(script: Record<string, unknown>, gate: CallGate, remaining = { tokens: 300 as number | null, moneyMicros: null as number | null }) {
    const binary = install(script);
    const serverData = join(tmp, "data");
    const projectDir = join(tmp, "P");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    mkdirSync(projectDir, { recursive: true });
    mkdirSync(join(tmp, "hosts"), { recursive: true });
    const deps: SessionSandboxDeps = { serverData, projectDirOf: () => projectDir, finPath: "/bin/sh", localEndpoint: join(serverData, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, hostRoot: join(tmp, "hosts") };
    const s = { id: SESSION_ID, projectId: "01P0000000000000000000000B", harness: "claudeCode", mode: "draft", role: "editor", model: "default", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 0 } } as unknown as Session;
    const boxScope = await Effect.runPromise(Scope.make());
    const scope = await Effect.runPromise(Scope.make());
    cleanups.push(() => Effect.runPromise(Scope.close(boxScope, Exit.void)));
    cleanups.push(() => Effect.runPromise(Scope.close(scope, Exit.void)));
    const box = await Effect.runPromise(Scope.provide(boxScope)(openSessionSandbox(s, "claudeCode", deps)));
    const adapter = ClaudeCodeAdapter({ sandbox: deps, sessions: { get: (id) => (id === s.id ? s : undefined) }, models: new ClaudeModelCache(join(tmp, "m")), remaining: () => remaining, detect: () => Effect.succeed({ path: binary, version: VERSION, floor: CLAUDE_FLOOR, signedIn: true }) });
    const spec: SessionSpec = { sessionId: s.id, projectId: s.projectId, model: "default", role: "editor", mode: "draft", system: "You edit.", cwd: box.workspace, env: box.env, providerRef: null, tools: ["Read"], sandbox: box, gate };
    const provider = await Effect.runPromise(Scope.provide(scope)(adapter.open(spec)));
    const events: ProviderEvent[] = [];
    void Effect.runPromise(Stream.runForEach(provider.events, (b) => Effect.sync(() => events.push(b.event))));
    return { provider, events, serverData };
  }

  function counting(refuse?: Parameters<typeof finError>) {
    const calls: ProjectedUsage[] = [];
    let released = 0;
    const gate: CallGate = {
      beforeCall: (p) => {
        calls.push(p);
        return refuse === undefined ? { ok: true } : { ok: false, error: finError(...refuse) };
      },
      release: () => void released++,
    };
    return { gate, calls, released: () => released };
  }

  withFake("gate_once_per_turn_settled_by_usage", async () => {
    const g = counting();
    const w = await open({ steps: [{ expect: "user", liveUsage: { inputTokens: 5, outputTokens: 1 }, cumulativeUsage: { inputTokens: 3, outputTokens: 1 } }, { expect: "user" }] }, g.gate);
    await Effect.runPromise(w.provider.startTurn({ runId: "r1", text: "go" }));
    await until(() => (w.events.some((e) => e.type === "turnCompleted") ? true : undefined));
    // One check for the whole turn (its two usage events, live and the result's), capped at what is left.
    expect(g.calls).toEqual([{ input: 0, output: 300, cacheRead: 0, cacheWrite: 0, reasoning: 0 }]);
    expect(usageOf(w.events)).toHaveLength(2);
    expect(g.released()).toBe(0);
    // A turn that reports no usage returns its reservation.
    await Effect.runPromise(w.provider.startTurn({ runId: "r2", text: "again" }));
    await until(() => (w.events.filter((e) => e.type === "turnCompleted").length === 2 ? true : undefined));
    expect(g.calls).toHaveLength(2);
    expect(g.released()).toBe(1);
  });

  withFake("a refused turn fails before any line reaches the CLI", async () => {
    const g = counting(["BUDGET_EXCEEDED", "the session's token budget is used up", { kind: "tokens" }]);
    const w = await open({ steps: [{ expect: "user", cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }] }, g.gate);
    await Effect.runPromise(w.provider.startTurn({ runId: "r1", text: "go" }));
    await until(() => (w.events.some((e) => e.type === "turnFailed") ? true : undefined));
    expect(w.events.map((e) => e.type)).toEqual(["turnStarted", "turnFailed"]);
    expect(w.events[1]).toMatchObject({ error: { code: "BUDGET_EXCEEDED" } });
    await new Promise((res) => setTimeout(res, 300));
    expect(observed(w.serverData, SESSION_ID).at(-1)?.userLines ?? []).toEqual([]);
    // The per-run model-call cap ends the turn as a completion (as the built-in loop does).
    const cap = counting(["BUDGET_EXCEEDED", "per-run cap", { kind: "modelCalls" }]);
    rmSync(join(tmp, "data"), { recursive: true, force: true });
    const w2 = await open({ steps: [] }, cap.gate);
    await Effect.runPromise(w2.provider.startTurn({ runId: "r1", text: "go" }));
    await until(() => (w2.events.some((e) => e.type === "turnCompleted") ? true : undefined));
    expect(w2.events.find((e) => e.type === "turnCompleted")).toEqual({ type: "turnCompleted", stopReason: "modelCallCap" });
  });
});
