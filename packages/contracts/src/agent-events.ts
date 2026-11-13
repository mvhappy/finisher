// The agent runtime's provider events and enums (sub-project 7 §3.2, §4.1; plan 7A Decision 5).
// TS-owned control-plane state, so Effect Schemas here rather than generated types. Adapters
// emit only `ProviderEvent`s; the runtime turns them into nodes.
import { Schema } from "effect";

export const Harness = Schema.Literals(["builtin", "claudeCode", "codex", "fake", "plugin"]);
export type Harness = typeof Harness.Type;

export const Mode = Schema.Literals(["plan", "draft", "auto"]);
export type Mode = typeof Mode.Type;

export const SessionStatus = Schema.Literals(["starting", "idle", "running", "waitingApproval", "interrupted", "ended"]);
export type SessionStatus = typeof SessionStatus.Type;

export const NodeKind = Schema.Literals([
  "userMessage",
  "assistantMessage",
  "reasoning",
  "plan",
  "toolCall",
  "toolResult",
  "approval",
  "subagent",
  "usage",
  "notice",
  "error",
  "summary",
]);
export type NodeKind = typeof NodeKind.Type;

const Count = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0));

/** A starter's or a role's budget for a session; absent fields take the defaults. */
export const BudgetSpec = Schema.Struct({
  tokens: Schema.optionalKey(Count),
  moneyMicros: Schema.optionalKey(Count),
  wallMs: Schema.optionalKey(Count),
  modelCallsPerRun: Schema.optionalKey(Count),
  toolCallsPerRun: Schema.optionalKey(Count),
});
export type BudgetSpec = typeof BudgetSpec.Type;

/** `{code, message, details, fix?}`, as a plain value inside an event. */
export const ErrorShape = Schema.Struct({
  code: Schema.String,
  message: Schema.String,
  details: Schema.Unknown,
  fix: Schema.optionalKey(Schema.String),
});

export const ContentBlock = Schema.Union([
  Schema.Struct({ type: Schema.Literal("text"), text: Schema.String }),
  Schema.Struct({ type: Schema.Literal("image"), mediaType: Schema.String, data: Schema.String }),
]);
export type ContentBlock = typeof ContentBlock.Type;

const ev = <T extends string, F extends Schema.Struct.Fields>(type: T, fields: F) =>
  Schema.Struct({ type: Schema.Literal(type), ...fields });

const TurnStarted = ev("turnStarted", {});
const MessageDelta = ev("messageDelta", { text: Schema.String });
const Message = ev("message", { role: Schema.Literals(["assistant", "user"]), blocks: Schema.Array(ContentBlock) });
const ReasoningDelta = ev("reasoningDelta", { text: Schema.String });
const Reasoning = ev("reasoning", {
  text: Schema.optionalKey(Schema.String),
  summary: Schema.optionalKey(Schema.String),
  /** Opaque provider reasoning, sent back unchanged on the next call. */
  encrypted: Schema.optionalKey(Schema.Unknown),
});
const Plan = ev("plan", {
  steps: Schema.Array(Schema.Struct({ text: Schema.String, status: Schema.Literals(["pending", "inProgress", "completed"]) })),
});
const ToolStarted = ev("toolStarted", { providerId: Schema.String, tool: Schema.String, input: Schema.Unknown });
const ToolOutputDelta = ev("toolOutputDelta", { providerId: Schema.String, text: Schema.String });
const ToolCompleted = ev("toolCompleted", { providerId: Schema.String, ok: Schema.Boolean, output: Schema.Unknown });
const Request = ev("request", {
  id: Schema.String,
  kind: Schema.Literals(["approval", "userInput"]),
  tool: Schema.optionalKey(Schema.String),
  input: Schema.optionalKey(Schema.Unknown),
});
const UsageEvent = ev("usage", {
  input: Count,
  output: Count,
  cacheRead: Count,
  cacheWrite: Count,
  reasoning: Count,
  costMicros: Schema.optionalKey(Count),
  /**
   * How the adapter knows `costMicros` (plan 7D Decision 15): `reported` by the provider, or
   * `subscription` (a plan sign-in: the money is shown, never charged against money budgets).
   * Absent: the runtime's pricing decides.
   */
  costBasis: Schema.optionalKey(Schema.Literals(["reported", "subscription"])),
});
const TurnCompleted = ev("turnCompleted", { stopReason: Schema.String });
const TurnFailed = ev("turnFailed", { error: ErrorShape });
const Exited = ev("exited", { code: Schema.NullOr(Schema.Number), signal: Schema.NullOr(Schema.String) });
/** The exact model traffic of one call (built-in only): the body as sent and the response. */
const Capture = ev("capture", { request: Schema.Unknown, response: Schema.Unknown });
/**
 * Something the adapter wants on the record that is not a turn's content (Decision 9: each
 * model-call retry): becomes a `notice` node `{[name]: details}`.
 */
const Notice = ev("notice", { name: Schema.String, details: Schema.optionalKey(Schema.Unknown) });
/**
 * The built-in loop's compaction (7B Decision 24): the summary that stands for the runs before
 * the current one from here on, written with the versioned `prompt`. Becomes a `summary` node.
 */
const Summary = ev("summary", { prompt: Schema.String, text: Schema.String });
/** A provider message the adapter does not know: becomes a `notice` node, never a failure. */
const UnknownEvent = ev("unknown", { name: Schema.String });

export const ProviderEvent = Schema.Union([
  TurnStarted,
  MessageDelta,
  Message,
  ReasoningDelta,
  Reasoning,
  Plan,
  ToolStarted,
  ToolOutputDelta,
  ToolCompleted,
  Request,
  UsageEvent,
  TurnCompleted,
  TurnFailed,
  Exited,
  Capture,
  Notice,
  Summary,
  UnknownEvent,
]);
export type ProviderEvent = typeof ProviderEvent.Type;

/** Every `ProviderEvent` type, in union order. */
export const PROVIDER_EVENT_TYPES = [
  "turnStarted",
  "messageDelta",
  "message",
  "reasoningDelta",
  "reasoning",
  "plan",
  "toolStarted",
  "toolOutputDelta",
  "toolCompleted",
  "request",
  "usage",
  "turnCompleted",
  "turnFailed",
  "exited",
  "capture",
  "notice",
  "summary",
  "unknown",
] as const satisfies readonly ProviderEvent["type"][];
