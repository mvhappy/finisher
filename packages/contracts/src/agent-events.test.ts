// The ProviderEvent schema and the runtime enums (plan 7A Task 6a).
import { Schema } from "effect";
import { describe, expect, it } from "vitest";
import { BudgetSpec, Harness, Mode, NodeKind, PROVIDER_EVENT_TYPES, ProviderEvent, SessionStatus } from "./agent-events.ts";

const samples: ProviderEvent[] = [
  { type: "turnStarted" },
  { type: "messageDelta", text: "Trim" },
  { type: "message", role: "assistant", blocks: [{ type: "text", text: "Trimmed." }, { type: "image", mediaType: "image/png", data: "iVBO" }] },
  { type: "reasoningDelta", text: "the intro" },
  { type: "reasoning", text: "think", summary: "s", encrypted: { opaque: [1, 2] } },
  { type: "reasoning" },
  { type: "plan", steps: [{ text: "look", status: "completed" }, { text: "trim", status: "inProgress" }] },
  { type: "toolStarted", providerId: "call_1", tool: "fin", input: { argv: ["view"] } },
  { type: "toolOutputDelta", providerId: "call_1", text: "{" },
  { type: "toolCompleted", providerId: "call_1", ok: true, output: { txId: "x" } },
  { type: "request", id: "r1", kind: "approval", tool: "fin", input: { argv: ["merge"] } },
  { type: "request", id: "r2", kind: "userInput" },
  { type: "requestCancelled", id: "r1" },
  { type: "usage", input: 10, output: 2, cacheRead: 100, cacheWrite: 0, reasoning: 1, costMicros: 42 },
  { type: "usage", input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 },
  { type: "usage", input: 3, output: 1, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 4200, costBasis: "subscription" },
  { type: "usage", input: 3, output: 1, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 0, costBasis: "reported" },
  { type: "turnCompleted", stopReason: "endTurn" },
  { type: "turnFailed", error: { code: "PROVIDER_EXITED", message: "exited 1", details: { code: 1 }, fix: "resume" } },
  { type: "exited", code: 1, signal: null },
  { type: "exited", code: null, signal: "SIGKILL" },
  { type: "capture", request: { model: "m" }, response: { id: "gen-1" } },
  { type: "notice", name: "retry", details: { attempt: 1, code: "PROVIDER_RATE_LIMITED", delayMs: 3000 } },
  { type: "notice", name: "marker" },
  { type: "summary", prompt: "compaction-v1", text: "The intro was trimmed; the outro is next." },
  { type: "unknown", name: "rateLimitUpdate" },
];

const decode = Schema.decodeUnknownSync(ProviderEvent);
const encode = Schema.encodeSync(ProviderEvent);

describe("ProviderEvent", () => {
  it("round-trips every variant", () => {
    const seen = new Set<string>();
    for (const e of samples) {
      const wire = JSON.parse(JSON.stringify(encode(e)));
      expect(decode(wire)).toEqual(e);
      seen.add(e.type);
    }
    expect([...seen].sort()).toEqual([...PROVIDER_EVENT_TYPES].sort());
    expect(ProviderEvent.members.length).toBe(PROVIDER_EVENT_TYPES.length);
  });

  it("refuses an unknown type, a missing field or a negative count", () => {
    expect(() => decode({ type: "fork" })).toThrow();
    expect(() => decode({ type: "toolStarted", tool: "fin", input: {} })).toThrow();
    expect(() => decode({ type: "usage", input: -1, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 })).toThrow();
    expect(() => decode({ type: "usage", input: 1.5, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 })).toThrow();
    // An adapter names how it knows the cost, never the runtime's own pricing bases.
    expect(() => decode({ type: "usage", input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, costBasis: "priced" })).toThrow();
    expect(() => decode({ type: "message", role: "system", blocks: [] })).toThrow();
  });
});

describe("runtime enums", () => {
  it("hold the spec's values", () => {
    expect(Harness.literals).toEqual(["builtin", "claudeCode", "codex", "fake", "plugin"]);
    expect(Mode.literals).toEqual(["plan", "draft", "auto"]);
    expect(SessionStatus.literals).toEqual(["starting", "idle", "running", "waitingApproval", "interrupted", "ended"]);
    expect(NodeKind.literals).toContain("toolCall");
    expect(NodeKind.literals).toHaveLength(12);
    expect(Schema.decodeUnknownSync(BudgetSpec)({ tokens: 5, moneyMicros: 0 })).toEqual({ tokens: 5, moneyMicros: 0 });
    expect(() => Schema.decodeUnknownSync(BudgetSpec)({ tokens: -5 })).toThrow();
  });
});
