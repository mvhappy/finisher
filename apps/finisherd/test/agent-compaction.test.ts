// Context compaction for the built-in loop (plan 7B Task 9, Decision 24; spec §4.4; Review
// Focus 6): before a call whose prompt would pass 75 % of the window, one summary call over the
// runs before the current one; those runs' messages are then one user-role message holding the
// summary. The summary is a `summary` node, its call an ordinary `agent.turn` with
// `purpose: compaction`, and the next turn's trace-store delta starts over, so the deltas still
// replay to every request's `bodyHash`.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, Node, ProviderEvent } from "@finisher/contracts";
import { blake3Hex } from "@finisher/fin-node";
import { Effect, Exit, Scope, Stream } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { CallGate, ProviderSession } from "../src/agent/adapter.ts";
import { COMPACTION_PROMPT, COMPACTION_PROMPT_ID, summaryMessage } from "../src/agent/builtin/compact.ts";
import { type ModelClient, type ModelResult, type ModelStreamEvent, openrouterClient } from "../src/agent/builtin/compat.ts";
import { BuiltinAdapter, type BuiltinDeps, LOOP_LIMITS } from "../src/agent/builtin/loop.ts";
import { historyFromNodes } from "../src/agent/builtin/loop-history.ts";
import type { ModelMessage, ModelRequest } from "../src/agent/builtin/request.ts";
import { Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentJournal } from "../src/agent/journal.ts";
import { type MessagesDelta, replayBodies } from "../src/agent/journal-content.ts";
import { cataloguePricing } from "../src/agent/runtime-deps.ts";
import { AgentStore } from "../src/agent/store.ts";
import { openControlStore } from "../src/control/db.ts";
import { agentEntries, content, flush, openStores, P1 } from "./support/agent-journal.ts";
import { startFakeModelServer } from "./support/fake-model-server.ts";
import { FinTool, makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

// An 8,000-token window: compaction past 6,000, the stop past 6,800.
const MODEL = "acme-closed/compact";
const CATALOGUE = Catalogue.load(fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url)), {
  extra: [{ id: MODEL, window: 8000, maxOutput: 100, price: { input: 1_000_000, output: 2_000_000, cacheRead: 100_000, cacheWrite: 1_250_000 } }],
});
const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function until(f: () => boolean, what: string | (() => string), ms = 8000): Promise<void> {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${typeof what === "string" ? what : what()}`);
    await delay(5);
  }
}

// SSE replies of the fake model server ---------------------------------------------------------

const chunk = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`;
const usageOf = (prompt: number, completion: number) => ({ prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion, cost: 0.0001 });
function textSse(text: string, prompt: number, completion = 10): string {
  const id = `gen-${text.length}-${prompt}`;
  return (
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: text }, finish_reason: null }] }) +
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: usageOf(prompt, completion) }) +
    "data: [DONE]\n\n"
  );
}
function toolSse(callId: string, argv: string[], prompt: number, completion = 10): string {
  const id = `gen-${callId}`;
  const call = { index: 0, id: callId, type: "function", function: { name: "fin", arguments: JSON.stringify({ argv }) } };
  return (
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: null, tool_calls: [call] }, finish_reason: null }] }) +
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }], usage: usageOf(prompt, completion) }) +
    "data: [DONE]\n\n"
  );
}

const SUMMARY = "The person asked for the first cut; it is done. Nothing is pending.";
const textsOf = (m: ModelMessage | Record<string, unknown>): string => JSON.stringify(m);

// A whole session through the runtime, the real client and the journal ------------------------

/**
 * Three runs. Run 1 ends at once. Run 2's first call reports 5,990 prompt tokens and calls a
 * tool: its next call would pass 75 %, so the loop compacts run 1 first, mid-run. Run 3 fits.
 */
async function compactedSession() {
  const server = await startFakeModelServer([
    { sse: textSse("reply one", 100) },
    { sse: toolSse("call_s", ["status"], 5990) },
    { sse: textSse(SUMMARY, 5000, 40) },
    { sse: textSse("reply two", 900) },
    { sse: textSse("reply three", 1000) },
  ]);
  cleanups.push(() => server.close());
  const dir = mkdtempSync(join(tmpdir(), "agent-compaction-"));
  const stores = openStores(dir, { diagnostics: "on", training: "on" });
  const clock = manualClock(Date.now());
  const control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  const store = new AgentStore(control, dir, clock, systemIds(clock));
  const journal = new AgentJournal({ diagnostics: stores.diag, traces: stores.traces, store, ids: systemIds(clock), clock });
  const client = openrouterClient({ testBaseUrl: server.baseUrl, cachingFor: () => "none", clock });
  const gateway = { withKey: <A>(_p: string, _s: string, f: (key: string) => Promise<A>) => f("sk-or-v1-test") };
  const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client, gateway, nodes: (sid) => store.nodes(sid, 1, 1000) });
  const h = makeRuntime({ store, control }, adapter, { clock, journal, tools: { builtin: new FinTool() }, pricing: cataloguePricing(CATALOGUE) });
  cleanups.push(async () => {
    await h.runtime.close();
    await control.close();
    await stores.close();
    rmSync(dir, { recursive: true, force: true });
  });
  await h.runtime.boot();
  const { session } = await Effect.runPromise(h.runtime.start({ projectId: P1, harness: "builtin", model: MODEL, prompt: "first" }, PEOPLE, META));
  const idleAfter = (count: number) =>
    until(() => store.get(session.id)!.status === "idle" && store.runs(session.id).length === count && store.runs(session.id).at(-1)!.status !== "running", () => `run ${count}: ${JSON.stringify({ s: store.get(session.id), runs: store.runs(session.id), nodes: store.nodes(session.id, 1, 1000).map((n) => [n.kind, n.payload]) })}`);
  await idleAfter(1);
  for (const [k, text] of [
    [2, "second"],
    [3, "third"],
  ] as const) {
    await Effect.runPromise(h.runtime.prompt(session.id, text, PEOPLE));
    await idleAfter(k);
  }
  await flush(stores);
  return { server, dir, store, session: store.get(session.id)!, stores };
}

describe("compaction in a session", () => {
  test("a_call_past_75_percent_compacts_first: one summary call, a summary node, then the summary instead of the older runs", async () => {
    const { server, store, session } = await compactedSession();
    const bodies = server.requests().map((r) => r.json as { messages: Record<string, unknown>[]; tools: unknown });
    expect(bodies).toHaveLength(5);
    // The first prompt once: stored before the session opened, it is not rebuilt into its own run.
    expect(bodies[0]!.messages).toHaveLength(2);
    const [, run2a, summaryCall, run2b, run3] = bodies as [any, any, any, any, any];
    // The summary call: the same system prompt and tools, the runs before the current one (run 1),
    // then the compaction prompt; run 2's own messages are not in it.
    expect(JSON.stringify(summaryCall.messages[0])).toBe(JSON.stringify(run2a.messages[0]));
    expect(JSON.stringify(summaryCall.tools)).toBe(JSON.stringify(run2a.tools));
    expect(summaryCall.messages.slice(1, 3)).toEqual(run2a.messages.slice(1, 3));
    expect(summaryCall.messages.at(-1)).toEqual({ role: "user", content: COMPACTION_PROMPT });
    expect(summaryCall.messages).toHaveLength(4);
    expect(summaryCall.messages.map(textsOf).join()).not.toContain("second");
    // The next request: system prompt and tools byte-identical, the summary for run 1, run 2 kept.
    expect(JSON.stringify(run2b.messages[0])).toBe(JSON.stringify(run2a.messages[0]));
    expect(JSON.stringify(run2b.tools)).toBe(JSON.stringify(run2a.tools));
    const summaryText = (summaryMessage(SUMMARY) as unknown as { content: { text: string }[] }).content[0]!.text;
    expect(run2b.messages[1]).toEqual({ role: "user", content: summaryText });
    expect(run2b.messages.slice(2)).toEqual(run2a.messages.slice(3).concat(run2b.messages.slice(-2)));
    expect(run2b.messages.map(textsOf).join()).not.toMatch(/"first"|reply one/);
    // Run 3 starts from the compacted conversation and does not compact again.
    expect(run3.messages[1]).toEqual(run2b.messages[1]);
    expect(run3.messages.map(textsOf).join()).toContain("reply two");
    // The summary node, in run 2, right after its call's usage.
    const runs = store.runs(session.id);
    const nodes = store.nodes(session.id, 1, 1000);
    const summary = nodes.filter((n) => n.kind === "summary");
    expect(summary).toHaveLength(1);
    expect(summary[0]).toMatchObject({ runId: runs[1]!.id, payload: { prompt: COMPACTION_PROMPT_ID, text: SUMMARY } });
    const at = nodes.indexOf(summary[0]!);
    expect(nodes[at - 1]!.kind).toBe("usage");
    // Charged to the session like any call: five calls' tokens.
    expect(session.usage.input).toBe(100 + 5990 + 5000 + 900 + 1000);
    expect(session.usage.output).toBe(10 + 10 + 40 + 10 + 10);
  });

  test("the_compaction_turn_is_journaled_with_purpose_compaction", async () => {
    const { dir } = await compactedSession();
    for (const sub of ["events", "agent-traces"] as const) {
      const turns = agentEntries(dir, sub).filter((e) => e.kind === "agent.turn");
      expect(turns.map((t) => t.payload.purpose ?? null), sub).toEqual([null, null, "compaction", null, null]);
      expect(turns[2]!.payload.outputTokens).toBe(40);
    }
  });

  test("replay_after_compaction: the deltas replay to every request's bodyHash; the turn after compaction from 0", async () => {
    const { dir, server } = await compactedSession();
    const turns = agentEntries(dir, "agent-traces").filter((e) => e.kind === "agent.turn");
    expect(turns).toHaveLength(5);
    const parts = turns.map((t) => ({
      run: t.payload.runId as string,
      turnId: t.payload.turnId as string,
      delta: content(dir, t.payload.messagesDelta) as MessagesDelta,
      system: JSON.stringify(content(dir, t.payload.system)),
      tools: JSON.stringify(content(dir, t.payload.tools)),
      bodyBlake3: t.payload.bodyBlake3 as string,
    }));
    const sent = server.requests().map((r) => r.body.toString("utf8"));
    const replayed: (string | undefined)[] = [];
    for (const run of [...new Set(parts.map((p) => p.run))]) replayed.push(...replayBodies(parts.filter((p) => p.run === run)));
    expect(replayed).toEqual(sent);
    for (const b of sent) expect(turns.some((t) => t.payload.bodyBlake3 === `blake3:${blake3Hex(Buffer.from(b, "utf8"))}`)).toBe(true);
    // Run 2: its first call, the summary call (based on it), the call after compaction (from 0).
    const run2 = parts.filter((p) => p.run === parts[1]!.run).map((p) => p.delta);
    expect(run2.map((d) => ("from" in d ? d.from : "raw"))).toEqual([0, 2, 0]);
    expect(run2[1]).toMatchObject({ baseTurnId: parts[1]!.turnId });
    expect(run2[2]).not.toHaveProperty("baseTurnId");
  });

  test("resume_after_a_crash_mid_compaction_rebuilds_the_same_history: the summary node is the boundary", async () => {
    const { store, session, server } = await compactedSession();
    const nodes = store.nodes(session.id, 1, 1000);
    const at = nodes.findIndex((n) => n.kind === "summary");
    const strip = (ms: ModelMessage[]) => ms.map((m) => (m.role === "user" ? { role: "user", content: [m.content.at(-1)] } : m));
    // A crash after the summary call and before its node: the conversation is as before it.
    const crashed = historyFromNodes(nodes.slice(0, at), LOOP_LIMITS);
    const beforeCall = historyFromNodes(nodes.slice(0, at - 1), LOOP_LIMITS);
    expect(crashed).toEqual(beforeCall);
    expect(crashed.map(textsOf).join()).toContain("reply one");
    // After the node: the summary, then run 2 as sent after compaction (facts are not stored).
    const after = historyFromNodes(nodes.slice(0, at + 1), LOOP_LIMITS);
    expect(after[0]).toEqual(summaryMessage(SUMMARY));
    const run2b = (server.requests()[3]!.json as { messages: unknown[] }).messages.slice(1);
    expect(after).toHaveLength(run2b.length);
    // The whole session: what run 3 sent, but run 3's own reply appended.
    const all = historyFromNodes(nodes, LOOP_LIMITS);
    expect(strip(all.slice(0, -1))).toEqual(strip(liveOf(server.requests()[4]!.json)));
    expect(all.at(-1)).toEqual({ role: "assistant", text: "reply three" });
  });
});

/** A wire body's messages (after the system message) in the loop's own form. */
function liveOf(json: Record<string, unknown>): ModelMessage[] {
  return (json.messages as Array<Record<string, any>>).slice(1).map((m): ModelMessage => {
    if (m.role === "user") return { role: "user", content: typeof m.content === "string" ? [{ type: "text", text: m.content }] : m.content };
    if (m.role === "tool") return { role: "tool", toolCallId: m.tool_call_id, content: m.content };
    return {
      role: "assistant",
      ...(m.content === null ? {} : { text: m.content }),
      ...(m.tool_calls === undefined ? {} : { toolCalls: m.tool_calls.map((c: any) => ({ id: c.id, name: c.function.name, arguments: c.function.arguments })) }),
    };
  });
}

// The loop alone, with a scripted client ----------------------------------------------------------

type Reply = { text?: string; usage?: Partial<typeof ZERO_USAGE>; fail?: FinError };

function scripted(replies: Reply[]): ModelClient & { requests: ModelRequest[] } {
  const requests: ModelRequest[] = [];
  return {
    preset: "openrouter",
    warnings: [],
    requests,
    wireBody: (req) => req as unknown as Record<string, unknown>,
    async *stream(req): AsyncIterable<ModelStreamEvent> {
      requests.push(structuredClone(req));
      const r = replies[Math.min(requests.length - 1, replies.length - 1)]!;
      if (r.fail) throw r.fail;
      if (r.text) yield { type: "textDelta", text: r.text };
      const result: ModelResult = {
        text: r.text ?? "",
        toolCalls: [],
        reasoningText: "",
        reasoning: [],
        stopReason: "endTurn",
        usage: { ...ZERO_USAGE, input: 10, output: 5, ...r.usage },
        usageEstimated: false,
        capture: { request: "{}", response: [] },
        bodyHash: "blake3:00",
        latencyMs: 1,
      };
      yield { type: "done", result };
    },
  };
}

const openGate: CallGate = { beforeCall: () => ({ ok: true }), release: () => {} };

async function drive(client: ModelClient, nodes?: BuiltinDeps["nodes"]) {
  const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client, ...(nodes === undefined ? {} : { nodes }) });
  const scope = await Effect.runPromise(Scope.make());
  cleanups.push(() => Effect.runPromise(Scope.close(scope, Exit.void)));
  const session: ProviderSession = await Effect.runPromise(
    Scope.provide(scope)(adapter.open({ sessionId: "S1", projectId: "P1", provider: "openrouter", model: MODEL, role: "editor", mode: "draft", system: "You edit.", cwd: "/w", env: {}, providerRef: null, gate: openGate })),
  );
  const events: ProviderEvent[] = [];
  let ended: ((e: ProviderEvent) => void) | null = null;
  Effect.runFork(
    Stream.runForEach(session.events, ({ event }) =>
      Effect.sync(() => {
        events.push(event);
        if (event.type === "turnCompleted" || event.type === "turnFailed") ended?.(event);
      }),
    ),
  );
  const turn = (text: string) =>
    new Promise<ProviderEvent>((resolve, reject) => {
      ended = resolve;
      Effect.runPromise(session.startTurn({ runId: "R", text })).catch(reject);
    });
  return { session, events, turn };
}

describe("compaction in the loop", () => {
  test("a_compaction_that_does_not_fit_ends_the_run_with_MODEL_CONTEXT_EXCEEDED", async () => {
    // Run 1 reports 6,700 prompt tokens: run 2 compacts, and the summary call would pass 85 %.
    const client = scripted([{ text: "one", usage: { input: 6700 } }, { text: "never" }]);
    const d = await drive(client);
    expect(await d.turn("first")).toMatchObject({ type: "turnCompleted" });
    expect(await d.turn("second")).toMatchObject({ type: "turnFailed", error: { code: "MODEL_CONTEXT_EXCEEDED", details: { reason: "compactionTooLarge", window: 8000 } } });
    expect(client.requests).toHaveLength(1);
    expect(d.events.filter((e) => e.type === "summary")).toEqual([]);
  });

  test("a_compaction_that_fails_ends_the_run_with_MODEL_CONTEXT_EXCEEDED", async () => {
    for (const second of [{ text: "   " }, { fail: Object.assign(new Error("down"), { code: "PROVIDER_UNAVAILABLE" }) as unknown as FinError }]) {
      const client = scripted([{ text: "one", usage: { input: 6100 } }, second, { text: "never" }]);
      const d = await drive(client);
      await d.turn("first");
      expect(await d.turn("second")).toMatchObject({ type: "turnFailed", error: { code: "MODEL_CONTEXT_EXCEEDED", details: { reason: "compactionFailed" } } });
      expect(client.requests).toHaveLength(2);
      expect(client.requests[1]!.messages.at(-1)).toEqual({ role: "user", content: [{ type: "text", text: COMPACTION_PROMPT }] });
    }
  });

  test("nothing_before_the_current_run: no summary call; the 85 % stop stays the backstop", async () => {
    const client = scripted([{ text: "fits", usage: { input: 10 } }]);
    const d = await drive(client);
    // About 6,250 tokens of prompt in the first run: past 75 %, below 85 %, nothing to compact.
    expect(await d.turn("x".repeat(24_500))).toMatchObject({ type: "turnCompleted", stopReason: "endTurn" });
    expect(client.requests).toHaveLength(1);
    expect(d.events.some((e) => e.type === "summary")).toBe(false);
  });

  test("the_summary_call_is_quiet_and_its_events_are_capture_usage_summary", async () => {
    const client = scripted([{ text: "one", usage: { input: 6100 } }, { text: SUMMARY, usage: { input: 6100, output: 40 } }, { text: "two" }]);
    const d = await drive(client);
    await d.turn("first");
    const from = d.events.length;
    await d.turn("second");
    const types = d.events.slice(from).map((e) => e.type);
    // The summary is not streamed as a reply: no delta before its capture.
    expect(types).toEqual(["turnStarted", "capture", "usage", "summary", "messageDelta", "capture", "usage", "message", "turnCompleted"]);
    const capture = d.events.slice(from).find((e) => e.type === "capture") as Extract<ProviderEvent, { type: "capture" }>;
    expect(capture.response).toMatchObject({ purpose: "compaction" });
    expect(d.events.find((e) => e.type === "summary")).toEqual({ type: "summary", prompt: COMPACTION_PROMPT_ID, text: SUMMARY });
    expect(client.requests[2]!.messages).toEqual([summaryMessage(SUMMARY), { role: "user", content: [{ type: "text", text: "second" }] }]);
    expect(client.requests[2]!.system).toBe(client.requests[0]!.system);
    expect(JSON.stringify(client.requests[2]!.tools)).toBe(JSON.stringify(client.requests[0]!.tools));
  });
});

// Rollback and compaction (7B Task 8) ------------------------------------------------------------

describe("compaction and rollback", () => {
  let seq = 0;
  const node = (runId: string, kind: Node["kind"], payload: unknown): Node => ({ id: `n${++seq}`, runId, sessionId: "S1", seq, kind, payload, createdAt: 0 });
  const run = (id: string, prompt: string, reply: string): Node[] => [
    node(id, "userMessage", { text: prompt, trigger: "prompt" }),
    node(id, "usage", {}),
    node(id, "assistantMessage", { role: "assistant", blocks: [{ type: "text", text: reply }] }),
  ];
  const user = (text: string): ModelMessage => ({ role: "user", content: [{ type: "text", text }] });
  const said = (text: string): ModelMessage => ({ role: "assistant", text });

  test("rolling_back_to_a_run_the_summary_covers_rolls_the_summary_back_too", async () => {
    // Run 3 compacted runs 1–2; run 4 came after.
    const r3: Node[] = [node("R3", "userMessage", { text: "three", trigger: "prompt" }), node("R3", "usage", {}), node("R3", "summary", { prompt: COMPACTION_PROMPT_ID, text: "S" }), node("R3", "usage", {}), node("R3", "assistantMessage", { role: "assistant", blocks: [{ type: "text", text: "r3" }] })];
    const nodes = [...run("R1", "one", "r1"), ...run("R2", "two", "r2"), ...r3, ...run("R4", "four", "r4")];
    const back = (after: string[]) => nodes.map((n) => (after.includes(n.runId) ? { ...n, rolledBack: true } : n));
    expect(historyFromNodes(nodes, LOOP_LIMITS)).toEqual([summaryMessage("S"), user("three"), said("r3"), user("four"), said("r4")]);
    // To run 3: the summary's own run stays, and with it the summary.
    expect(historyFromNodes(back(["R4"]), LOOP_LIMITS)).toEqual([summaryMessage("S"), user("three"), said("r3")]);
    // To run 2 (or 1): the summary's run is rolled back; the runs it covered come back whole.
    expect(historyFromNodes(back(["R3", "R4"]), LOOP_LIMITS)).toEqual([user("one"), said("r1"), user("two"), said("r2")]);
    expect(historyFromNodes(back(["R2", "R3", "R4"]), LOOP_LIMITS)).toEqual([user("one"), said("r1")]);
    // A second summary replaces the first with everything before its own run.
    const r5 = [node("R5", "userMessage", { text: "five", trigger: "prompt" }), node("R5", "usage", {}), node("R5", "summary", { prompt: COMPACTION_PROMPT_ID, text: "S2" })];
    expect(historyFromNodes([...nodes, ...r5], LOOP_LIMITS)).toEqual([summaryMessage("S2"), user("five")]);
    // The live loop rebuilds from the marked nodes on rollback, the summary node's blob read.
    let stored: Node[] = nodes;
    const client = scripted([{ text: "five" }]);
    const d = await drive(client, async () => stored);
    stored = back(["R3", "R4"]);
    await Effect.runPromise(d.session.rollback(2));
    await d.turn("again");
    expect(client.requests[0]!.messages).toEqual([user("one"), said("r1"), user("two"), said("r2"), user("again")]);
  });
});
