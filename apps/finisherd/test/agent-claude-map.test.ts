// Claude Code's lines as `ProviderEvent`s (plan 7D Task 7, Decision 14): `map.ts` over the
// hand-written fixtures and single lines, every event checked against the contract.
import { readFileSync } from "node:fs";
import { FinError, ProviderEvent } from "@finisher/contracts";
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import { LineMapper, resultError, SIGNIN_FIX } from "../src/agent/claude/map.ts";
import { ClaudeUsage } from "../src/agent/claude/usage.ts";
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
    const mapped = all(new LineMapper(), fixture("session"));
    // Live usage (Task 9: `message_start`/`message_delta`) is checked below; the rest in order.
    const events = mapped.filter((e) => e.type !== "usage" || mapped.indexOf(e) > mapped.findIndex((x) => x.type === "toolCompleted"));
    expect(mapped.filter((e) => e.type === "usage").slice(0, 2)).toEqual([
      { type: "usage", input: 12, output: 1, cacheRead: 3400, cacheWrite: 0, reasoning: 0, costMicros: 0, costBasis: "reported" },
      { type: "usage", input: 0, output: 86, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 0, costBasis: "reported" },
    ]);
    const types = events.map((e) => e.type);
    // The init is the gate's; message_start/stop and signature/json deltas add nothing.
    expect(types.slice(0, 7)).toEqual(["reasoningDelta", "messageDelta", "reasoning", "message", "toolStarted", "toolCompleted", "toolCompleted"]);
    expect(events[2]).toEqual({ type: "reasoning", text: "The timeline needs a trim.", encrypted: { signature: "c2lnbmF0dXJl" } });
    expect(events[3]).toEqual({ type: "message", role: "assistant", blocks: [{ type: "text", text: "I will read the cut list." }] });
    expect(events[4]).toEqual({ type: "toolStarted", providerId: "toolu_01", tool: "Read", input: { file_path: "/srv/finisher/workspace/s1/cuts.txt" } });
    expect(events[5]).toEqual({ type: "toolCompleted", providerId: "toolu_01", ok: true, output: "00:00:01.000 00:00:04.500\n" });
    expect(events[6]).toMatchObject({ type: "toolCompleted", providerId: "toolu_02", ok: false });
    expect(events).toContainEqual({ type: "notice", name: "retry", details: { attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 1200 } });
    expect(events).toContainEqual({ type: "notice", name: "compacted", details: { trigger: "auto", preTokens: 158000 } });
    const done = events.findIndex((e) => e.type === "turnCompleted");
    // The result adds what the stream did not show, and the turn's money; then its per-model split.
    expect(events[done - 2]).toEqual({ type: "usage", input: 12, output: 73, cacheRead: 3400, cacheWrite: 0, reasoning: 0, costMicros: 21300, costBasis: "reported" });
    expect(events[done - 1]).toEqual({ type: "notice", name: "modelUsage", details: { "test-claude-big": { input: 24, output: 160, cacheRead: 6800, cacheWrite: 0, costMicros: 21300 } } });
    expect(events[done]).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
  });

  test("deltas_streamed_not_stored: only text and thinking deltas stream", () => {
    const m = new LineMapper();
    const ev = (event: Record<string, unknown>) => m.map(line({ type: "stream_event", session_id: "s", parent_tool_use_id: null, event }));
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hi" } })).toEqual([{ type: "messageDelta", text: "Hi" }]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "hm" } })).toEqual([{ type: "reasoningDelta", text: "hm" }]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{" } })).toEqual([]);
    // A message's usage is live usage (Task 9), never a delta.
    expect(ev({ type: "message_start", message: { id: "m", usage: { input_tokens: 3 } } })).toEqual([{ type: "usage", input: 3, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 0, costBasis: "reported" }]);
    expect(ev({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "" } })).toEqual([]);
  });
});

describe("results", () => {
  test("usage is what each result added (cumulative per process)", () => {
    const u = new ClaudeUsage();
    const ev = (input: number, output: number, cacheRead: number, costMicros: number) => ({ type: "usage", input, output, cacheRead, cacheWrite: 0, reasoning: 0, costMicros, costBasis: "reported" });
    expect(u.usage(result({ total_cost_usd: 0.001, usage: { input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 5 } }))).toEqual([ev(10, 2, 5, 1000)]);
    expect(u.usage(result({ total_cost_usd: 0.003, usage: { input_tokens: 25, output_tokens: 6, cache_read_input_tokens: 5 } }))).toEqual([ev(15, 4, 0, 2000)]);
    // Nothing added: no event; a total that goes back charges nothing.
    expect(u.usage(result({ total_cost_usd: 0.003, usage: { input_tokens: 25, output_tokens: 6, cache_read_input_tokens: 5 } }))).toEqual([]);
    expect(u.usage(result({ total_cost_usd: 0.001, usage: { input_tokens: 1, output_tokens: 1 } }))).toEqual([]);
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
    expect(new LineMapper().map(line({ type: "system", subtype: "api_retry", attempt: 2, retry_delay_ms: 500, error_status: 429 }))).toEqual([{ type: "notice", name: "retry", details: { attempt: 2, code: "PROVIDER_RATE_LIMITED", delayMs: 500 } }]);
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

describe("the signed-in CLI's lines (7D Task 0, 2.1.321)", () => {
  // Scrubbed copies: s1a's rate_limit_event, s2a's refused outside writes and its result, s1b's
  // interrupt (two user lines and the aborted result), s1f's budget stop.
  const real = fixture("signed-in");
  const results = real.filter((l): l is ResultMsg => l.kind === "result");

  test("rate_limit_event_is_silent_while_allowed: a notice once it is not", () => {
    const m = new LineMapper();
    expect(real[0]).toMatchObject({ kind: "rateLimit", rate_limit_info: { status: "allowed_warning", rateLimitType: "seven_day", utilization: 0.29 } });
    expect(all(m, [real[0]!])).toEqual([]);
    expect(all(m, [line({ type: "rate_limit_event", rate_limit_info: { status: "allowed" } })])).toEqual([]);
    expect(all(m, [line({ type: "rate_limit_event", rate_limit_info: { status: "rejected", resetsAt: 1795093200, rateLimitType: "five_hour", utilization: 1 } })])).toEqual([
      { type: "notice", name: "rateLimit", details: { harness: "claudeCode", status: "rejected", window: "five_hour", resetsAt: 1795093200, utilization: 1 } },
    ]);
  });

  test("interrupt_result_ends_the_turn_interrupted: only while Finisher interrupts; an abort it did not ask for fails the turn", () => {
    const tail = real.slice(-4, -1);
    expect(tail.map((l) => l.kind)).toEqual(["user", "user", "result"]);
    const content = (interrupting: boolean) => all(new LineMapper(), tail, interrupting).filter((e) => e.type !== "usage" && !(e.type === "notice" && e.name === "modelUsage"));
    expect(content(true)).toEqual([
      { type: "toolCompleted", providerId: "toolu_01S", ok: false, output: expect.stringMatching(/^The user doesn't want to proceed/) },
      { type: "turnCompleted", stopReason: "interrupted" },
    ]);
    // Review of part B (S1): fail closed, so a held steer never starts a run of its own.
    const unasked = content(false);
    expect(unasked.some((e) => e.type === "turnCompleted")).toBe(false);
    expect(unasked.at(-1)).toMatchObject({ type: "turnFailed", error: { code: "PROVIDER_UNAVAILABLE", details: { terminalReason: "aborted_tools" } } });
    // Absent, not null (s1b): the key is not there at all.
    expect(Object.hasOwn(results[1]!, "result")).toBe(false);
    // Unverified: an interrupt with no tool running; counted only while Finisher interrupts.
    const streaming = result({ is_error: true, subtype: "error_during_execution", result: undefined, stop_reason: null });
    expect(new LineMapper().map(streaming, true).at(-1)).toEqual({ type: "turnCompleted", stopReason: "interrupted" });
    expect(new LineMapper().map(streaming, false).at(-1)).toMatchObject({ type: "turnFailed", error: { code: "PROVIDER_UNAVAILABLE" } });
  });

  test("outside_write_is_cli_denied: a notice per denial no prompt asked about, never an approval", () => {
    const asked = new Set(["toolu_01B"]);
    let ended = 0;
    const events = all(new LineMapper(undefined, { asked: (d) => d.tool_use_id !== undefined && asked.has(d.tool_use_id), turnEnded: () => ended++ }), real.slice(1, 6));
    // The prompts' memory is per turn: dropped at each result.
    expect(ended).toBe(1);
    expect(events.filter((e) => e.type === "request")).toEqual([]);
    expect(events.filter((e) => e.type === "toolCompleted").map((e) => e.type === "toolCompleted" && [e.providerId, e.ok])).toEqual([
      ["toolu_01W", false],
      ["toolu_01E", false],
    ]);
    expect(events.filter((e) => e.type === "notice" && e.name === "cliDenied")).toEqual([
      { type: "notice", name: "cliDenied", details: { harness: "claudeCode", tool: "Write", toolUseId: "toolu_01W", path: "/srv/finisher/proj/out.txt" } },
      { type: "notice", name: "cliDenied", details: { harness: "claudeCode", tool: "Edit", toolUseId: "toolu_01E", path: "/srv/finisher/proj/b.txt" } },
    ]);
    expect(events.at(-1)).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
    // Review of part B (O1): a malformed entry is skipped with a notice; the result (its usage)
    // still decodes and ends the turn. An entry without a `tool_use_id` is still a denial.
    const odd = line({ ...JSON.parse(JSON.stringify(results[0])), kind: undefined, type: "result", permission_denials: [{ tool_name: 7 }, "x", null, { tool_name: "Write", tool_input: { file_path: "/x" } }] });
    const oddEvents = all(new LineMapper(), [odd]);
    expect(oddEvents.filter((e) => e.type === "notice" && (e.name === "cliDenied" || e.name === "malformedDenials"))).toEqual([
      { type: "notice", name: "cliDenied", details: { harness: "claudeCode", tool: "Write", toolUseId: null, path: "/x" } },
      { type: "notice", name: "malformedDenials", details: { harness: "claudeCode", count: 3 } },
    ]);
    expect(oddEvents.at(-1)).toEqual({ type: "turnCompleted", stopReason: "endTurn" });
  });

  test("budget_stop_is_budget_exceeded: keyed on terminal_reason too", () => {
    const budget = results[2]!;
    expect(budget).toMatchObject({ subtype: "error_max_budget_usd", terminal_reason: "budget_exhausted", errors: ["Reached maximum budget ($0.0002)"] });
    expect(Object.hasOwn(budget, "result")).toBe(false);
    expect(all(new LineMapper(), [budget]).at(-1)).toMatchObject({ type: "turnFailed", error: { code: "BUDGET_EXCEEDED", details: { kind: "money" } } });
    expect(resultError(result({ is_error: true, subtype: "error_during_execution", terminal_reason: "budget_exhausted" }), false).code).toBe("BUDGET_EXCEEDED");
  });

  test("thinking_tokens_is_silent (review of part B: s2a, s2c, s3A, s3C, s3D)", () => {
    const m = new LineMapper();
    // A scrubbed copy of s2a's line.
    const real = line({ type: "system", subtype: "thinking_tokens", estimated_tokens: 50, estimated_tokens_delta: 50, session_id: "01890a5d-ac96-774b-bcce-b302099a8057", uuid: "5b1c3e1a-0d55-4a8f-9d4c-2c1f8e9e1100" });
    expect(all(m, [real, real])).toEqual([]);
  });

  test("background_task_lines_are_unsolicited_turn_notices", () => {
    const m = new LineMapper();
    for (const sub of ["task_started", "task_updated", "task_notification", "background_tasks_changed"]) {
      expect(all(m, [line({ type: "system", subtype: sub, session_id: "s" })])).toEqual([{ type: "notice", name: "unsolicitedTurn", details: { harness: "claudeCode", line: `system/${sub}` } }]);
    }
  });
});
