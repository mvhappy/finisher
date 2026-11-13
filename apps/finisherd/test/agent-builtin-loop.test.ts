// The built-in loop (plan 7A Task 8b; Decisions 6, 9, 10, 13): driven by a scripted model client
// as the runtime drives it (tool results through `respond`), plus the real OpenRouter client
// against the fake model server for retries, and one run through the real runtime.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, Node, ProviderEvent } from "@finisher/contracts";
import { Effect, Exit, Scope, Stream } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { CallGate, ProviderSession, SessionSpec } from "../src/agent/adapter.ts";
import { BuiltinAdapter, type BuiltinDeps, LOOP_LIMITS, uniqueIds } from "../src/agent/builtin/loop.ts";
import { boundOutput, historyFromNodes } from "../src/agent/builtin/loop-history.ts";
import { type ModelClient, type ModelResult, type ModelStreamEvent, openrouterClient } from "../src/agent/builtin/compat.ts";
import { FinIndex } from "../src/agent/builtin/fin-tool.ts";
import type { ModelRequest, ToolCall } from "../src/agent/builtin/request.ts";
import { TOOL_DEFS, toolsHash } from "../src/agent/builtin/tools.ts";
import { Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock } from "../src/agent/clock.ts";
import { cataloguePricing } from "../src/agent/runtime-deps.ts";
import { finError } from "../src/errors.ts";
import { type FakeModelServer, sse, startFakeModelServer } from "./support/fake-model-server.ts";

const CATALOGUE = Catalogue.load(fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url)), {
  extra: [
    { id: "acme-closed/small", window: 1000, maxOutput: 100 },
    { id: "acme-closed/notools", window: 8192, tools: false },
    // Priced, so a session of it may start (`PRICING_UNKNOWN` otherwise).
    { id: "acme-closed/big", window: 8192, price: { input: 1_000_000, output: 2_000_000, cacheRead: 100_000, cacheWrite: 1_250_000 } },
  ],
});
const MODEL = "acme-closed/big";
const cleanups: (() => Promise<void>)[] = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

// A scripted model client ------------------------------------------------------------------

type Reply = { text?: string; calls?: Array<[string, string, unknown]>; usage?: Partial<typeof ZERO_USAGE>; stop?: ModelResult["stopReason"]; reasoning?: unknown[] };

function result(r: Reply): ModelResult {
  const toolCalls: ToolCall[] = (r.calls ?? []).map(([id, name, input]) => ({ id, name, arguments: typeof input === "string" ? input : JSON.stringify(input) }));
  return {
    text: r.text ?? "",
    toolCalls,
    reasoningText: "",
    reasoning: r.reasoning ?? [],
    stopReason: r.stop ?? (toolCalls.length > 0 ? "toolUse" : "endTurn"),
    usage: { ...ZERO_USAGE, input: 10, output: 5, ...r.usage },
    usageEstimated: false,
    capture: { request: "{}", response: [] },
    bodyHash: "blake3:00",
    latencyMs: 1,
  };
}

function scripted(replies: Array<Reply | ((req: ModelRequest) => Reply)>): ModelClient & { requests: ModelRequest[] } {
  const requests: ModelRequest[] = [];
  return {
    preset: "openrouter",
    warnings: [],
    requests,
    wireBody: (req) => req as unknown as Record<string, unknown>,
    async *stream(req): AsyncIterable<ModelStreamEvent> {
      requests.push(structuredClone(req));
      const next = replies[Math.min(requests.length - 1, replies.length - 1)]!;
      const r = typeof next === "function" ? next(req) : next;
      if (r.text) yield { type: "textDelta", text: r.text };
      yield { type: "done", result: result(r) };
    },
  };
}

// The runtime's side: events, tool results, the gate ------------------------------------------

interface Gate extends CallGate {
  readonly calls: number;
  readonly releases: number;
  refuse: FinError | null;
}

function gate(): Gate {
  const g = {
    calls: 0,
    releases: 0,
    refuse: null as FinError | null,
    beforeCall() {
      g.calls++;
      return g.refuse === null ? ({ ok: true } as const) : ({ ok: false, error: g.refuse } as const);
    },
    release() {
      g.releases++;
    },
  };
  return g;
}

type Tool = (id: string, tool: string, input: unknown) => Promise<{ ok: boolean; output: unknown }>;

interface Driven {
  readonly events: ProviderEvent[];
  readonly session: ProviderSession;
  readonly gate: Gate;
  readonly log: string[];
  turn(text: string, facts?: string): Promise<ProviderEvent>;
}

const echo: Tool = async (_id, _tool, input) => ({ ok: true, output: { echoed: input } });

async function drive(deps: Omit<Partial<BuiltinDeps>, "client"> & { client: ModelClient }, tool: Tool = echo, spec: Partial<SessionSpec> = {}): Promise<Driven> {
  const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/nonexistent/fin", ...deps, client: () => deps.client });
  const g = gate();
  const scope = await Effect.runPromise(Scope.make());
  cleanups.push(() => Effect.runPromise(Scope.close(scope, Exit.void)));
  const session = await Effect.runPromise(
    Scope.provide(scope)(
      adapter.open({ sessionId: "S1", projectId: "P1", provider: "openrouter", model: MODEL, role: "editor", mode: "draft", system: "You edit.", cwd: "/w", env: {}, providerRef: null, gate: g, ...spec }),
    ),
  );
  const events: ProviderEvent[] = [];
  const log: string[] = [];
  let ended: ((e: ProviderEvent) => void) | null = null;
  Effect.runFork(
    Stream.runForEach(session.events, ({ event }) =>
      Effect.sync(() => {
        events.push(event);
        if (event.type === "toolStarted") {
          log.push(`start ${event.providerId}`);
          void tool(event.providerId, event.tool, event.input).then((a) => {
            log.push(`end ${event.providerId}`);
            return Effect.runPromise(session.respond(event.providerId, { kind: "toolResult", ...a }));
          });
        }
        if (event.type === "turnCompleted" || event.type === "turnFailed") ended?.(event);
      }),
    ),
  );
  return {
    events,
    session,
    gate: g,
    log,
    turn: (text, facts) =>
      new Promise<ProviderEvent>((resolve, reject) => {
        ended = resolve;
        Effect.runPromise(session.startTurn({ runId: "R1", text, ...(facts === undefined ? {} : { facts }) })).catch(reject);
      }),
  };
}

const types = (es: ProviderEvent[]) => es.map((e) => e.type);
const ofType = <T extends ProviderEvent["type"]>(es: ProviderEvent[], t: T) => es.filter((e): e is Extract<ProviderEvent, { type: T }> => e.type === t);
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe("built-in loop", () => {
  test("end_turn_stops", async () => {
    const client = scripted([{ text: "Done." }]);
    const d = await drive({ client });
    const end = await d.turn("tighten the intro", "Project P1, branch br1, mode draft.");
    expect(end).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
    expect(types(d.events)).toEqual(["turnStarted", "messageDelta", "capture", "usage", "message", "turnCompleted"]);
    expect(client.requests).toHaveLength(1);
    const req = client.requests[0]!;
    // The system prompt is the role's alone; the run's facts head the user message (Decision 13).
    expect(req.system).toBe("You edit.");
    expect(req.tools).toEqual(TOOL_DEFS);
    expect(req.messages).toEqual([{ role: "user", content: [{ type: "text", text: "Project P1, branch br1, mode draft." }, { type: "text", text: "tighten the intro" }] }]);
    expect(d.gate.calls).toBe(1);
    expect(toolsHash).toMatch(/^blake3:[0-9a-f]{64}$/);
  });

  test("tool_round_trip", async () => {
    const client = scripted([{ calls: [["c1", "fin", { argv: ["view"] }]] }, { text: "Trimmed." }]);
    const d = await drive({ client }, async () => ({ ok: true, output: { exit: 0, stdout: { items: 3 } } }));
    expect(await d.turn("go")).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
    const started = ofType(d.events, "toolStarted");
    // `fin view` gets a budget from the window: 5 % of 8192, at most 4,000 tokens.
    expect(started).toEqual([{ type: "toolStarted", providerId: "c1", tool: "fin", input: { argv: ["view", "--budget", "409"] } }]);
    expect(ofType(d.events, "toolCompleted")).toEqual([{ type: "toolCompleted", providerId: "c1", ok: true, output: { exit: 0, stdout: { items: 3 } } }]);
    const second = client.requests[1]!.messages;
    expect(second.at(-2)).toMatchObject({ role: "assistant", toolCalls: [{ id: "c1", name: "fin" }] });
    expect(second.at(-1)).toEqual({ role: "tool", toolCallId: "c1", content: JSON.stringify({ exit: 0, stdout: { items: 3 } }) });
    expect(ofType(d.events, "usage")).toHaveLength(2);
  });

  test("bad_tool_calls_are_answered_without_running", async () => {
    const client = scripted([{ calls: [["c1", "shell", { cmd: "ls" }], ["c2", "fin", "{not json"]] }, { text: "ok" }]);
    const d = await drive({ client });
    await d.turn("go");
    expect(ofType(d.events, "toolStarted")).toEqual([]);
    const msgs = client.requests[1]!.messages.filter((m) => m.role === "tool");
    expect(msgs.map((m) => JSON.parse((m as { content: string }).content).error.code)).toEqual(["USAGE", "USAGE"]);
  });

  test("parallel_reads_serial_writes", async () => {
    const index = new FinIndex([
      { name: "view", write: false, paths: [] },
      { name: "status", write: false, paths: [] },
      { name: "item.trim", write: true, paths: [] },
    ]);
    const calls: Array<[string, string, unknown]> = [
      ["r1", "fin", { argv: ["status"] }],
      ["r2", "workspace_read", { path: "notes.md" }],
      ["w1", "fin", { argv: ["item", "trim", "c1", "--end", "-1s"] }],
      ["r3", "workspace_list", {}],
      ["w2", "workspace_write", { path: "a.md", content: "x" }],
      ["w3", "fin", { argv: ["frobnicate"] }],
    ];
    const client = scripted([{ calls }, { text: "ok" }]);
    const d = await drive({ client, index: async () => index }, async (id) => {
      await delay(id === "r1" ? 30 : 5);
      return { ok: true, output: id };
    });
    await d.turn("go");
    // r1 and r2 run together; each write waits for everything before it; an unknown command is a write.
    expect(d.log).toEqual(["start r1", "start r2", "end r2", "end r1", "start w1", "end w1", "start r3", "end r3", "start w2", "end w2", "start w3", "end w3"]);
    const results = client.requests[1]!.messages.filter((m) => m.role === "tool").map((m) => (m as { toolCallId: string }).toolCallId);
    expect(results).toEqual(["r1", "r2", "w1", "r3", "w2", "w3"]);
  });

  test("per_run_caps", async () => {
    expect([LOOP_LIMITS.modelCalls, LOOP_LIMITS.toolCalls]).toEqual([60, 200]);
    const always = scripted([{ calls: [["x", "workspace_list", {}]] }]);
    const d = await drive({ client: always, limits: { modelCalls: 3 } }, async () => ({ ok: true, output: [] }));
    expect(await d.turn("go")).toEqual({ type: "turnCompleted", stopReason: "modelCallCap" });
    expect(always.requests).toHaveLength(3);
    // The tool cap: the third call of the run is answered, not run, and the run ends.
    let n = 0;
    const two = scripted([() => ({ calls: [[`a${++n}`, "workspace_list", {}], [`b${n}`, "workspace_list", {}]] })]);
    const t = await drive({ client: two, limits: { toolCalls: 3 } });
    expect(await t.turn("go")).toEqual({ type: "turnCompleted", stopReason: "toolCallCap" });
    expect(ofType(t.events, "toolStarted").map((e) => e.providerId)).toEqual(["a1", "b1", "a2"]);
    // The runtime's per-run model-call cap ends the run only; a session budget fails the turn.
    const g = await drive({ client: scripted([{ text: "x" }]) });
    g.gate.refuse = finError("BUDGET_EXCEEDED", "cap", { kind: "modelCalls", used: 60, limit: 60 });
    expect(await g.turn("go")).toEqual({ type: "turnCompleted", stopReason: "modelCallCap" });
    const m = await drive({ client: scripted([{ text: "x" }]) });
    m.gate.refuse = finError("BUDGET_EXCEEDED", "money", { kind: "money", used: 10, limit: 10 });
    expect(await m.turn("go")).toMatchObject({ type: "turnFailed", error: { code: "BUDGET_EXCEEDED", details: { kind: "money" } } });
  });

  test("result_truncation_head_tail_note", async () => {
    const big = `HEAD${"x".repeat(40 * 1024)}TAIL`;
    const client = scripted([{ calls: [["c1", "workspace_read", { path: "big" }], ["c2", "workspace_read", { path: "small" }]] }, { text: "ok" }]);
    const d = await drive({ client }, async (id) => ({ ok: true, output: id === "c1" ? big : "short" }));
    await d.turn("go");
    const [cut, small] = client.requests[1]!.messages.filter((m) => m.role === "tool") as Array<{ content: string }>;
    // A text result is shown as text, a structured one as JSON.
    expect(cut!.content.startsWith("HEADxxx")).toBe(true);
    expect(cut!.content.endsWith("xxxTAIL")).toBe(true);
    expect(cut!.content).toContain("tool call c1");
    expect(Buffer.byteLength(cut!.content)).toBeLessThan(16 * 1024 + 200);
    expect(Buffer.byteLength(cut!.content)).toBeGreaterThan(16 * 1024);
    expect(small!.content).toBe("short");
    // The runtime still gets the whole output.
    expect(ofType(d.events, "toolCompleted")[0]!.output).toBe(big);
  });

  test("context_stop_at_85_percent", async () => {
    // A 1,000-token window: the first call's prompt and reply count 900, past 850.
    const client = scripted([{ calls: [["c1", "workspace_list", {}]], usage: { input: 880, output: 20 } }, { text: "never" }]);
    const d = await drive({ client }, echo, { model: "acme-closed/small" });
    expect(await d.turn("go")).toMatchObject({ type: "turnFailed", error: { code: "MODEL_CONTEXT_EXCEEDED", details: { window: 1000 } } });
    expect(client.requests).toHaveLength(1);
    // A prompt that is too long from the start is never sent.
    const none = scripted([{ text: "never" }]);
    const e = await drive({ client: none }, echo, { model: "acme-closed/small" });
    expect(await e.turn("x".repeat(4000))).toMatchObject({ type: "turnFailed", error: { code: "MODEL_CONTEXT_EXCEEDED" } });
    expect(none.requests).toHaveLength(0);
  });

  test("retry_on_429_with_retry_after", async () => {
    const server: FakeModelServer = await startFakeModelServer([
      { status: 429, body: { error: { code: 429, message: "slow down" } }, headers: { "retry-after": "3" } },
      { sse: sse("tool-split") },
      { sse: sse("text") },
    ]);
    cleanups.push(() => server.close());
    const sleeps: number[] = [];
    const client = openrouterClient({ testBaseUrl: server.baseUrl, clock: manualClock(), sleep: async (ms) => void sleeps.push(ms) });
    let runs = 0;
    const d = await drive({ client, gateway: { withKey: (_p, _s, f) => f("sk-test") } }, async () => {
      runs++;
      return { ok: true, output: { exit: 0 } };
    });
    expect(await d.turn("trim c1")).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
    expect(sleeps).toEqual([3000]);
    const posts = server.requests().filter((r) => r.method === "POST");
    expect(posts).toHaveLength(3);
    // The same request body is sent again; the tool ran once.
    expect(posts[1]!.body.equals(posts[0]!.body)).toBe(true);
    expect(runs).toBe(1);
    expect(ofType(d.events, "notice")).toEqual([{ type: "notice", name: "retry", details: { attempt: 1, code: "PROVIDER_RATE_LIMITED", delayMs: 3000 } }]);
    // Every attempt passed the gate; the one that reported nothing released its reservation.
    expect(d.gate.calls).toBe(3);
    expect(d.gate.releases).toBe(1);
    expect(ofType(d.events, "usage")).toHaveLength(2);
    // The capture is the exact body as sent.
    const capture = ofType(d.events, "capture")[0]!;
    expect(Buffer.from(capture.request as string, "utf8").equals(posts[1]!.body)).toBe(true);
    expect(capture.response).toMatchObject({ upstream: "DeepSeek", bodyHash: expect.stringMatching(/^blake3:/) });
  });

  test("a_refusal_between_attempts_ends_the_run", async () => {
    const server = await startFakeModelServer([{ status: 503, body: { error: { message: "busy" } } }, { sse: sse("text") }]);
    cleanups.push(() => server.close());
    const client = openrouterClient({ testBaseUrl: server.baseUrl, clock: manualClock(), sleep: async () => {} });
    const d = await drive({ client, gateway: { withKey: (_p, _s, f) => f("sk-test") } });
    const real = d.gate.beforeCall.bind(d.gate);
    d.gate.beforeCall = (p) => (d.gate.calls === 0 ? real(p) : (d.gate.refuse = finError("BUDGET_EXCEEDED", "money", { kind: "money" }), real(p)));
    expect(await d.turn("go")).toMatchObject({ type: "turnFailed", error: { code: "BUDGET_EXCEEDED" } });
    expect(server.requests().filter((r) => r.method === "POST")).toHaveLength(1);
  });

  test("steer_joins_before_the_next_call", async () => {
    const client = scripted([{ calls: [["c1", "workspace_list", {}]] }, { text: "ok" }]);
    let d!: Driven;
    d = await drive({ client }, async () => {
      await Effect.runPromise(d.session.steer({ text: "also the outro" }));
      return { ok: true, output: [] };
    });
    await d.turn("go");
    expect(client.requests[1]!.messages.at(-1)).toEqual({ role: "user", content: [{ type: "text", text: "also the outro" }] });
  });

  test("interrupt_ends_the_turn", async () => {
    const client = scripted([{ calls: [["c1", "workspace_list", {}]] }, { text: "never" }]);
    const d = await drive({ client }, () => new Promise(() => {}));
    const end = d.turn("go");
    while (ofType(d.events, "toolStarted").length === 0) await delay(2);
    await Effect.runPromise(d.session.interrupt());
    expect(await end).toEqual({ type: "turnCompleted", stopReason: "interrupted" });
    expect(client.requests).toHaveLength(1);
    // A second turn may start after it.
    await expect(d.turn("again")).resolves.toMatchObject({ type: "turnCompleted" });
  });

  test("a_model_without_tools_is_refused_at_open", async () => {
    const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => scripted([]) });
    const scope = await Effect.runPromise(Scope.make());
    const exit = await Effect.runPromiseExit(
      Scope.provide(scope)(adapter.open({ sessionId: "S", projectId: "P", provider: "openrouter", model: "acme-closed/notools", role: "editor", mode: "draft", system: "", cwd: "/w", env: {}, providerRef: null })),
    );
    await Effect.runPromise(Scope.close(scope, Exit.void));
    expect(Exit.isFailure(exit) && JSON.stringify(exit.cause)).toContain("noTools");
  });

  test("history_from_nodes_resumes_without_rerunning", () => {
    let seq = 0;
    const n = (kind: Node["kind"], payload: unknown, providerId?: string): Node => ({
      id: `n${++seq}`,
      runId: "R",
      sessionId: "S",
      seq,
      kind,
      payload,
      createdAt: 0,
      ...(providerId === undefined ? {} : { providerId }),
    });
    const h = historyFromNodes([
      n("userMessage", { text: "go", trigger: "prompt" }),
      n("reasoning", { text: "think", encrypted: [{ type: "reasoning.text", text: "think" }] }),
      n("assistantMessage", { role: "assistant", blocks: [{ type: "text", text: "Looking." }] }),
      n("toolCall", { tool: "fin", input: { argv: ["view"] } }, "c1"),
      n("toolCall", { tool: "fin", input: { argv: ["item", "trim"] } }, "c2"),
      n("toolResult", { ok: true, output: { exit: 0 } }, "c1"),
      n("toolResult", { ok: false, output: { code: "SESSION_INTERRUPTED", synthetic: true } }, "c2"),
      n("usage", { input: 1 }),
    ], LOOP_LIMITS);
    expect(h).toEqual([
      { role: "user", content: [{ type: "text", text: "go" }] },
      {
        role: "assistant",
        text: "Looking.",
        toolCalls: [
          { id: "c1", name: "fin", arguments: '{"argv":["view"]}' },
          { id: "c2", name: "fin", arguments: '{"argv":["item","trim"]}' },
        ],
        reasoning: [{ type: "reasoning.text", text: "think" }],
      },
      { role: "tool", toolCallId: "c1", content: '{"exit":0}' },
      { role: "tool", toolCallId: "c2", content: '{"ok":false,"code":"SESSION_INTERRUPTED","synthetic":true}' },
    ]);
  });

  test("history_joins_hook_context_back (7F Decision 16): a prompt's block after its text, a hook steer as a steer", () => {
    let seq = 0;
    const n = (kind: Node["kind"], payload: unknown): Node => ({ id: `n${++seq}`, runId: "R", sessionId: "S", seq, kind, payload, createdAt: 0 });
    const block = (t: string) => `<hook-context>\n${t}\n</hook-context>`;
    const h = historyFromNodes([
      n("notice", { hookContext: { event: "SessionStart", hooks: ["a"], text: "S" } }),
      n("notice", { hookContext: { event: "UserPrompt", hooks: ["b"], text: "U" } }),
      n("userMessage", { text: "go", trigger: "prompt" }),
      n("usage", { input: 1 }),
      n("assistantMessage", { role: "assistant", blocks: [{ type: "text", text: "Looking." }] }),
      n("userMessage", { text: block("lint failed"), trigger: "hook", hookContext: { event: "PostCommand", hooks: ["lint"] } }),
      n("notice", { steersTaken: { count: 1 } }),
      n("usage", { input: 1 }),
      n("assistantMessage", { role: "assistant", blocks: [{ type: "text", text: "Fixing." }] }),
    ], LOOP_LIMITS);
    expect(h).toEqual([
      { role: "user", content: [{ type: "text", text: "go\n\n<hook-context>\nS\n\nU\n</hook-context>" }] },
      { role: "assistant", text: "Looking." },
      { role: "user", content: [{ type: "text", text: block("lint failed") }] },
      { role: "assistant", text: "Fixing." },
    ]);
  });
});

describe("built-in loop: review findings (Task 8b)", () => {
  const index = async () => new FinIndex([{ name: "view", write: false, paths: [] }, { name: "item.trim", write: true, paths: [] }]);

  test("task10_capture_then_usage_then_content_then_tool_calls", async () => {
    const client = scripted([{ text: "Looking.", reasoning: [{ type: "reasoning.text", text: "hm" }], calls: [["c1", "fin", { argv: ["view"] }]] }, { text: "Done." }]);
    const d = await drive({ client, index });
    await d.turn("go");
    const t = types(d.events);
    const first = (x: string) => t.indexOf(x as ProviderEvent["type"]);
    expect(first("capture")).toBeLessThan(first("usage"));
    expect(first("usage")).toBeLessThan(first("reasoning"));
    expect(first("reasoning")).toBeLessThan(first("message"));
    expect(first("message")).toBeLessThan(first("toolStarted"));
    // The second call: its capture, then its usage, before its message.
    expect(t.slice(t.indexOf("toolCompleted") + 1).filter((x) => x !== "messageDelta")).toEqual(["capture", "usage", "message", "turnCompleted"]);
    const cap = ofType(d.events, "capture")[0]!;
    expect(cap.response).toMatchObject({ chunks: [], stopReason: "toolUse", latencyMs: 1 });
  });

  test("m8_duplicate_tool_call_ids_are_renamed", async () => {
    expect(uniqueIds([{ id: "a", name: "fin", arguments: "" }, { id: "a", name: "fin", arguments: "" }, { id: "a_2", name: "fin", arguments: "" }, { id: "", name: "fin", arguments: "" }]).map((c) => c.id)).toEqual(["a", "a_3", "a_2", "call_3"]);
    const client = scripted([{ calls: [["c1", "fin", { argv: ["view"] }], ["c1", "fin", { argv: ["view", "x"] }]] }, { text: "Done." }]);
    const d = await drive({ client, index });
    const end = await Promise.race([d.turn("go"), delay(3000).then(() => "hung")]);
    expect(end).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
    expect(ofType(d.events, "toolStarted").map((e) => e.providerId)).toEqual(["c1", "c1_2"]);
    const msgs = client.requests[1]!.messages;
    expect(msgs.filter((m) => m.role === "tool").map((m) => (m as { toolCallId: string }).toolCallId)).toEqual(["c1", "c1_2"]);
  });

  test("m6_at_most_8_reads_at_once_and_stored_output_is_bounded", async () => {
    const calls: Array<[string, string, unknown]> = Array.from({ length: 12 }, (_, i) => [`r${i}`, "fin", { argv: ["view", String(i)] }]);
    const client = scripted([{ calls }, { text: "Done." }]);
    let running = 0;
    let most = 0;
    const tool: Tool = async () => {
      most = Math.max(most, ++running);
      await delay(20);
      running--;
      return { ok: true, output: "x".repeat(2 * 1024 * 1024) };
    };
    const d = await drive({ client, index }, tool);
    await d.turn("go");
    expect(most).toBe(8);
    for (const e of ofType(d.events, "toolCompleted")) expect(JSON.stringify(e.output).length).toBeLessThan(1024 * 1024 + 100);
    expect(boundOutput({ a: 1 }, 100)).toEqual({ a: 1 });
    expect(boundOutput("y".repeat(10), 4)).toEqual({ truncated: true, bytes: 10, head: "yyyy" });
  });

  test("l9_every_admitted_attempt_is_settled_once", async () => {
    const throwing = (kind: "throw" | "end" | "retryThenThrow"): ModelClient => ({
      preset: "openrouter",
      warnings: [],
      wireBody: (req) => req as unknown as Record<string, unknown>,
      async *stream(): AsyncIterable<ModelStreamEvent> {
        if (kind === "retryThenThrow") yield { type: "retry", attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 0, cost: { usage: null, usageEstimated: false } };
        if (kind === "end") return;
        throw new Error("the stream broke");
      },
    });
    for (const [kind, attempts] of [["throw", 1], ["end", 1], ["retryThenThrow", 2]] as const) {
      const d = await drive({ client: throwing(kind) });
      const end = await d.turn("go");
      expect(end.type, kind).toBe("turnFailed");
      expect(d.gate.calls, kind).toBe(attempts);
      expect(d.gate.releases, kind).toBe(attempts);
    }
  });

  test("m7_resume_rebuilds_the_live_conversation_byte_for_byte", async () => {
    // A reply with reasoning, text, a read, a write, an unknown tool and a big result; a steer
    // taken in between; then a second reply with only calls; then the end.
    const client = scripted([
      { text: "Looking.", reasoning: [{ type: "reasoning.encrypted", data: "e1" }], calls: [["c1", "fin", { argv: ["view"] }], ["c2", "nope", {}], ["c3", "fin", { argv: ["item", "trim", "x"] }], ["c4", "fin", "{bad"]] },
      { calls: [["c5", "fin", { argv: ["item", "trim", "y"] }], ["c6", "fin", { argv: ["item", "trim", "z"] }]] },
      { text: "Done." },
    ]);
    let seq = 0;
    const nodes: Node[] = [];
    const node = (kind: Node["kind"], payload: unknown, providerId?: string) =>
      nodes.push({ id: `n${++seq}`, runId: "R", sessionId: "S", seq, kind, payload, createdAt: 0, ...(providerId === undefined ? {} : { providerId }) });
    let session: ProviderSession | null = null;
    const tool: Tool = async (id, _t, input) => {
      if (id === "c3") {
        node("userMessage", { text: "also the outro", trigger: "steer" });
        await Effect.runPromise(session!.steer({ text: "also the outro" }));
      }
      return id === "c1" ? { ok: true, output: { big: "z".repeat(40_000) } } : { ok: id !== "c6", output: { exit: id === "c6" ? 3 : 0, input } };
    };
    node("userMessage", { text: "go", trigger: "prompt" });
    const d = await drive({ client, index }, tool);
    session = d.session;
    // The runtime's side: the nodes it writes for each event (`runtime-engine.ts`).
    const stop = (async () => {
      for (let seen = 0; ; ) {
        while (seen < d.events.length) {
          const e = d.events[seen++]!;
          if (e.type === "usage") node("usage", { input: e.input });
          else if (e.type === "reasoning") node("reasoning", { ...(e.encrypted ? { encrypted: e.encrypted } : {}) });
          else if (e.type === "message") node("assistantMessage", { role: e.role, blocks: e.blocks });
          else if (e.type === "toolStarted") node("toolCall", { tool: e.tool, input: e.input }, e.providerId);
          else if (e.type === "toolCompleted") node("toolResult", { ok: e.ok, output: e.output }, e.providerId);
          else if (e.type === "notice") node("notice", { [e.name]: e.details ?? true });
          else if (e.type === "turnCompleted") return;
        }
        await delay(1);
      }
    })();
    await d.turn("go");
    await stop;
    const live = client.requests.at(-1)!.messages;
    const rebuilt = historyFromNodes(nodes, LOOP_LIMITS);
    expect(JSON.stringify(rebuilt.slice(0, live.length))).toBe(JSON.stringify(live));
    // The steer came after the first reply's results, as one user message.
    expect(live.filter((m) => m.role === "user")).toHaveLength(2);
    const c1 = live.find((m) => m.role === "tool" && m.toolCallId === "c1") as { content: string } | undefined;
    expect(c1?.content).toContain("bytes cut here");
  });
});

describe("built-in loop in the runtime", () => {
  test("the_runtime_runs_the_loop_tools_with_call_ids", async () => {
    const { AgentStore } = await import("../src/agent/store.ts");
    const { openControlStore } = await import("../src/control/db.ts");
    const { systemIds } = await import("../src/agent/clock.ts");
    const { FinTool, makeRuntime, META, PEOPLE } = await import("./support/runtime-doubles.ts");
    const dir = mkdtempSync(join(tmpdir(), "agent-builtin-"));
    const clock = manualClock(Date.now());
    const control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    const store = new AgentStore(control, dir, clock, systemIds(clock));
    const client = scripted([{ calls: [["c1", "fin", { argv: ["item", "trim", "c1"] }]] }, { text: "Trimmed." }]);
    const tool = new FinTool();
    const h = makeRuntime({ store, control }, BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client }), { clock, tools: { builtin: tool }, tool, pricing: cataloguePricing(CATALOGUE) });
    cleanups.push(async () => {
      await h.runtime.close();
      await control.close();
      rmSync(dir, { recursive: true, force: true });
    });
    await h.runtime.boot();
    const { session } = await Effect.runPromise(h.runtime.start({ projectId: "P1", harness: "builtin", model: MODEL, prompt: "trim it" }, PEOPLE, META));
    const t0 = Date.now();
    while (store.get(session.id)!.status !== "idle" || store.nodes(session.id).length < 6) {
      if (Date.now() - t0 > 5000) throw new Error(`not idle: ${JSON.stringify(store.nodes(session.id).map((x) => x.kind))}`);
      await delay(5);
    }
    expect(store.nodes(session.id).map((x) => x.kind)).toEqual(["userMessage", "usage", "toolCall", "toolResult", "usage", "assistantMessage"]);
    expect(tool.calls).toHaveLength(1);
    const call = tool.calls[0]!;
    expect(call.callId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(call.env.FIN_CREDENTIAL).toContain(session.id);
    expect(tool.commits.has(call.callId)).toBe(true);
    // The run's facts headed the prompt, never the system prompt.
    expect(client.requests[0]!.system).toBe("You are the editor.");
    expect((client.requests[0]!.messages[0] as unknown as { content: { text: string }[] }).content[0]!.text).toContain("Project P1");
  });
});
