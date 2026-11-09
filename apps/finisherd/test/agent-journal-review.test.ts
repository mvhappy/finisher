// The Task 10 review's fixes (plan 7A Task 10): provenance in the trace store (B1), deltas that
// chain only through recorded turns and replay checked by hash (H1), tool-result codes from the
// FinError table only (M3), no prompt hashes on stream turns (L1), and the review's missing
// cases (L5): training revoked mid-turn, diagnostics off with training on, a stream harness.
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Journal } from "@finisher/fin-node";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { chatBody, type ModelRequest } from "../src/agent/builtin/request.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentJournal, type JournalSink, resultCodeOf } from "../src/agent/journal.ts";
import { hashOf, type MessagesDelta, type ReplayTurn, replayBodies, splitRequest } from "../src/agent/journal-content.ts";
import { agentEntries, CONTENT_FIELDS, content, EMPTY_STORE, type Entry, flush, openStores, P1, type Stores, session, usage } from "./support/agent-journal.ts";

let dir: string;
const cleanups: (() => Promise<void> | void)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-journal-review-"));
});

afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
  rmSync(dir, { recursive: true, force: true });
});

function stores(privacy: object | null): Stores {
  const s = openStores(dir, privacy);
  cleanups.push(() => s.close());
  return s;
}

const TRAINING = { diagnostics: "on", training: "on" };
const setTraining = (s: Stores, on: boolean) => {
  const p = JSON.stringify({ diagnostics: "on", training: on ? "on" : "off" });
  s.diag.applySettings(p);
  s.traces.applySettings(p);
};
const hasContent = (e: Entry) => CONTENT_FIELDS.some((f) => f in e.payload);
const of = (es: Entry[], kind: string) => es.filter((e) => e.kind === kind);

const req = (messages: ModelRequest["messages"]): ModelRequest => ({
  system: "You are the editor. Use fin.",
  tools: [{ name: "fin", description: "Run fin.", parameters: { type: "object", properties: { argv: { type: "array" } } } }],
  messages,
  model: "anthropic/claude-sonnet-5.5",
  maxTokens: 1000,
});
const body = (r: ModelRequest) => JSON.stringify(chatBody(r, { preset: "openrouter", caching: "none" }));
const USER = { role: "user", content: [{ type: "text", text: "trim the intro" }] } as const;
const CALLED = { role: "assistant", toolCalls: [{ id: "call_1", name: "fin", arguments: '{"argv":["item","trim","c1"]}' }] } as const;
const RESULT = { role: "tool", toolCallId: "call_1", content: '{"txId":"tx"}' } as const;
const SAID = { role: "assistant", text: "Trimmed." } as const;
const MORE = { role: "user", content: [{ type: "text", text: "and the outro" }] } as const;
/** Four requests of one run, each the previous plus new messages. */
const CONV: ModelRequest["messages"][] = [[USER], [USER, CALLED, RESULT], [USER, CALLED, RESULT, SAID, MORE], [USER, CALLED, RESULT, SAID, MORE, SAID]];
const RUN = "01JR0000000000000000000001";

/** The trace store's turns of `RUN` as the replay reads them. */
function replayInput(): ReplayTurn[] {
  return of(agentEntries(dir, "agent-traces"), "agent.turn")
    .filter((t) => t.payload.messagesDelta !== undefined)
    .map((t) => ({
      turnId: t.payload.turnId,
      delta: content(dir, t.payload.messagesDelta) as MessagesDelta,
      system: JSON.stringify(content(dir, t.payload.system)),
      tools: JSON.stringify(content(dir, t.payload.tools)),
      bodyBlake3: t.payload.bodyBlake3,
    }));
}

/** A trace sink over `t` whose blobs are refused while `refuse()` holds. */
function refusing(t: Journal, refuse: () => boolean): JournalSink {
  return {
    get enabled() {
      return t.enabled;
    },
    get training() {
      return t.training;
    },
    record: (...a) => t.record(...a),
    recordsProject: (p) => t.recordsProject(p),
    attachBlob: (b, m) => (refuse() ? undefined : t.attachBlob(b, m)),
  };
}

/**
 * Records `CONV` as turns of `RUN`, running `before(i)` ahead of turn i's capture and `mid(i)`
 * between its capture and usage. Each turn is flushed before the next: a consent or policy
 * change also drops the lines still queued (off means off), which is not what these test.
 */
async function turns(s: Stores, j: AgentJournal, hooks: { before?: (i: number) => void; mid?: (i: number) => void } = {}) {
  const ss = session();
  const sent = CONV.map((m) => body(req(m)));
  for (const [i, request] of sent.entries()) {
    hooks.before?.(i);
    j.capture({ session: ss, runId: RUN, request, response: { chunks: [{ choices: [{ delta: { content: "ok" } }] }], stopReason: "endTurn", latencyMs: 5 } });
    hooks.mid?.(i);
    j.turn({ session: ss, runId: RUN, usage: usage() });
    await flush(s);
  }
  return sent;
}

const newJournal = (s: Stores, traces: JournalSink = s.traces, store = EMPTY_STORE) => {
  const clock = manualClock();
  return new AgentJournal({ diagnostics: s.diag, traces, store, ids: systemIds(clock), clock });
};

describe("B1: provenance in the trace store", () => {
  test("a stream harness's session, metadata-only turns, calls and results are all in the trace store with provenance", async () => {
    const s = stores(TRAINING);
    const ss = session({ harness: "claudeCode", provider: "anthropic", model: "claude-opus-5-5", weights: "closed" });
    const j = newJournal(s, s.traces, { ...EMPTY_STORE, get: () => ss });
    j.session(ss.id, ss, "started", null);
    // A CLI harness reports usage from its stream: no capture.
    j.turn({ session: ss, runId: RUN, usage: usage({ costBasis: "subscription" }) });
    j.toolCall({ session: ss, runId: RUN, callId: "01JC0000000000000000000001", providerId: "p1", tool: "Bash", input: { command: "fin view" } });
    j.toolResult({ sessionId: ss.id, runId: RUN, callId: "01JC0000000000000000000001", ok: true, output: { stdout: "ok" }, synthetic: false });
    j.session(ss.id, ss, "ended", null);
    await flush(s);
    const t = agentEntries(dir, "agent-traces");
    expect(t.map((e) => e.kind)).toEqual(["agent.session", "agent.turn", "agent.toolCall", "agent.toolResult", "agent.session"]);
    const turn = of(t, "agent.turn")[0]!;
    expect(turn.payload).toMatchObject({ capture: "stream", harness: "claudeCode", provider: "anthropic", model: "claude-opus-5-5", weights: "closed" });
    expect(hasContent(turn)).toBe(false);
    // L1: no hash of an empty prompt on a stream turn, in either store.
    for (const sub of ["events", "agent-traces"] as const) {
      const st = of(agentEntries(dir, sub), "agent.turn")[0]!;
      expect(st.payload.systemHash).toBeUndefined();
      expect(st.payload.toolsHash).toBeUndefined();
      expect(st.payload.bodyHash).toBeUndefined();
    }
    for (const e of [...of(t, "agent.toolCall"), ...of(t, "agent.toolResult")]) {
      expect(e.payload.provenance).toEqual({ harness: "claudeCode", provider: "anthropic", model: "claude-opus-5-5", weights: "closed" });
      expect(hasContent(e)).toBe(true);
    }
    // The diagnostics journal's copies carry no provenance stamp (sessions and turns have it flat).
    expect(agentEntries(dir, "events").some((e) => "provenance" in e.payload)).toBe(false);
  });

  test("a synthetic result and an approval that arrive before any turn carry the session's provenance", async () => {
    const s = stores(TRAINING);
    const ss = session({ harness: "codex", provider: "openai", model: "gpt-5.5", weights: "closed" });
    const now = 1_794_571_260_000;
    const j = newJournal(s, s.traces, { ...EMPTY_STORE, get: () => ss });
    j.approval({
      session: ss,
      approval: { id: "01JA0000000000000000000001", sessionId: ss.id, kind: "rule", subject: { rule: "deny-main" }, argsHash: "h", requestedAt: now - 10, expiresAt: now + 10, status: "answered", decision: "deny", by: "local:local", answeredAt: now },
    });
    j.toolResult({ sessionId: ss.id, runId: RUN, callId: "01JC0000000000000000000009", ok: false, output: { code: "SESSION_INTERRUPTED", synthetic: true }, synthetic: true });
    await flush(s);
    const t = agentEntries(dir, "agent-traces");
    expect(t.map((e) => e.kind)).toEqual(["agent.approval", "agent.toolResult"]);
    for (const e of t) {
      expect(e.payload.provenance).toMatchObject({ harness: "codex", provider: "openai", weights: "closed" });
      expect(typeof e.payload.provenance.model).toBe("string");
      expect(hasContent(e)).toBe(true);
    }
    expect(of(t, "agent.toolResult")[0]!.payload.code).toBe("SESSION_INTERRUPTED");
  });
});

describe("H1: deltas chain only through recorded turns", () => {
  test("in order: every delta past the first names its base and the replay rebuilds each body, checked by hash", async () => {
    const s = stores(TRAINING);
    const sent = await turns(s, newJournal(s));
    const input = replayInput();
    expect(input.map((t) => ("from" in t.delta ? t.delta.from : "raw"))).toEqual([0, 1, 3, 5]);
    input.forEach((t, i) => {
      expect(t.bodyBlake3).toBe(hashOf(sent[i]!));
      if (i > 0) expect("from" in t.delta && t.delta.baseTurnId).toBe(input[i - 1]!.turnId);
    });
    expect(replayBodies(input)).toEqual(sent);
    // A tampered hash drops that turn and the one based on it; the replay never returns other bytes.
    const tampered = input.map((t, i) => (i === 1 ? { ...t, bodyBlake3: hashOf("other") } : t));
    expect(replayBodies(tampered)).toEqual([sent[0], undefined, undefined, undefined]);
    // A missing turn (the store lost turn 2): turn 3's base is not the turn replayed before it.
    expect(replayBodies([input[0]!, input[2]!])).toEqual([sent[0], undefined]);
  });

  test("training off for one turn: the next delta starts over and every recorded turn replays", async () => {
    const s = stores(TRAINING);
    const sent = await turns(s, newJournal(s), { before: (i) => setTraining(s, i !== 1) });
    const input = replayInput();
    expect(input).toHaveLength(3);
    expect(input.map((t) => ("from" in t.delta ? [t.delta.from, t.delta.baseTurnId] : "raw"))).toEqual([
      [0, undefined],
      [0, undefined],
      [5, input[1]!.turnId],
    ]);
    expect(replayBodies(input)).toEqual([sent[0], sent[2], sent[3]]);
  });

  test("L5: training revoked mid-turn (between capture and usage): no trace of that turn, no blob, the next starts over", async () => {
    const s = stores(TRAINING);
    const sent = await turns(s, newJournal(s), {
      before: (i) => i === 2 && setTraining(s, true),
      mid: (i) => i === 1 && setTraining(s, false),
    });
    const t = of(agentEntries(dir, "agent-traces"), "agent.turn");
    expect(t.map((e) => e.payload.seq)).toEqual([1, 3, 4]);
    const input = replayInput();
    expect("from" in input[1]!.delta && input[1]!.delta.from).toBe(0);
    expect(replayBodies(input)).toEqual([sent[0], sent[2], sent[3]]);
    // Every blob on disk is one a recorded turn references (system and tools, shared).
    const refs = new Set(t.flatMap((e) => ["system", "tools", "messagesDelta", "output"].map((f) => e.payload[f]?.blob?.hash).filter(Boolean)));
    expect(readdirSync(join(dir, "agent-traces", "blobs")).length).toBe(refs.size);
  });

  test("a refused blob: the turn is in the trace store without content, and the next delta starts over", async () => {
    const s = stores(TRAINING);
    let refuse = false;
    const sent = await turns(s, newJournal(s, refusing(s.traces, () => refuse)), { before: (i) => void (refuse = i === 1) });
    const t = of(agentEntries(dir, "agent-traces"), "agent.turn");
    expect(t.map((e) => [e.payload.seq, hasContent(e)])).toEqual([
      [1, true],
      [2, false],
      [3, true],
      [4, true],
    ]);
    expect(t[1]!.payload).toMatchObject({ capture: "exact", harness: "builtin", weights: "closed" });
    const input = replayInput();
    expect("from" in input[1]!.delta && input[1]!.delta.from).toBe(0);
    expect(replayBodies(input)).toEqual([sent[0], sent[2], sent[3]]);
  });

  test("a project not recorded for one turn (its policy unknown, then known again): the next delta starts over", async () => {
    // (`never` would also purge what the project recorded: unknown records nothing and keeps it.)
    const s = stores(TRAINING);
    const sent = await turns(s, newJournal(s), { before: (i) => s.diag.setProjectPolicy(P1, i === 1 ? null : "default") });
    const t = of(agentEntries(dir, "agent-traces"), "agent.turn");
    expect(t.map((e) => e.payload.seq)).toEqual([1, 3, 4]);
    const input = replayInput();
    expect("from" in input[1]!.delta && input[1]!.delta.from).toBe(0);
    expect(replayBodies(input)).toEqual([sent[0], sent[2], sent[3]]);
  });
});

describe("consent and codes", () => {
  test("L5: diagnostics off with training on: the trace store holds everything with content; events/ no content", async () => {
    const s = stores({ diagnostics: "off", training: "on" });
    const ss = session();
    const j = newJournal(s, s.traces, { ...EMPTY_STORE, get: () => ss });
    j.session(ss.id, ss, "started", null);
    await turns(s, j);
    j.toolCall({ session: ss, runId: RUN, callId: "01JC0000000000000000000001", providerId: "p1", tool: "fin", input: { argv: ["view"] } });
    j.toolResult({ sessionId: ss.id, runId: RUN, callId: "01JC0000000000000000000001", ok: true, output: { text: "ok" }, synthetic: false });
    await flush(s);
    const t = agentEntries(dir, "agent-traces");
    expect(new Set(t.map((e) => e.kind))).toEqual(new Set(["agent.session", "agent.turn", "agent.toolCall", "agent.toolResult"]));
    expect(of(t, "agent.turn").every(hasContent)).toBe(true);
    expect(t.every((e) => e.consent.purposes.join() === "training")).toBe(true);
    const d = agentEntries(dir, "events");
    expect(d.filter(hasContent)).toEqual([]);
    expect(d.every((e) => !e.consent.purposes.includes("diagnostics"))).toBe(true);
  });

  test("M3: a tool result's code is a FinError code or OTHER; free text never enters a record", async () => {
    expect(resultCodeOf("NOT_FOUND")).toBe("NOT_FOUND");
    expect(resultCodeOf("notFound")).toBe("NOT_FOUND");
    expect(resultCodeOf("ENOENT: no such file /home/me/secret.mov")).toBe("OTHER");
    expect(resultCodeOf("MY_CUSTOMER_ACME")).toBe("OTHER");
    expect(resultCodeOf(42)).toBe("OTHER");
    expect(resultCodeOf(undefined)).toBeUndefined();
    const s = stores(TRAINING);
    const ss = session();
    const j = newJournal(s, s.traces, { ...EMPTY_STORE, get: () => ss });
    j.toolResult({ sessionId: ss.id, runId: RUN, callId: "01JC0000000000000000000002", ok: false, output: { code: "ACME_PROJECT_X_FAILED" }, synthetic: false });
    j.toolResult({ sessionId: ss.id, runId: RUN, callId: "01JC0000000000000000000003", ok: false, output: { code: "BRANCH_PROTECTED" }, synthetic: false });
    await flush(s);
    expect(of(agentEntries(dir, "events"), "agent.toolResult").map((e) => e.payload.code)).toEqual(["OTHER", "BRANCH_PROTECTED"]);
  });
});

// M1: the Rust replay (`fin-dataset-inputs`) reads the same deltas. The fixture is this file's
// split of `CONV`; the Rust test replays it and must rebuild the same bytes.
const FIXTURE = join(import.meta.dirname, "../../../crates/fin-dataset-inputs/tests/fixtures/replay.json");

test("the Rust replay's fixture is what splitRequest makes now (FIN_UPDATE_FIXTURES=1 rewrites it)", () => {
  let previous: unknown[] = [];
  let base: string | undefined;
  const turnsOut = CONV.map((m, i) => {
    const request = body(req(m));
    const split = splitRequest(request, previous, base);
    const turnId = `01JT00000000000000000000${String(i).padStart(2, "0")}`;
    previous = split.messages;
    base = turnId;
    return { turnId, delta: split.delta, system: split.system, tools: split.tools, bodyBlake3: hashOf(request), body: request };
  });
  const text = `${JSON.stringify({ turns: turnsOut }, null, 2)}\n`;
  if (process.env.FIN_UPDATE_FIXTURES === "1") writeFileSync(FIXTURE, text);
  expect(readFileSync(FIXTURE, "utf8")).toBe(text);
  expect(replayBodies(turnsOut)).toEqual(turnsOut.map((t) => t.body));
});

describe("saved prompts", () => {
  test("the journaled agent.session carries trigger savedPrompt, a child subagent, else user", async () => {
    const s = stores(TRAINING);
    const ss = session({ trigger: "savedPrompt" });
    const j = newJournal(s, s.traces, { ...EMPTY_STORE, get: () => ss });
    j.session(ss.id, ss, "started", null);
    await flush(s);
    for (const sub of ["events", "agent-traces"] as const) {
      expect(of(agentEntries(dir, sub), "agent.session")[0]?.payload.trigger, sub).toBe("savedPrompt");
    }
  });
});
