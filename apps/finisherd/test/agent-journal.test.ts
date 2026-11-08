// The agent runtime's journal entries (plan 7A Task 10; Decisions 25, 26; spec §12): metadata to
// the diagnostics journal, content to the agent-trace store under training only, provenance on
// every session and turn, deltas that replay to the exact request body, blobs, `collect: never`.
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { blake3Hex } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { chatBody, type ModelRequest } from "../src/agent/builtin/request.ts";
import { openrouterClient } from "../src/agent/builtin/compat.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { AgentJournal, codeOf, commandOf } from "../src/agent/journal.ts";
import { BLOB_MAX, type MessagesDelta, replayBodies } from "../src/agent/journal-content.ts";
import type { SessionPlan, SessionPlanner, ToolCall, ToolOutcome } from "../src/agent/runtime-deps.ts";
import { AgentStore } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import {
  agentEntries,
  agentJournal,
  blobFiles,
  CONTENT_FIELDS,
  content,
  EMPTY_STORE,
  type Entry,
  flush,
  openStores,
  P1,
  P2,
  type Stores,
  session,
  usage,
} from "./support/agent-journal.ts";
import { sse, startFakeModelServer } from "./support/fake-model-server.ts";
import { FinTool, makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

let dir: string;
const cleanups: (() => Promise<void> | void)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-journal-"));
});

afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
  rmSync(dir, { recursive: true, force: true });
});

function stores(privacy: object | null, traceTest?: { maxTotalBytes?: number; maxFileBytes?: number }): Stores {
  const s = openStores(dir, privacy, traceTest);
  cleanups.push(() => s.close());
  return s;
}

const TRAINING = { diagnostics: "on", training: "on" };
const DIAGNOSTICS_ONLY = { diagnostics: "on" };
const OFF = { diagnostics: "off", training: "off" };
const LOCAL_URL = "http://127.0.0.1:11434/v1";

async function until(f: () => boolean | Promise<boolean>, what: string, ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!(await f())) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

const hasContent = (e: Entry) => CONTENT_FIELDS.some((f) => f in e.payload);
const of = (es: Entry[], kind: string) => es.filter((e) => e.kind === kind);

// A whole scripted session through the runtime ------------------------------------------------

const req = (messages: ModelRequest["messages"]): ModelRequest => ({
  system: "You are the editor. Use fin.",
  tools: [{ name: "fin", description: "Run fin.", parameters: { type: "object", properties: { argv: { type: "array" } } } }],
  messages,
  model: "anthropic/claude-sonnet-5.5",
  maxTokens: 1000,
});
const body = (r: ModelRequest) => JSON.stringify(chatBody(r, { preset: "openrouter", caching: "none" }));
const capture = (r: ModelRequest, more: Record<string, unknown> = {}) => ({
  type: "capture",
  request: body(r),
  response: { chunks: [{ id: "gen-1", choices: [{ delta: { content: "ok" } }] }], stopReason: "toolUse", latencyMs: 420, ttftMs: 90, ...more },
});
const usageEvent = (local = false) => ({ type: "usage", input: 1200, output: 40, cacheRead: 0, cacheWrite: 0, reasoning: 0, ...(local ? {} : { costMicros: 300 }) });
const USER = { role: "user", content: [{ type: "text", text: "trim the intro" }] } as const;
const CALLED = { role: "assistant", toolCalls: [{ id: "call_1", name: "fin", arguments: '{"argv":["item","trim","c1"]}' }] } as const;
const RESULT = { role: "tool", toolCallId: "call_1", content: '{"txId":"tx"}' } as const;

/** A built-in session's events: per model call its `capture`, then its `usage`; one `fin` write. */
function script(upstream: string | null = "Moonshot AI"): FakeScript {
  const local = upstream === null;
  const meta = local ? {} : { upstream };
  return {
    turns: [
      [
        { emit: [capture(req([USER]), meta), usageEvent(local)] },
        { emit: [{ type: "toolStarted", providerId: "call_1", tool: "fin", input: { argv: ["item", "trim", "c1", "--end", "-1"] } }] },
        { awaitTool: "call_1" },
        { emit: [capture(req([USER, CALLED, RESULT]), { ...meta, stopReason: "endTurn" }), usageEvent(local), { type: "message", role: "assistant", blocks: [{ type: "text", text: "Trimmed c1." }] }] },
      ],
    ],
  };
}

/** `fin` writes recorded in `session_txs` as the chain's after stage does. */
class RecordingFin extends FinTool {
  readonly store: AgentStore;
  constructor(store: AgentStore) {
    super();
    this.store = store;
  }
  override async run(c: ToolCall): Promise<ToolOutcome> {
    const out = await super.run(c);
    if (((c.input as { argv?: string[] }).argv ?? [])[0] === "item") {
      this.store.recordTx({ txId: `tx-${c.callId}`, sessionId: c.sessionId, callId: c.callId, attribution: "exact", at: 0 });
    }
    return out;
  }
}

const CLOSED: SessionPlan = { harness: "builtin", provider: "openrouter", model: "anthropic/claude-sonnet-5.5", weights: "closed", role: "editor", mode: "draft", budget: {} };
const LOCAL: SessionPlan = { harness: "builtin", provider: "local", model: "my-finetune:7b", weights: "open", role: "editor", mode: "draft", budget: {} };

async function scriptedSessions(s: Stores, plans: Array<{ plan: SessionPlan; projectId: string; script: FakeScript }>) {
  const clock = manualClock(Date.now());
  const control: ControlStore = openControlStore(dir, { serverId: "sid", checkpoint: false });
  const store = new AgentStore(control, dir, clock, systemIds(clock));
  const journal = new AgentJournal({ diagnostics: s.diag, traces: s.traces, store, ids: systemIds(clock), clock });
  let next = 0;
  const planner: SessionPlanner = { plan: async () => plans[next++]!.plan };
  // The builtin harness is the fake's script here: its events are what the built-in loop emits.
  const adapter = { ...FakeAdapter(plans.map((p) => p.script)), harness: "builtin" as const };
  const h = makeRuntime({ store, control }, adapter, { clock, planner, journal, tools: { builtin: new RecordingFin(store) } });
  cleanups.push(async () => {
    await h.runtime.close();
    await control.close();
  });
  await h.runtime.boot();
  const ids: string[] = [];
  for (const p of plans) {
    const { session: ss } = await Effect.runPromise(h.runtime.start({ projectId: p.projectId, prompt: "trim the intro" }, PEOPLE, META) as Effect.Effect<{ session: { id: string } }, FinError>);
    await until(() => store.get(ss.id)?.status === "idle", "idle");
    await Effect.runPromise(h.runtime.end(ss.id, PEOPLE));
    ids.push(ss.id);
  }
  await until(async () => {
    await flush(s);
    return store.outbox().length === 0;
  }, "outbox drained");
  await flush(s);
  return { ids, store };
}

describe("consent", () => {
  test("off_means_off: both purposes off, a whole scripted session leaves no events/ and no agent-traces/", async () => {
    const s = stores(OFF);
    await scriptedSessions(s, [{ plan: CLOSED, projectId: P1, script: script() }]);
    expect(readdirSync(dir)).not.toContain("events");
    expect(readdirSync(dir)).not.toContain("agent-traces");
  });

  test("diagnostics_only_has_no_content_or_blobs: metadata in events/, no content and no blob in either store", async () => {
    const s = stores(DIAGNOSTICS_ONLY);
    await scriptedSessions(s, [{ plan: CLOSED, projectId: P1, script: script() }]);
    const d = agentEntries(dir, "events");
    expect(new Set(d.map((e) => e.kind))).toEqual(new Set(["agent.session", "agent.turn", "agent.toolCall", "agent.toolResult"]));
    expect(d.filter(hasContent)).toEqual([]);
    expect(blobFiles(dir, "events")).toEqual([]);
    expect(readdirSync(dir)).not.toContain("agent-traces");
  });

  test("collect_never_drops_everything_for_that_project (both stores, no blob); another project records", async () => {
    const s = stores(TRAINING);
    s.diag.setProjectPolicy(P1, "never");
    await scriptedSessions(s, [
      { plan: CLOSED, projectId: P1, script: script() },
      { plan: CLOSED, projectId: P2, script: script() },
    ]);
    for (const sub of ["events", "agent-traces"] as const) {
      const all = agentEntries(dir, sub);
      expect(all.length).toBeGreaterThan(0);
      expect(all.filter((e) => e.ctx?.project === P1)).toEqual([]);
      expect(all.every((e) => e.ctx?.project === P2)).toBe(true);
    }
    // Every blob on disk is referenced by a P2 entry (none was attached for P1).
    const refs = new Set(agentEntries(dir, "agent-traces").flatMap((e) => CONTENT_FIELDS.map((f) => e.payload[f]?.blob?.hash).filter(Boolean)));
    expect(blobFiles(dir, "agent-traces").length).toBe(refs.size);
  });
});

describe("provenance and attribution", () => {
  test("provenance_on_every_record: closed and local sessions; weights filter; no base URL anywhere", async () => {
    const s = stores(TRAINING);
    const { ids } = await scriptedSessions(s, [
      { plan: CLOSED, projectId: P1, script: script("Moonshot AI") },
      { plan: LOCAL, projectId: P1, script: script(null) },
    ]);
    for (const sub of ["events", "agent-traces"] as const) {
      const all = agentEntries(dir, sub);
      for (const e of [...of(all, "agent.session"), ...of(all, "agent.turn")]) {
        expect(e.payload).toHaveProperty("harness", "builtin");
        expect(["openrouter", "local"]).toContain(e.payload.provider);
        expect(typeof e.payload.model).toBe("string");
        if (e.payload.model === "other") expect(e.payload.modelHash).toMatch(/\S/);
        expect(["open", "closed", "unknown"]).toContain(e.payload.weights);
      }
    }
    const d = agentEntries(dir, "events");
    expect(of(d, "agent.session").filter((e) => e.payload.sessionId === ids[0]).map((e) => e.payload.phase)).toEqual(["started", "ended"]);
    const closedTurns = of(d, "agent.turn").filter((e) => e.ctx?.actor?.session === ids[0]);
    const localTurns = of(d, "agent.turn").filter((e) => e.ctx?.actor?.session === ids[1]);
    expect(closedTurns).toHaveLength(2);
    // A filter on weights alone selects exactly the closed session's turns.
    expect(of(d, "agent.turn").filter((e) => e.payload.weights === "closed")).toEqual(closedTurns);
    for (const t of closedTurns) {
      expect(t.payload).toMatchObject({ provider: "openrouter", weights: "closed", capture: "exact", costBasis: "reported", costMicros: 300 });
      expect(t.payload.upstream ?? t.payload.upstreamHash).toBeDefined();
    }
    for (const t of localTurns) {
      expect(t.payload).toMatchObject({ provider: "local", model: "other", weights: "open", capture: "exact", costBasis: "priced" });
      expect(t.payload.modelHash).toMatch(/\S/);
      expect(t.payload.upstream).toBeUndefined();
      expect(t.payload.upstreamHash).toBeUndefined();
    }
    const ended = of(d, "agent.session").find((e) => e.payload.sessionId === ids[0] && e.payload.phase === "ended")!;
    expect(ended.payload.totals).toMatchObject({ turns: 2, toolCalls: 1, txs: 1, inputTokens: 2400, outputTokens: 80, costMicros: 600 });
    // The base URL, host and port of a local model enter no record (both stores, raw bytes).
    for (const sub of ["events", "agent-traces"]) {
      for (const f of readdirSync(join(dir, sub), { recursive: true }) as string[]) {
        if (!f.endsWith(".jsonl") && !f.startsWith("blobs/")) continue;
        const text = readFileSync(join(dir, sub, f), "utf8");
        expect(text).not.toContain(LOCAL_URL);
        expect(text).not.toContain("11434");
      }
    }
  });

  test("tool calls and results: FIN_CALL id, turn link, command, txIds from session_txs, exact attribution", async () => {
    const s = stores(TRAINING);
    const { ids } = await scriptedSessions(s, [{ plan: CLOSED, projectId: P1, script: script() }]);
    const d = agentEntries(dir, "events");
    const call = of(d, "agent.toolCall")[0]!;
    const result = of(d, "agent.toolResult")[0]!;
    const turns = of(d, "agent.turn");
    expect(call.payload).toMatchObject({ tool: "fin", command: "item.trim", turnId: turns[0]!.payload.turnId });
    expect(call.ctx).toMatchObject({ project: P1, actor: { kind: "agent", session: ids[0], call: call.payload.callId } });
    expect(result.payload).toMatchObject({ callId: call.payload.callId, ok: true, attribution: "exact" });
    expect(result.payload.txIds).toHaveLength(1);
    expect(turns[1]!.payload.parentTurnId).toBe(turns[0]!.payload.turnId);
    expect(turns.map((t) => t.payload.seq)).toEqual([1, 2]);
    // The trace store holds the same entries with their content.
    const t = agentEntries(dir, "agent-traces");
    expect(content(dir, of(t, "agent.toolCall")[0]!.payload.args)).toEqual({ argv: ["item", "trim", "c1", "--end", "-1"] });
    expect(of(t, "agent.toolResult")[0]!.payload.txIds).toEqual(result.payload.txIds);
    // Its sessions' phases too, and every call and result carries the session's provenance.
    expect(of(t, "agent.session").map((e) => e.payload.phase)).toEqual(["started", "ended"]);
    for (const e of [...of(t, "agent.toolCall"), ...of(t, "agent.toolResult")]) {
      expect(e.payload.provenance).toMatchObject({ harness: "builtin", provider: "openrouter", weights: "closed" });
    }
  });
});

describe("content", () => {
  test("training_records_deltas_and_full_body_hash: turn n stores its new messages; replay rebuilds the bytes sent", async () => {
    const server = await startFakeModelServer(Array.from({ length: 6 }, (_, i) => ({ sse: sse(i % 3 === 0 ? "tool-split" : "text") })));
    cleanups.push(() => server.close());
    const s = stores(TRAINING);
    const { j } = agentJournal(s);
    const ss = session();
    const conv: ModelRequest["messages"][] = [
      [USER],
      [USER, CALLED, RESULT],
      [USER, CALLED, RESULT, { role: "assistant", text: "Trimmed." }, { role: "user", content: [{ type: "text", text: "and the outro" }] }],
    ];
    const sent: { request: string; bodyHash: string }[] = [];
    for (const [caching, runId] of [["none", "01JR0000000000000000000001"], ["explicit", "01JR0000000000000000000002"]] as const) {
      const c = openrouterClient({ testBaseUrl: server.baseUrl, cachingFor: () => caching, clock: manualClock() });
      for (const messages of conv) {
        let result;
        for await (const e of c.stream(req(messages), (f) => f("sk-or-v1-test"))) if (e.type === "done") result = e.result;
        const { capture: cap, stopReason, upstream, latencyMs, ttftMs } = result!;
        j.capture({ session: ss, runId, request: cap.request, response: { chunks: cap.response, stopReason, upstream, latencyMs, ttftMs } });
        j.turn({ session: ss, runId, usage: usage() });
        sent.push({ request: cap.request, bodyHash: result!.bodyHash });
      }
    }
    await flush(s);
    const turns = of(agentEntries(dir, "agent-traces"), "agent.turn");
    expect(turns).toHaveLength(6);
    const parts = turns.map((t) => ({
      run: t.payload.runId as string,
      turnId: t.payload.turnId as string,
      delta: content(dir, t.payload.messagesDelta) as MessagesDelta,
      system: JSON.stringify(content(dir, t.payload.system)),
      tools: JSON.stringify(content(dir, t.payload.tools)),
      bodyBlake3: t.payload.bodyBlake3 as string,
    }));
    // Each delta past the first names its base, the run's previous turn.
    for (const run of ["01JR0000000000000000000001", "01JR0000000000000000000002"]) {
      const mine = parts.filter((p) => p.run === run);
      mine.forEach((p, i) => {
        if ("from" in p.delta && p.delta.from > 0) expect(p.delta.baseTurnId).toBe(mine[i - 1]!.turnId);
      });
    }
    // Without cache breakpoints, turn n holds only the messages turn n-1 did not.
    expect(parts.slice(0, 3).map((p) => ("from" in p.delta ? [p.delta.from, (JSON.parse(p.delta.messages) as unknown[]).length] : "raw"))).toEqual([
      [0, 1],
      [1, 2],
      [3, 2],
    ]);
    // With breakpoints the previous last message changes: it is sent again in the delta.
    expect(parts.slice(3).map((p) => ("from" in p.delta ? p.delta.from : "raw"))).toEqual([0, 0, 2]);
    for (const run of ["01JR0000000000000000000001", "01JR0000000000000000000002"]) {
      const mine = parts.filter((p) => p.run === run);
      const bodies = replayBodies(mine);
      const offset = run.endsWith("1") ? 0 : 3;
      bodies.forEach((b, i) => {
        expect(b).toBe(server.requests()[offset + i]!.body.toString("utf8"));
        expect(`blake3:${blake3Hex(Buffer.from(b!, "utf8"))}`).toBe(sent[offset + i]!.bodyHash);
      });
    }
    // Metadata: exact capture, a keyed bodyHash equal in both stores; no content in events/.
    const meta = of(agentEntries(dir, "events"), "agent.turn");
    expect(meta.map((m) => m.payload.capture)).toEqual(Array(6).fill("exact"));
    expect(meta.map((m) => m.payload.bodyHash)).toEqual(turns.map((t) => t.payload.bodyHash));
    expect(meta.filter(hasContent)).toEqual([]);
    expect(meta.some((m) => "bodyBlake3" in m.payload)).toBe(false);
  });

  test("system_and_tools_blobs_deduplicate: two sessions of one role share each blob; purging one keeps it", async () => {
    const s = stores(TRAINING);
    const { j } = agentJournal(s);
    const a = session({ id: "01JS000000000000000000000A", projectId: P1 });
    const b = session({ id: "01JS000000000000000000000B", projectId: P2 });
    for (const [ss, runId] of [[a, "01JR000000000000000000000A"], [b, "01JR000000000000000000000B"]] as const) {
      j.capture({ session: ss, runId, request: body(req([USER])), response: [] });
      j.turn({ session: ss, runId, usage: usage() });
    }
    await flush(s);
    const turns = of(agentEntries(dir, "agent-traces"), "agent.turn");
    expect(turns).toHaveLength(2);
    expect(turns[0]!.payload.system.blob.hash).toBe(turns[1]!.payload.system.blob.hash);
    expect(turns[0]!.payload.tools.blob.hash).toBe(turns[1]!.payload.tools.blob.hash);
    const before = blobFiles(dir, "agent-traces");
    expect(before.length).toBe(2); // one system and one tools blob (the delta and output are inline)
    JSON.parse(await s.traces.purge(JSON.stringify({ project: P1 })));
    const left = agentEntries(dir, "agent-traces");
    expect(left.map((e) => e.ctx?.project)).toEqual([P2]);
    expect(blobFiles(dir, "agent-traces")).toEqual(before);
  });

  test("large_tool_output_is_a_blob (over ~4 KiB); over_4_mib_truncated", async () => {
    const s = stores(TRAINING);
    const { j } = agentJournal(s, { ...EMPTY_STORE, get: () => session() });
    const ss = session();
    const calls = ["01JC0000000000000000000001", "01JC0000000000000000000002", "01JC0000000000000000000003"];
    const outputs = [{ text: "small" }, { text: "x".repeat(6 * 1024) }, { text: "y".repeat(5 * 1024 * 1024) }];
    calls.forEach((callId, i) => {
      j.toolCall({ session: ss, runId: "01JR0000000000000000000001", callId, providerId: `p${i}`, tool: "fin", input: { argv: ["view"] } });
      j.toolResult({ sessionId: ss.id, runId: "01JR0000000000000000000001", callId, ok: true, output: outputs[i], synthetic: false });
    });
    await flush(s);
    const results = of(agentEntries(dir, "agent-traces"), "agent.toolResult");
    expect(results.map((r) => Object.keys(r.payload.output)[0])).toEqual(["inline", "blob", "blob"]);
    expect(results.map((r) => r.payload.truncated ?? false)).toEqual([false, false, true]);
    expect(content(dir, results[1]!.payload.output)).toEqual(outputs[1]);
    const big = content(dir, results[2]!.payload.output) as { truncated: boolean; head: string };
    expect(big.truncated).toBe(true);
    expect(results[2]!.payload.output.blob.bytes).toBeLessThanOrEqual(BLOB_MAX);
    expect(big.head.startsWith('{"text":"yyyy')).toBe(true);
    expect(of(agentEntries(dir, "events"), "agent.toolResult").filter(hasContent)).toEqual([]);
  });

  test("trace_store_is_separate: a full trace store evicts whole old sessions; diagnostics keeps every entry", async () => {
    const s = stores(TRAINING, { maxTotalBytes: 96 * 1024, maxFileBytes: 16 * 1024 });
    const { j } = agentJournal(s);
    const sessions = Array.from({ length: 12 }, (_, i) => session({ id: `01JS00000000000000000000${String(i).padStart(2, "0")}` }));
    let n = 0;
    for (const ss of sessions) {
      for (let k = 0; k < 6; k++) {
        const callId = `01JC${String(++n).padStart(22, "0")}`;
        j.toolCall({ session: ss, runId: `${ss.id.slice(0, 24)}RR`, callId, providerId: `p${n}`, tool: "fin", input: { argv: ["view"] } });
        j.toolResult({ sessionId: ss.id, runId: `${ss.id.slice(0, 24)}RR`, callId, ok: true, output: { text: "z".repeat(3000) }, synthetic: false });
      }
      j.session(ss.id, ss, "ended", null);
      await flush(s);
    }
    const kept = agentEntries(dir, "agent-traces");
    const bySession = new Map<string, number>();
    for (const e of kept) bySession.set(e.ctx!.actor.session, (bySession.get(e.ctx!.actor.session) ?? 0) + 1);
    // Old sessions are gone whole; every one left has all 13 of its entries (6 calls, 6
    // results, its `ended`); the newest stays.
    expect(bySession.has(sessions[0]!.id)).toBe(false);
    expect(bySession.has(sessions[11]!.id)).toBe(true);
    for (const count of bySession.values()) expect(count).toBe(13);
    const d = agentEntries(dir, "events");
    expect(of(d, "agent.toolResult")).toHaveLength(72);
    expect(of(d, "agent.session")).toHaveLength(12);
  });
});

describe("delivery", () => {
  test("a call's result is recorded once (a synthetic one after the real one, or the outbox again)", async () => {
    const s = stores(DIAGNOSTICS_ONLY);
    const ss = session();
    const { j } = agentJournal(s, { ...EMPTY_STORE, get: () => ss });
    const callId = "01JC0000000000000000000001";
    j.toolCall({ session: ss, runId: "R", callId, providerId: "p", tool: "workspace_read", input: {} });
    j.toolResult({ sessionId: ss.id, runId: "R", callId, ok: true, output: {}, synthetic: false });
    j.toolResult({ sessionId: ss.id, runId: "R", callId, ok: false, output: { code: "SESSION_INTERRUPTED" }, synthetic: true });
    const other = "01JC0000000000000000000002";
    j.toolResult({ sessionId: ss.id, runId: "R", callId: other, ok: false, output: { code: "SESSION_INTERRUPTED", synthetic: true }, synthetic: true });
    j.toolResult({ sessionId: ss.id, runId: "R", callId: other, ok: false, output: { code: "SESSION_INTERRUPTED", synthetic: true }, synthetic: true });
    await flush(s);
    const d = agentEntries(dir, "events");
    expect(of(d, "agent.toolCall")[0]!.payload.tool).toBe("workspace");
    expect(of(d, "agent.toolResult").map((e) => [e.payload.callId, e.payload.ok, e.payload.code])).toEqual([
      [callId, true, undefined],
      [other, false, "SESSION_INTERRUPTED"],
    ]);
  });

  test("approvals: an open one is not recorded; answered and expired ones are, once; subject is content", async () => {
    const s = stores(TRAINING);
    const ss = session();
    const now = 1_794_571_260_000;
    const base = { sessionId: ss.id, kind: "budget" as const, subject: { kind: "tokens", used: 10, limit: 10 }, argsHash: "budget:tokens", requestedAt: now - 60_000, expiresAt: now + 60_000 };
    const expired = { ...base, id: "01JA0000000000000000000002", status: "expired" as const };
    const { j } = agentJournal(s, { ...EMPTY_STORE, approvals: () => [expired] });
    j.approval({ session: ss, approval: { ...base, id: "01JA0000000000000000000001", status: "open" } });
    j.approval({ session: ss, approval: { ...base, id: "01JA0000000000000000000001", status: "answered", decision: "allow", by: "local:local", answeredAt: now - 30_000 } });
    j.session(ss.id, ss, "resumed", null);
    j.session(ss.id, ss, "interrupted", "BUDGET_EXCEEDED");
    await flush(s);
    const d = of(agentEntries(dir, "events"), "agent.approval");
    expect(d.map((e) => [e.payload.approvalId, e.payload.decision, e.payload.by, e.payload.latencyMs])).toEqual([
      ["01JA0000000000000000000001", "allow", "user", 30_000],
      ["01JA0000000000000000000002", "expired", "policy", expect.any(Number)],
    ]);
    expect(d.filter(hasContent)).toEqual([]);
    expect(content(dir, of(agentEntries(dir, "agent-traces"), "agent.approval")[0]!.payload.subject)).toEqual(base.subject);
  });

  test("a phase of a deleted session uses the provenance seen before; one never seen is logged, not thrown", async () => {
    const s = stores(DIAGNOSTICS_ONLY);
    const { j, logs } = agentJournal(s);
    const ss = session();
    j.session(ss.id, ss, "started", null);
    j.session(ss.id, undefined, "ended", "serverRestart");
    j.session("01JS0000000000000000000ZZZ", undefined, "ended", null);
    await flush(s);
    const phases = of(agentEntries(dir, "events"), "agent.session").map((e) => [e.payload.phase, e.payload.reason, e.payload.totals]);
    expect(phases).toEqual([
      ["started", undefined, undefined],
      ["ended", "SERVER_RESTART", undefined],
    ]);
    expect(logs.some((l) => l.includes("no provenance"))).toBe(true);
  });

  test("helpers: codes, fin commands", () => {
    expect(codeOf("serverRestart")).toBe("SERVER_RESTART");
    expect(codeOf("BUDGET_EXCEEDED")).toBe("BUDGET_EXCEEDED");
    expect(codeOf("not a code!")).toBeUndefined();
    expect(commandOf({ argv: ["item", "trim", "c1"] })).toBe("item.trim");
    expect(commandOf({ argv: ["view", "--budget", "2000"] })).toBe("view");
    expect(commandOf({ argv: ["--json"] })).toBeUndefined();
    expect(commandOf("x")).toBeUndefined();
  });
});
