// Claude Code's lines as `ProviderEvent`s (plan 7D Task 7, Decision 14): `map.ts` over the
// hand-written fixtures and single lines, every event checked against the contract.
import { readFileSync } from "node:fs";
import { FinError, ProviderEvent } from "@finisher/contracts";
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import { cumulativeUsage, LineMapper, resultError, SIGNIN_FIX } from "../src/agent/claude/map.ts";
import { type CliLine, decodeLine, type ResultMsg } from "../src/agent/claude/protocol.ts";

const decodeEvent = Schema.decodeUnknownSync(ProviderEvent);

function line(o: Record<string, unknown>): CliLine {
  const l = decodeLine(JSON.stringify(o));
  if (l instanceof FinError) throw new Error(`bad fixture: ${l.message}`);
  return l;
}
const fixture = (name: string): CliLine[] =>
  readFileSync(new URL(`./fixtures/claude/${name}.jsonl`, import.meta.url), "utf8")
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((raw) => {
      const l = decodeLine(raw);
      if (l instanceof FinError) throw new Error(`bad fixture: ${l.message}`);
      return l;
    });

/** Every event `m` makes of `lines`, each one valid under the contract. */
function all(m: LineMapper, lines: readonly CliLine[], interrupting = false): ProviderEvent[] {
  const out = lines.flatMap((l) => m.map(l, interrupting));
  for (const e of out) expect(() => decodeEvent(e)).not.toThrow();
  return out;
}

const result = (over: Record<string, unknown> = {}) =>
  line({ type: "result", subtype: "success", is_error: false, result: "done", stop_reason: "end_turn", total_cost_usd: 0, usage: { input_tokens: 0, output_tokens: 0 }, queued_turn_count: 0, ...over }) as ResultMsg;

describe("the session fixture", () => {
  test("prompt_to_nodes: reasoning with its signature, a message, a tool call and its results, notices, usage and the end", () => {
    const events = all(new LineMapper(), fixture("session"));
    const types = events.map((e) => e.type);
    // The init is the gate's; message_start/stop and signature/json deltas add nothing.
    expect(types.slice(0, 7)).toEqual(["reasoningDelta", "messageDelta", "reasoning", "message", "toolStarted", "toolCompleted", "toolCompleted"]);
    expect(events[2]).toEqual({ type: "reasoning", text: "The timeline needs a trim.", encrypted: { signature: "c2lnbmF0dXJl" } });
    expect(events[3]).toEqual({ type: "message", role: "assistant", blocks: [{ type: "text", text: "I will read the cut list." }] });
    expect(events[4]).toEqual({ type: "toolStarted", providerId: "toolu_01", tool: "Read", input: { file_path: "/srv/finisher/workspace/s1/cuts.txt" } });
    expect(events[5]).toEqual({ type: "toolCompleted", providerId: "toolu_01", ok: true, output: "00:00:01.000 00:00:04.500\n" });
    expect(events[6]).toMatchObject({ type: "toolCompleted", providerId: "toolu_02", ok: false });
    expect(events).toContainEqual({ type: "notice", name: "retry", details: { attempt: 1, code: 529, delayMs: 1200 } });
    expect(events).toContainEqual({ type: "notice", name: "compacted", details: { trigger: "auto", preTokens: 158000 } });
    const done = events.findIndex((e) => e.type === "turnCompleted");
    expect(events[done - 1]).toEqual({ type: "usage", input: 24, output: 160, cacheRead: 6800, cacheWrite: 0, reasoning: 0, costMicros: 21300 });
    expect(events[done]).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
  });

  test("deltas_streamed_not_stored: only text and thinking deltas stream", () => {
    const m = new LineMapper();
    const ev = (event: Record<string, unknown>) => m.map(line({ type: "stream_event", session_id: "s", parent_tool_use_id: null, event }));
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hi" } })).toEqual([{ type: "messageDelta", text: "Hi" }]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "hm" } })).toEqual([{ type: "reasoningDelta", text: "hm" }]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{" } })).toEqual([]);
    expect(ev({ type: "message_start", message: { id: "m", usage: { input_tokens: 3 } } })).toEqual([]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "" } })).toEqual([]);
  });
});

describe("results", () => {
  test("usage is what each result added (cumulative per process)", () => {
    const u = cumulativeUsage();
    expect(u.usage(result({ total_cost_usd: 0.001, usage: { input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 5 } }))).toEqual({ type: "usage", input: 10, output: 2, cacheRead: 5, cacheWrite: 0, reasoning: 0, costMicros: 1000 });
    expect(u.usage(result({ total_cost_usd: 0.003, usage: { input_tokens: 25, output_tokens: 6, cache_read_input_tokens: 5 } }))).toEqual({ type: "usage", input: 15, output: 4, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 2000 });
    // Nothing added: no event; a total that goes back charges nothing.
    expect(u.usage(result({ total_cost_usd: 0.003, usage: { input_tokens: 25, output_tokens: 6, cache_read_input_tokens: 5 } }))).toBeNull();
    expect(u.usage(result({ total_cost_usd: 0.001, usage: { input_tokens: 1, output_tokens: 1 } }))).toBeNull();
  });

  test("steer_stays_in_run: a result with a queued steer does not end the run", () => {
    const m = new LineMapper();
    const first = m.map(result({ queued_turn_count: 1, total_cost_usd: 0.001, usage: { input_tokens: 4, output_tokens: 1 } }));
    expect(first.map((e) => e.type)).toEqual(["usage"]);
    const second = m.map(result({ queued_turn_count: 0, total_cost_usd: 0.002, usage: { input_tokens: 8, output_tokens: 2 } }));
    expect(second.map((e) => e.type)).toEqual(["usage", "turnCompleted"]);
  });

  test("not_logged_in_is_provider_auth_with_fix: keyed on is_error, not subtype", () => {
    const events = all(new LineMapper(), fixture("session").filter((l) => l.kind === "result" && l.is_error));
    const failed = events.find((e) => e.type === "turnFailed");
    expect(failed).toMatchObject({ type: "turnFailed", error: { code: "PROVIDER_AUTH", fix: SIGNIN_FIX, details: { reason: "notLoggedIn" } } });
    // The synthetic assistant that precedes it maps to nothing.
    expect(new LineMapper().map(line({ type: "assistant", session_id: "s", parent_tool_use_id: null, message: { model: "<synthetic>", content: [{ type: "text", text: "Not logged in · Please run /login" }] }, error: "authentication_failed", is_api_error_message: true }))).toEqual([]);
  });

  test("errors by status and subtype", () => {
    const code = (over: Record<string, unknown>, interrupting = false) => resultError(result({ is_error: true, ...over }), interrupting).code;
    expect(code({ api_error_status: 401 })).toBe("PROVIDER_AUTH");
    expect(code({ api_error_status: 403 })).toBe("PROVIDER_AUTH");
    expect(code({ api_error_status: 429 })).toBe("PROVIDER_RATE_LIMITED");
    expect(code({ api_error_status: 529 })).toBe("PROVIDER_OVERLOADED");
    expect(code({ subtype: "error_max_budget_usd" })).toBe("BUDGET_EXCEEDED");
    expect(code({ subtype: "error_during_execution", result: "" }, true)).toBe("SESSION_INTERRUPTED");
    expect(code({ subtype: "error_during_execution", result: "boom" })).toBe("PROVIDER_UNAVAILABLE");
  });
});

describe("notices", () => {
  test("api_retry_is_notice", () => {
    expect(new LineMapper().map(line({ type: "system", subtype: "api_retry", attempt: 2, retry_delay_ms: 500, error_status: 429 }))).toEqual([{ type: "notice", name: "retry", details: { attempt: 2, code: 429, delayMs: 500 } }]);
  });

  test("unknown_line_is_notice: once per name; status lines are silent", () => {
    const m = new LineMapper();
    const unknown = all(m, fixture("unknown"));
    expect(unknown.length).toBeGreaterThan(0);
    for (const e of unknown) expect(e.type).toBe("unknown");
    expect(m.map(line({ type: "fake_future_type" }))).toEqual([{ type: "unknown", name: "fake_future_type" }]);
    expect(m.map(line({ type: "fake_future_type" }))).toEqual([]);
    expect(m.map(line({ type: "system", subtype: "status", status: "requesting" }))).toEqual([]);
    expect(m.map(line({ type: "system", subtype: "ui_invalidate" }))).toEqual([]);
    expect(m.map(line({ type: "system", subtype: "permission_denied" }))).toEqual([]);
    expect(m.map(line({ type: "system", subtype: "brand_new" }))).toEqual([{ type: "unknown", name: "system/brand_new" }]);
  });

  test("unexpected_subagent_line_is_notice: a subagent's message is never content", () => {
    const m = new LineMapper();
    const sub = m.map(line({ type: "assistant", session_id: "s", parent_tool_use_id: "toolu_9", message: { content: [{ type: "tool_use", id: "toolu_10", name: "Bash", input: {} }] } }));
    expect(sub).toEqual([{ type: "notice", name: "unexpectedSubagent", details: { parentToolUseId: "toolu_9" } }]);
    expect(m.map(line({ type: "user", session_id: "s", parent_tool_use_id: "toolu_9", message: { content: [{ type: "tool_result", tool_use_id: "toolu_10", content: "x" }] } }))[0]).toMatchObject({ name: "unexpectedSubagent" });
    expect(m.map(line({ type: "stream_event", session_id: "s", parent_tool_use_id: "toolu_9", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "x" } } }))).toEqual([]);
  });
});
