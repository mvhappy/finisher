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
import { COMPACTION_PROMPT, COMPACTION_PROMPT_ID, SUMMARY_HEADER, summaryMessage } from "../src/agent/builtin/compact.ts";
import { type ModelClient, type ModelResult, type ModelStreamEvent, openrouterClient } from "../src/agent/builtin/compat.ts";
import { BuiltinAdapter, type BuiltinDeps, LOOP_LIMITS } from "../src/agent/builtin/loop.ts";
import { historyFromNodes } from "../src/agent/builtin/loop-history.ts";
import { TOOL_DEFS } from "../src/agent/builtin/tools.ts";
import type { ModelMessage, ModelRequest, ToolCall } from "../src/agent/builtin/request.ts";
import { Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentJournal } from "../src/agent/journal.ts";
import { type MessagesDelta, replayBodies } from "../src/agent/journal-content.ts";
import { cataloguePricing } from "../src/agent/runtime-deps.ts";
import { AgentStore } from "../src/agent/store.ts";
import { allNodes } from "../src/agent/wiring.ts";
import { openControlStore } from "../src/control/db.ts";
import { finError } from "../src/errors.ts";
import { agentEntries, content, flush, openStores, P1 } from "./support/agent-journal.ts";
import { type FakeResponse, startFakeModelServer } from "./support/fake-model-server.ts";
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
function toolSse(callId: string, argv: string[], prompt: number, completion = 10, name = "fin", input: unknown = { argv }): string {
  const id = `gen-${callId}`;
  const call = { index: 0, id: callId, type: "function", function: { name, arguments: JSON.stringify(input) } };
  return (
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: null, tool_calls: [call] }, finish_reason: null }] }) +
    chunk({ id, provider: "DeepSeek", object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }], usage: usageOf(prompt, completion) }) +
    "data: [DONE]\n\n"
  );
}

const SUMMARY = "The person asked for the first cut; it is done. Nothing is pending.";
const textsOf = (m: ModelMessage | Record<string, unknown>): string => JSON.stringify(m);
/** The text of a summary's message (one text part). */
const summaryText = (text: string): string => (summaryMessage(text) as unknown as { content: { text: string }[] }).content[0]!.text;

// A whole session through the runtime, the real client and the journal ------------------------

/** A server, the stores and a runtime over them; `restart` is a new store, adapter and runtime on the same `control.db` (a reopen). */
async function harness(replies: readonly FakeResponse[]) {
  const server = await startFakeModelServer(replies);
  cleanups.push(() => server.close());
  const dir = mkdtempSync(join(tmpdir(), "agent-compaction-"));
  const stores = openStores(dir, { diagnostics: "on", training: "on" });
  const clock = manualClock(Date.now());
  const control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  const client = openrouterClient({ testBaseUrl: server.baseUrl, cachingFor: () => "none", clock });
  const gateway = { withKey: <A>(_p: string, _s: string, f: (key: string) => Promise<A>) => f("sk-or-v1-test") };
  const tool = new FinTool();
  const open = () => {
    const store = new AgentStore(control, dir, clock, systemIds(clock));
    const journal = new AgentJournal({ diagnostics: stores.diag, traces: stores.traces, store, ids: systemIds(clock), clock });
    // Every node's payload, its blob read (`wiring.ts`), as the server rebuilds a conversation.
    const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client, gateway, nodes: (sid) => allNodes(store, sid) });
    return { store, h: makeRuntime({ store, control }, adapter, { clock, journal, tools: { builtin: tool }, pricing: cataloguePricing(CATALOGUE) }) };
  };
  let live = open();
  cleanups.push(async () => {
    await live.h.runtime.close();
    await control.close();
    await stores.close();
    rmSync(dir, { recursive: true, force: true });
  });
  await live.h.runtime.boot();
  const t = {
    server,
    dir,
    stores,
    get store() {
      return live.store;
    },
    get runtime() {
      return live.h.runtime;
    },
    async start(prompt: string) {
      const { session } = await Effect.runPromise(live.h.runtime.start({ projectId: P1, harness: "builtin", model: MODEL, prompt }, PEOPLE, META));
      return session.id;
    },
    idleAfter: (id: string, count: number) =>
      until(() => live.store.get(id)!.status === "idle" && live.store.runs(id).length === count && live.store.runs(id).at(-1)!.status !== "running", () => `run ${count}: ${JSON.stringify({ s: live.store.get(id), runs: live.store.runs(id), nodes: live.store.nodes(id, 1, 1000).map((n) => [n.kind, n.payload]) })}`),
    /** A crash (nothing more is written), then the restart. */
    async restart() {
      await live.h.runtime.abandon();
      live = open();
      return live.h.runtime.boot();
    },
  };
  return t;
}

/**
 * Three runs. Run 1 ends at once. Run 2's first call reports 5,990 prompt tokens and calls a
 * tool: its next call would pass 75 %, so the loop compacts run 1 first, mid-run. Run 3 fits.
 */
async function compactedSession(summary = SUMMARY, more: readonly FakeResponse[] = []) {
  const t = await harness([
    { sse: textSse("reply one", 100) },
    { sse: toolSse("call_s", ["status"], 5990) },
    { sse: textSse(summary, 5000, 40) },
    { sse: textSse("reply two", 900) },
    { sse: textSse("reply three", 1000) },
    ...more,
  ]);
  const id = await t.start("first");
  await t.idleAfter(id, 1);
  for (const [k, text] of [
    [2, "second"],
    [3, "third"],
  ] as const) {
    await Effect.runPromise(t.runtime.prompt(id, text, PEOPLE));
    await t.idleAfter(id, k);
  }
  await flush(t.stores);
  return { ...t, server: t.server, dir: t.dir, store: t.store, session: t.store.get(id)!, stores: t.stores };
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

describe("resume and the rebuilt conversation", () => {
  type Wire = Array<Record<string, any>>;
  const wire = (t: { server: { requests(): Array<{ json: Record<string, unknown> }> } }) => t.server.requests().map((r) => r.json.messages as Wire);
  /** A user message's prompt: its last text part (a turn's facts come before it and are not stored). */
  const said = (m: Record<string, any>) => (typeof m.content === "string" ? m.content : (m.content as Array<{ text: string }>).at(-1)!.text);
  const count = (hay: string, needle: string) => hay.split(needle).length - 1;

  test("a_resume_opens_a_new_instance_whose_first_request_holds_the_prompt_once", async () => {
    const t = await harness([{ sse: textSse("reply one", 100) }, { sse: textSse("reply two", 120) }]);
    const id = await t.start("first prompt");
    await t.idleAfter(id, 1);
    await Effect.runPromise(t.runtime.interrupt(id, PEOPLE));
    expect(t.runtime.stats().live).toBe(0);
    await Effect.runPromise(t.runtime.resume(id, PEOPLE, "the resumed prompt"));
    await t.idleAfter(id, 2);
    const [one, two] = wire(t) as [Wire, Wire];
    expect(one).toHaveLength(2);
    expect(two.map((m) => m.role)).toEqual(["system", "user", "assistant", "user"]);
    expect(count(JSON.stringify(two), "first prompt")).toBe(1);
    expect(count(JSON.stringify(two), "the resumed prompt")).toBe(1);
    expect(said(two[3]!)).toBe("the resumed prompt");
  });

  test("large_payloads_survive_a_resume_rebuild_byte_identical: a prompt and a workspace_write input past 4 KiB", async () => {
    const big = `begin ${"prompt words ".repeat(400)}end`;
    const content = `head ${"file line\n".repeat(500)}tail`;
    const t = await harness([
      { sse: toolSse("call_w", [], 2000, 10, "workspace_write", { path: "notes.txt", content }) },
      { sse: textSse("done", 2100) },
      { sse: textSse("again done", 2200) },
    ]);
    const id = await t.start(big);
    await t.idleAfter(id, 1);
    // Both stored as blobs: the rebuild has to read them.
    const nodes = t.store.nodes(id, 1, 1000);
    expect(nodes.filter((n) => n.blobRef !== undefined).map((n) => n.kind).sort()).toEqual(["toolCall", "userMessage"]);
    await Effect.runPromise(t.runtime.interrupt(id, PEOPLE));
    await Effect.runPromise(t.runtime.resume(id, PEOPLE, "again"));
    await t.idleAfter(id, 2);
    const [, first, resumed] = wire(t) as [Wire, Wire, Wire];
    expect(resumed).toHaveLength(first.length + 2);
    expect(said(resumed[1]!)).toBe(big);
    // The call and its result: the same bytes as sent before the resume.
    for (const i of [2, 3]) expect(JSON.stringify(resumed[i])).toBe(JSON.stringify(first[i]));
    expect(JSON.parse(resumed[2]!.tool_calls[0].function.arguments)).toEqual({ path: "notes.txt", content });
    expect(resumed[4]).toEqual({ role: "assistant", content: "done" });
  });

  test("a_summary_past_4_KiB_is_honoured_after_a_resume: no compaction again", async () => {
    const big = `${SUMMARY}${" detail".repeat(800)}`;
    const t = await compactedSession(big, [{ sse: textSse("reply four", 1100) }]);
    expect(t.store.nodes(t.session.id, 1, 1000).find((n) => n.kind === "summary")!.blobRef).toBeDefined();
    await Effect.runPromise(t.runtime.interrupt(t.session.id, PEOPLE));
    await Effect.runPromise(t.runtime.resume(t.session.id, PEOPLE, "fourth"));
    await t.idleAfter(t.session.id, 4);
    const all = wire(t);
    expect(all).toHaveLength(6);
    const four = all[5]!;
    expect(four[1]).toEqual({ role: "user", content: summaryText(big) });
    expect(JSON.stringify(four)).not.toMatch(/reply one/);
    expect(JSON.stringify(four)).toMatch(/reply three/);
  });

  test("a_resume_after_a_crash_mid_compaction_compacts_again_through_a_reopened_adapter", async () => {
    // Run 1's prompt is large for real (a fresh estimate after the restart sees it); its call
    // reports 6,100 prompt tokens, so run 2 compacts; the summary call never answers: a crash.
    const tools = Math.ceil(JSON.stringify(TOOL_DEFS).length / 4);
    const big = "w".repeat(4 * (6300 - tools));
    const t = await harness([
      { sse: textSse("reply one", 6100) },
      { sse: textSse("never", 1), stallAfter: 0 },
      { sse: textSse(SUMMARY, 6300, 40) },
      { sse: textSse("reply three", 900) },
    ]);
    const id = await t.start(big);
    await t.idleAfter(id, 1);
    await Effect.runPromise(t.runtime.prompt(id, "second", PEOPLE));
    await until(() => t.server.requests().length === 2, "the summary call");
    const rec = await t.restart();
    expect(rec.interrupted).toEqual([id]);
    expect(t.store.nodes(id, 1, 1000).some((n) => n.kind === "summary")).toBe(false);
    await Effect.runPromise(t.runtime.resume(id, PEOPLE, "third"));
    await t.idleAfter(id, 3);
    const [, crashed, again, after] = wire(t) as [Wire, Wire, Wire, Wire];
    for (const call of [crashed, again]) expect(call.at(-1)).toEqual({ role: "user", content: COMPACTION_PROMPT });
    // The rebuilt conversation is whole before the summary: run 1 (its prompt read from its
    // blob), run 2's prompt; then the summary instead of them, run 3 kept.
    expect(again.slice(1, -1).map((m) => (m.role === "user" ? said(m) : m.content))).toEqual([big, "reply one", "second"]);
    expect(said(crashed[1]!)).toBe(big);
    expect(after.slice(1).map((m) => said(m))).toEqual([summaryText(SUMMARY), "third"]);
    const summary = t.store.nodes(id, 1, 1000).filter((n) => n.kind === "summary");
    expect(summary.map((n) => n.runId)).toEqual([t.store.runs(id)[2]!.id]);
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

type Reply = { text?: string; usage?: Partial<typeof ZERO_USAGE>; fail?: FinError; stopReason?: ModelResult["stopReason"]; toolCalls?: ToolCall[] };

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
        toolCalls: r.toolCalls ?? [],
        reasoningText: "",
        reasoning: [],
        stopReason: r.stopReason ?? (r.toolCalls ? "toolUse" : "endTurn"),
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

async function drive(client: ModelClient, nodes?: BuiltinDeps["nodes"], gate: CallGate = openGate) {
  const adapter = BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client, ...(nodes === undefined ? {} : { nodes }) });
  const scope = await Effect.runPromise(Scope.make());
  cleanups.push(() => Effect.runPromise(Scope.close(scope, Exit.void)));
  const session: ProviderSession = await Effect.runPromise(
    Scope.provide(scope)(adapter.open({ sessionId: "S1", projectId: "P1", provider: "openrouter", model: MODEL, role: "editor", mode: "draft", system: "You edit.", cwd: "/w", env: {}, providerRef: null, gate })),
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

const notices = (events: ProviderEvent[], name: string) => events.filter((e): e is Extract<ProviderEvent, { type: "notice" }> => e.type === "notice" && e.name === name).map((e) => e.details);
const prompt = (text: string): ModelMessage => ({ role: "user", content: [{ type: "text", text }] });

describe("compaction in the loop", () => {
  test("a_compaction_that_does_not_fit_falls_back_to_the_uncompacted_request: it fits under 85 %, so it is sent", async () => {
    // Run 1 reports 6,700 prompt tokens: run 2 compacts, and the summary call would pass 85 %.
    const client = scripted([{ text: "one", usage: { input: 6700 } }, { text: "two" }]);
    const d = await drive(client);
    expect(await d.turn("first")).toMatchObject({ type: "turnCompleted" });
    expect(await d.turn("second")).toMatchObject({ type: "turnCompleted", stopReason: "endTurn" });
    expect(client.requests).toHaveLength(2);
    expect(client.requests[1]!.messages).toEqual([prompt("first"), { role: "assistant", text: "one" }, prompt("second")]);
    expect(notices(d.events, "compactionSkipped")).toEqual([expect.objectContaining({ reason: "tooLarge", window: 8000 })]);
    expect(d.events.filter((e) => e.type === "summary")).toEqual([]);
  });

  test("a_compaction_that_does_not_fit_and_a_request_past_85_percent_ends_the_run_with_MODEL_CONTEXT_EXCEEDED", async () => {
    const client = scripted([{ text: "one", usage: { input: 6790 } }, { text: "never" }]);
    const d = await drive(client);
    await d.turn("first");
    expect(await d.turn(`second ${"x".repeat(600)}`)).toMatchObject({ type: "turnFailed", error: { code: "MODEL_CONTEXT_EXCEEDED", details: { window: 8000, compaction: "tooLarge" } } });
    expect(client.requests).toHaveLength(1);
    expect(notices(d.events, "compactionSkipped")).toEqual([expect.objectContaining({ reason: "tooLarge" })]);
  });

  test("a_compaction_that_cannot_help_is_not_made: the current run alone would still pass 85 %", async () => {
    const client = scripted([{ text: "one", usage: { input: 10 } }, { text: "two" }]);
    const d = await drive(client);
    await d.turn("first");
    // About 6,760 tokens of prompt in run 2: past 75 %, below 85 %; with a summary in place of
    // run 1 (its budget, `maxTokens`, and the tools counted afresh) it would pass 85 %.
    expect(await d.turn("x".repeat(4 * 6760))).toMatchObject({ type: "turnCompleted", stopReason: "endTurn" });
    expect(client.requests).toHaveLength(2);
    expect(client.requests[1]!.messages).toHaveLength(3);
    expect(notices(d.events, "compactionSkipped")).toEqual([expect.objectContaining({ reason: "cannotHelp" })]);
  });

  const fallThrough: Array<[string, Reply, Record<string, unknown>]> = [
    ["empty", { text: "   " }, { reason: "empty" }],
    ["toolOnly", { toolCalls: [{ id: "c1", name: "fin", arguments: "{}" }] }, { reason: "toolCalls" }],
    ["toolsAndText", { text: "Let me look first.", toolCalls: [{ id: "c1", name: "fin", arguments: "{}" }] }, { reason: "toolCalls" }],
    ["truncated", { text: "The person asked for", stopReason: "maxTokens" }, { reason: "truncated" }],
    ["unavailable", { fail: finError("PROVIDER_UNAVAILABLE", "down", {}) }, { reason: "callFailed", cause: "PROVIDER_UNAVAILABLE" }],
    ["rateLimited", { fail: finError("PROVIDER_RATE_LIMITED", "slow down", {}) }, { reason: "callFailed", cause: "PROVIDER_RATE_LIMITED" }],
    ["overloaded", { fail: finError("PROVIDER_OVERLOADED", "timed out", {}) }, { reason: "callFailed", cause: "PROVIDER_OVERLOADED" }],
  ];
  test.each(fallThrough)("a_summary_that_cannot_be_used_falls_back_to_the_uncompacted_request (%s)", async (_name, second, notice) => {
    const client = scripted([{ text: "one", usage: { input: 6100 } }, second, { text: "two" }]);
    const d = await drive(client);
    await d.turn("first");
    const from = d.events.length;
    expect(await d.turn("second")).toMatchObject({ type: "turnCompleted", stopReason: "endTurn" });
    expect(client.requests).toHaveLength(3);
    expect(client.requests[1]!.messages.at(-1)).toEqual(prompt(COMPACTION_PROMPT));
    expect(client.requests[2]!.messages).toEqual([prompt("first"), { role: "assistant", text: "one" }, prompt("second")]);
    expect(notices(d.events, "compactionSkipped")).toEqual([expect.objectContaining(notice)]);
    expect(d.events.some((e) => e.type === "summary")).toBe(false);
    // A summary call that answered stays charged and journaled: its capture (marked) and usage.
    const own = d.events.slice(from);
    const captures = own.filter((e): e is Extract<ProviderEvent, { type: "capture" }> => e.type === "capture");
    if (second.fail === undefined) {
      expect(captures.map((c) => (c.response as { purpose?: string }).purpose ?? null)).toEqual(["compaction", null]);
      expect(own.filter((e) => e.type === "usage")).toHaveLength(2);
    } else {
      expect(captures.map((c) => (c.response as { purpose?: string }).purpose ?? null)).toEqual([null]);
    }
    // Nothing of the summary reply is streamed or kept.
    expect(own.filter((e) => e.type === "message" || e.type === "toolStarted").length).toBe(1);
  });

  test("a_budget_refusal_of_the_summary_call_is_a_budget_stop", async () => {
    const budget = finError("BUDGET_EXCEEDED", "the session's tokens are used up", { kind: "tokens" });
    // The gate refuses the summary call; and a budget error thrown by the call is one too.
    let calls = 0;
    const gate: CallGate = { beforeCall: () => (++calls === 2 ? { ok: false, error: budget } : { ok: true }), release: () => {} };
    const refused = scripted([{ text: "one", usage: { input: 6100 } }, { text: "never" }]);
    const d = await drive(refused, undefined, gate);
    await d.turn("first");
    expect(await d.turn("second")).toMatchObject({ type: "turnFailed", error: { code: "BUDGET_EXCEEDED" } });
    expect(refused.requests).toHaveLength(1);
    const thrown = scripted([{ text: "one", usage: { input: 6100 } }, { fail: budget }, { text: "never" }]);
    const e = await drive(thrown);
    await e.turn("first");
    expect(await e.turn("second")).toMatchObject({ type: "turnFailed", error: { code: "BUDGET_EXCEEDED" } });
    expect(thrown.requests).toHaveLength(2);
    expect(notices(e.events, "compactionSkipped")).toEqual([]);
  });

  test("the_summary_message_is_marked_as_the_assistants_own_summary_not_the_persons_words", () => {
    const text = summaryText("S");
    expect(text.startsWith(SUMMARY_HEADER)).toBe(true);
    expect(SUMMARY_HEADER).toMatch(/written by you, the assistant/);
    expect(SUMMARY_HEADER).toMatch(/not new instructions from the person/);
    expect(text.endsWith("\n\nS")).toBe(true);
    expect(COMPACTION_PROMPT).toMatch(/quote the person's requests, marked as quotes/);
    expect(COMPACTION_PROMPT).not.toMatch(/in their own words/);
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
