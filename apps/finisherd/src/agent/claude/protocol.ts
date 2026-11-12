// Claude Code's stream-JSON lines and its control protocol (plan 7D Task 2, Decisions 13–18).
// The CLI runs `-p --input-format stream-json --output-format stream-json`: one JSON object per
// `\n`-terminated line each way. Shapes follow the CLI's documentation and the Agent SDK's
// published source (read as documentation only, never a dependency).
//
// The decode rule (Review Focus 6, drift fails closed): a `type` (or a `system` subtype, a
// `control_request` subtype, a content-block or stream-event type) this module does not know
// becomes an "unknown" value the mapper turns into a notice, never an error; a known type
// missing a field the adapter needs is `PROVIDER_PROTOCOL`; fields a newer CLI adds are
// dropped; a line over 16 MiB is `PROVIDER_PROTOCOL`. `decodeLine` never throws.
import type { FinError } from "@finisher/contracts";
import { Schema } from "effect";
import { finError } from "../../errors.ts";

/** The longest line accepted either way, in UTF-8 bytes. */
export const MAX_LINE_BYTES = 16 * 1024 * 1024;

const Str = Schema.String;
const Num = Schema.Number;
const opt = Schema.optionalKey;
const JsonObject = Schema.Record(Schema.String, Schema.Unknown);

// ---- content blocks (assistant and user messages) ----

const TextBlock = Schema.Struct({ type: Schema.Literal("text"), text: Str });
const ThinkingBlock = Schema.Struct({ type: Schema.Literal("thinking"), thinking: Str, signature: opt(Str) });
const RedactedThinkingBlock = Schema.Struct({ type: Schema.Literal("redacted_thinking"), data: Str });
const ToolUseBlock = Schema.Struct({ type: Schema.Literal("tool_use"), id: Str, name: Str, input: JsonObject });
const ToolResultBlock = Schema.Struct({
  type: Schema.Literal("tool_result"),
  tool_use_id: Str,
  content: opt(Schema.Union([Str, Schema.Array(Schema.Unknown), Schema.Null])),
  is_error: opt(Schema.NullOr(Schema.Boolean)),
});

/** A block of a type this module does not know (an image, a server tool, a newer CLI's). */
export interface UnknownBlock {
  readonly type: "unknown";
  readonly name: string;
}
export type AssistantBlock =
  | typeof TextBlock.Type
  | typeof ThinkingBlock.Type
  | typeof RedactedThinkingBlock.Type
  | typeof ToolUseBlock.Type
  | UnknownBlock;
export type UserBlock = typeof TextBlock.Type | typeof ToolResultBlock.Type | UnknownBlock;

const ASSISTANT_BLOCKS: Record<string, Schema.Top> = {
  text: TextBlock,
  thinking: ThinkingBlock,
  redacted_thinking: RedactedThinkingBlock,
  tool_use: ToolUseBlock,
};
const USER_BLOCKS: Record<string, Schema.Top> = { text: TextBlock, tool_result: ToolResultBlock };

// ---- token usage ----

const Usage = Schema.Struct({
  input_tokens: Num,
  output_tokens: Num,
  cache_creation_input_tokens: opt(Schema.NullOr(Num)),
  cache_read_input_tokens: opt(Schema.NullOr(Num)),
});
export type Usage = typeof Usage.Type;

const ModelUsage = Schema.Struct({
  inputTokens: opt(Num),
  outputTokens: opt(Num),
  cacheReadInputTokens: opt(Num),
  cacheCreationInputTokens: opt(Num),
  costUSD: opt(Num),
});

// ---- the lines ----

const Envelope = { uuid: opt(Str), session_id: opt(Str), parent_tool_use_id: opt(Schema.NullOr(Str)) };

const SystemInitLine = Schema.Struct({
  subtype: Schema.Literal("init"),
  session_id: Str,
  tools: Schema.Array(Str),
  model: Str,
  permissionMode: opt(Str),
  apiKeySource: opt(Str),
  cwd: opt(Str),
  claude_code_version: opt(Str),
  mcp_servers: opt(Schema.Array(Schema.Struct({ name: Str, status: opt(Str) }))),
  plugins: opt(Schema.Array(Schema.Struct({ name: Str, path: opt(Str), source: opt(Str) }))),
  skills: opt(Schema.Array(Schema.Unknown)),
  slash_commands: opt(Schema.Array(Schema.Unknown)),
  agents: opt(Schema.Array(Schema.Unknown)),
  // `null` means auto memory is off; absent is not the same and is kept absent.
  memory_paths: opt(Schema.Unknown),
  capabilities: opt(Schema.Array(Str)),
  output_style: opt(Str),
  messaging_socket_path: opt(Schema.NullOr(Str)),
});
const ApiRetryLine = Schema.Struct({
  subtype: Schema.Literal("api_retry"),
  attempt: Num,
  retry_delay_ms: Num,
  max_retries: opt(Num),
  error_status: opt(Schema.NullOr(Num)),
  error: opt(Schema.Unknown),
});
const CompactBoundaryLine = Schema.Struct({
  subtype: Schema.Literal("compact_boundary"),
  compact_metadata: opt(Schema.Struct({ trigger: opt(Str), pre_tokens: opt(Num) })),
});
const AssistantLine = Schema.Struct({
  ...Envelope,
  message: Schema.Struct({
    id: opt(Str),
    model: opt(Str),
    content: Schema.Array(Schema.Unknown),
    stop_reason: opt(Schema.NullOr(Str)),
    usage: opt(Usage),
  }),
});
const UserLine = Schema.Struct({
  ...Envelope,
  message: Schema.Struct({ content: Schema.Union([Str, Schema.Array(Schema.Unknown)]) }),
});
const StreamEventLine = Schema.Struct({ ...Envelope, event: Schema.Struct({ type: Str }) });
const ResultLine = Schema.Struct({
  subtype: Str,
  is_error: Schema.Boolean,
  usage: Usage,
  total_cost_usd: Num,
  session_id: opt(Str),
  result: opt(Str),
  duration_ms: opt(Num),
  duration_api_ms: opt(Num),
  num_turns: opt(Num),
  stop_reason: opt(Schema.NullOr(Str)),
  api_error_status: opt(Schema.NullOr(Num)),
  queued_turn_count: opt(Num),
  modelUsage: opt(Schema.Record(Str, ModelUsage)),
  permission_denials: opt(Schema.Array(Schema.Unknown)),
});
const ControlRequestLine = Schema.Struct({ request_id: Str, request: Schema.Struct({ subtype: Str }) });
const CanUseTool = Schema.Struct({
  subtype: Schema.Literal("can_use_tool"),
  tool_name: Str,
  input: JsonObject,
  tool_use_id: opt(Str),
  permission_suggestions: opt(Schema.NullOr(Schema.Array(Schema.Unknown))),
  blocked_path: opt(Schema.NullOr(Str)),
  decision_reason: opt(Schema.Unknown),
});
const HookCallback = Schema.Struct({ subtype: Schema.Literal("hook_callback"), callback_id: Str, input: opt(Schema.Unknown), tool_use_id: opt(Schema.NullOr(Str)) });
const McpMessage = Schema.Struct({ subtype: Schema.Literal("mcp_message"), server_name: Str, message: Schema.Unknown });
const ControlResponseLine = Schema.Struct({
  response: Schema.Union([
    Schema.Struct({ subtype: Schema.Literal("success"), request_id: Str, response: opt(Schema.Unknown) }),
    Schema.Struct({ subtype: Schema.Literal("error"), request_id: Str, error: Str }),
  ]),
});
const ControlCancelLine = Schema.Struct({ request_id: Str });

// ---- stream events (`--include-partial-messages`): the Messages API's raw stream events ----

const StreamDelta = {
  text_delta: Schema.Struct({ type: Schema.Literal("text_delta"), text: Str }),
  thinking_delta: Schema.Struct({ type: Schema.Literal("thinking_delta"), thinking: Str }),
  signature_delta: Schema.Struct({ type: Schema.Literal("signature_delta"), signature: Str }),
  input_json_delta: Schema.Struct({ type: Schema.Literal("input_json_delta"), partial_json: Str }),
};
const PartialUsage = Schema.Struct({
  input_tokens: opt(Schema.NullOr(Num)),
  output_tokens: opt(Schema.NullOr(Num)),
  cache_creation_input_tokens: opt(Schema.NullOr(Num)),
  cache_read_input_tokens: opt(Schema.NullOr(Num)),
});
const MessageStart = Schema.Struct({
  type: Schema.Literal("message_start"),
  message: Schema.Struct({ id: opt(Str), model: opt(Str), usage: opt(PartialUsage) }),
});
const MessageDelta = Schema.Struct({
  type: Schema.Literal("message_delta"),
  delta: opt(Schema.Struct({ stop_reason: opt(Schema.NullOr(Str)) })),
  usage: opt(PartialUsage),
});
const MessageStop = Schema.Struct({ type: Schema.Literal("message_stop") });
const BlockStart = Schema.Struct({ type: Schema.Literal("content_block_start"), index: Num, content_block: Schema.Struct({ type: Str }) });
const BlockDelta = Schema.Struct({ type: Schema.Literal("content_block_delta"), index: Num, delta: Schema.Struct({ type: Str }) });
const BlockStop = Schema.Struct({ type: Schema.Literal("content_block_stop"), index: Num });
const STREAM_EVENTS: Record<string, Schema.Top> = {
  message_start: MessageStart,
  message_delta: MessageDelta,
  message_stop: MessageStop,
  content_block_start: BlockStart,
  content_block_delta: BlockDelta,
  content_block_stop: BlockStop,
};
/** A stream delta; a delta type this module does not know is `unknown`. */
export type StreamDeltaPayload = { [K in keyof typeof StreamDelta]: (typeof StreamDelta)[K]["Type"] }[keyof typeof StreamDelta] | { readonly type: "unknown"; readonly name: string };
export type StreamPayload =
  | typeof MessageStart.Type
  | typeof MessageDelta.Type
  | typeof MessageStop.Type
  | typeof BlockStart.Type
  | (Omit<typeof BlockDelta.Type, "delta"> & { readonly delta: StreamDeltaPayload })
  | typeof BlockStop.Type
  | { readonly type: "unknown"; readonly name: string };

// ---- the decoded line ----

type Env = { readonly uuid?: string; readonly session_id?: string; readonly parent_tool_use_id?: string | null };

export type SystemInit = { readonly kind: "init" } & Omit<typeof SystemInitLine.Type, "subtype">;
export type SystemOther =
  | ({ readonly kind: "apiRetry" } & Omit<typeof ApiRetryLine.Type, "subtype">)
  | ({ readonly kind: "compactBoundary" } & Omit<typeof CompactBoundaryLine.Type, "subtype">)
  /** Any other `system` subtype (`status`, `permission_denied`, a newer CLI's): its name only. */
  | { readonly kind: "system"; readonly subtype: string };
export interface AssistantMsg extends Env {
  readonly kind: "assistant";
  readonly message: Omit<typeof AssistantLine.Type.message, "content"> & { readonly content: readonly AssistantBlock[] };
}
export interface UserMsg extends Env {
  readonly kind: "user";
  readonly message: { readonly content: string | readonly UserBlock[] };
}
export interface StreamEvent extends Env {
  readonly kind: "stream";
  readonly event: StreamPayload;
}
export type ResultMsg = { readonly kind: "result" } & typeof ResultLine.Type;
export type CanUseToolRequest = typeof CanUseTool.Type;
export type CliRequest =
  | CanUseToolRequest
  | typeof HookCallback.Type
  | typeof McpMessage.Type
  /** A subtype this module does not know: the session answers it `{error}`. */
  | { readonly subtype: "unknown"; readonly name: string };
export interface ControlRequest {
  readonly kind: "controlRequest";
  readonly request_id: string;
  readonly request: CliRequest;
}
export type ControlResponse = { readonly kind: "controlResponse" } & typeof ControlResponseLine.Type;
export interface ControlCancel {
  readonly kind: "controlCancel";
  readonly request_id: string;
}
/** A line whose `type` this module does not know (the mapper records a notice). */
export interface Unknown {
  readonly kind: "unknown";
  readonly type: string;
}

export type CliLine = SystemInit | SystemOther | AssistantMsg | UserMsg | StreamEvent | ResultMsg | ControlRequest | ControlResponse | ControlCancel | Unknown;

// ---- decoding ----

const name64 = (s: string): string => s.slice(0, 64);

class Drift {
  readonly error: FinError;
  constructor(error: FinError) {
    this.error = error;
  }
}

/** `raw` through `schema`, or a thrown `Drift` naming where the line broke. */
function must<A>(schema: Schema.Top, raw: unknown, what: string): A {
  const r = Schema.decodeUnknownResult(schema as Schema.Codec<A>)(raw);
  if (r._tag === "Success") return r.success;
  throw new Drift(finError("PROVIDER_PROTOCOL", `a claude ${what} line does not match the protocol`, { what, issue: String(r.failure).slice(0, 500) }));
}

function blocks<B>(content: readonly unknown[], known: Record<string, Schema.Top>, what: string): B[] {
  return content.map((b) => {
    const type = (b as { type?: unknown } | null)?.type;
    if (typeof type !== "string") throw new Drift(finError("PROVIDER_PROTOCOL", `a claude ${what} content block without a type`, { what }));
    const schema = Object.hasOwn(known, type) ? known[type] : undefined;
    return schema === undefined ? ({ type: "unknown", name: name64(type) } as B) : must<B>(schema, b, `${what} ${type} block`);
  });
}

function streamPayload(event: { type: string }, raw: unknown): StreamPayload {
  const schema = Object.hasOwn(STREAM_EVENTS, event.type) ? STREAM_EVENTS[event.type] : undefined;
  if (schema === undefined) return { type: "unknown", name: name64(event.type) };
  const ev = must<StreamPayload>(schema, raw, `stream_event ${event.type}`);
  if (ev.type !== "content_block_delta") return ev;
  const rawDelta = (raw as { delta: unknown }).delta;
  const dt = ev.delta.type;
  const ds = Object.hasOwn(StreamDelta, dt) ? StreamDelta[dt as keyof typeof StreamDelta] : undefined;
  const delta: StreamDeltaPayload = ds === undefined ? { type: "unknown", name: name64(dt) } : must<StreamDeltaPayload>(ds, rawDelta, `stream_event ${dt}`);
  return { ...ev, delta };
}

function systemLine(o: Record<string, unknown>): SystemInit | SystemOther {
  const subtype = o.subtype;
  if (typeof subtype !== "string") throw new Drift(finError("PROVIDER_PROTOCOL", "a claude system line without a subtype", {}));
  switch (subtype) {
    case "init": {
      const { subtype: _, ...rest } = must<typeof SystemInitLine.Type>(SystemInitLine, o, "system/init");
      return { kind: "init", ...rest };
    }
    case "api_retry": {
      const { subtype: _, ...rest } = must<typeof ApiRetryLine.Type>(ApiRetryLine, o, "system/api_retry");
      return { kind: "apiRetry", ...rest };
    }
    case "compact_boundary": {
      const { subtype: _, ...rest } = must<typeof CompactBoundaryLine.Type>(CompactBoundaryLine, o, "system/compact_boundary");
      return { kind: "compactBoundary", ...rest };
    }
    default:
      return { kind: "system", subtype: name64(subtype) };
  }
}

function controlRequest(o: Record<string, unknown>): ControlRequest {
  const line = must<typeof ControlRequestLine.Type>(ControlRequestLine, o, "control_request");
  const raw = o.request;
  const sub = line.request.subtype;
  let request: CliRequest;
  if (sub === "can_use_tool") request = must(CanUseTool, raw, "control_request can_use_tool");
  else if (sub === "hook_callback") request = must(HookCallback, raw, "control_request hook_callback");
  else if (sub === "mcp_message") request = must(McpMessage, raw, "control_request mcp_message");
  else request = { subtype: "unknown", name: name64(sub) };
  return { kind: "controlRequest", request_id: line.request_id, request };
}

function decodeObject(o: Record<string, unknown>): CliLine {
  const type = o.type;
  if (typeof type !== "string" || type.length === 0) {
    throw new Drift(finError("PROVIDER_PROTOCOL", "a claude line without a type", { got: typeof type }));
  }
  switch (type) {
    case "system":
      return systemLine(o);
    case "assistant": {
      const line = must<typeof AssistantLine.Type>(AssistantLine, o, "assistant");
      const content = blocks<AssistantBlock>(line.message.content, ASSISTANT_BLOCKS, "assistant");
      return { kind: "assistant", ...line, message: { ...line.message, content } };
    }
    case "user": {
      const line = must<typeof UserLine.Type>(UserLine, o, "user");
      const c = line.message.content;
      const content = typeof c === "string" ? c : blocks<UserBlock>(c, USER_BLOCKS, "user");
      return { kind: "user", ...line, message: { content } };
    }
    case "stream_event": {
      const line = must<typeof StreamEventLine.Type>(StreamEventLine, o, "stream_event");
      return { kind: "stream", ...line, event: streamPayload(line.event, o.event) };
    }
    case "result":
      return { kind: "result", ...must<typeof ResultLine.Type>(ResultLine, o, "result") };
    case "control_request":
      return controlRequest(o);
    case "control_response":
      return { kind: "controlResponse", ...must<typeof ControlResponseLine.Type>(ControlResponseLine, o, "control_response") };
    case "control_cancel_request":
      return { kind: "controlCancel", request_id: must<typeof ControlCancelLine.Type>(ControlCancelLine, o, "control_cancel_request").request_id };
    default:
      return { kind: "unknown", type: name64(type) };
  }
}

/** UTF-8 length of `s` above `cap`, without encoding it when the answer is clear. */
function overCap(s: string, cap: number): boolean {
  if (s.length > cap) return true; // a UTF-16 code unit is at least one UTF-8 byte
  if (s.length * 3 <= cap) return false; // and at most three
  return Buffer.byteLength(s, "utf8") > cap;
}

/** One line of the CLI's output (without its `\n`). Never throws. */
export function decodeLine(raw: string): CliLine | FinError {
  try {
    if (overCap(raw, MAX_LINE_BYTES)) {
      return finError("PROVIDER_PROTOCOL", "a claude line is longer than 16 MiB", { maxBytes: MAX_LINE_BYTES });
    }
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch {
      return finError("PROVIDER_PROTOCOL", "a claude line is not JSON", { length: raw.length });
    }
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return finError("PROVIDER_PROTOCOL", "a claude line is not a JSON object", { got: Array.isArray(value) ? "array" : typeof value });
    }
    return decodeObject(value as Record<string, unknown>);
  } catch (e) {
    if (e instanceof Drift) return e.error;
    return finError("PROVIDER_PROTOCOL", "a claude line could not be decoded", { error: String(e).slice(0, 200) });
  }
}

// ---- NDJSON framing ----

export interface LineFramer {
  /** The complete lines in `chunk` plus what earlier chunks left; an over-long line is one `PROVIDER_PROTOCOL`. */
  push(chunk: Uint8Array | string): (string | FinError)[];
  /** The last line when the stream ended without a `\n`. */
  end(): (string | FinError)[];
}

/**
 * Splits a byte stream on `\n` (0x0A) only: a `\r` before it is dropped, U+2028/U+2029 and
 * `\r` alone stay inside the line, empty lines are skipped. Bytes are buffered, never decoded
 * until a line is whole, so a UTF-8 sequence split across chunks survives. The cap holds while
 * buffering: a line that passes `maxBytes` yields one error and is discarded up to its `\n`, so
 * an endless line cannot grow memory.
 */
export function lineFramer(maxBytes: number = MAX_LINE_BYTES): LineFramer {
  const utf8 = new TextDecoder("utf-8");
  let parts: Uint8Array[] = [];
  let size = 0;
  let discarding = false;

  const take = (tail: Uint8Array | null): string | null => {
    const all = tail === null ? parts : [...parts, tail];
    parts = [];
    size = 0;
    const bytes = all.length === 1 ? all[0]! : Buffer.concat(all);
    let n = bytes.length;
    if (n > 0 && bytes[n - 1] === 0x0d) n--;
    return n === 0 ? null : utf8.decode(bytes.subarray(0, n));
  };
  const tooLong = (): FinError => finError("PROVIDER_PROTOCOL", "a claude line is longer than the cap", { maxBytes });

  return {
    push(chunk) {
      let buf = typeof chunk === "string" ? Buffer.from(chunk, "utf8") : chunk;
      const out: (string | FinError)[] = [];
      for (;;) {
        const nl = buf.indexOf(0x0a);
        if (nl < 0) break;
        const head = buf.subarray(0, nl);
        buf = buf.subarray(nl + 1);
        if (discarding) {
          discarding = false;
          continue;
        }
        const crlf = head.length > 0 ? head[head.length - 1] === 0x0d : parts.length > 0 && parts.at(-1)!.at(-1) === 0x0d;
        if (size + head.length - (crlf ? 1 : 0) > maxBytes) {
          parts = [];
          size = 0;
          out.push(tooLong());
          continue;
        }
        const line = take(head);
        if (line !== null) out.push(line);
      }
      if (buf.length > 0 && !discarding) {
        // +1: a `\r` before the coming `\n` is not part of the line
        if (size + buf.length > maxBytes + 1) {
          parts = [];
          size = 0;
          discarding = true;
          out.push(tooLong());
        } else {
          parts.push(Uint8Array.from(buf));
          size += buf.length;
        }
      }
      return out;
    },
    end() {
      if (discarding) {
        discarding = false;
        return [];
      }
      if (size - (parts.at(-1)?.at(-1) === 0x0d ? 1 : 0) > maxBytes) {
        parts = [];
        size = 0;
        return [tooLong()];
      }
      const line = take(null);
      return line === null ? [] : [line];
    },
  };
}

// ---- encoding (host → CLI) ----

/** The permission modes Finisher sets; `bypassPermissions` is never one of them. */
export type HostPermissionMode = "default" | "acceptEdits" | "plan";
export type HostRequest = { readonly subtype: "initialize" } | { readonly subtype: "interrupt" } | { readonly subtype: "set_permission_mode"; readonly mode: HostPermissionMode };
export type CanUseToolAnswer =
  | { readonly behavior: "allow"; readonly updatedInput: Readonly<Record<string, unknown>> }
  | { readonly behavior: "deny"; readonly message: string; readonly interrupt?: boolean };
export interface McpReply {
  readonly mcp_response: unknown;
}

const line = (v: unknown): string => `${JSON.stringify(v)}\n`;

/** A user turn (or a steer): one text block per entry. The result ends in `\n`. */
export function encodeUser(text: readonly string[], sessionUuid: string): string {
  return line({
    type: "user",
    message: { role: "user", content: text.map((t) => ({ type: "text", text: t })) },
    parent_tool_use_id: null,
    session_id: sessionUuid,
  });
}

/** A host → CLI control request. The result ends in `\n`. */
export function encodeControl(requestId: string, req: HostRequest): string {
  let request: Record<string, unknown>;
  switch (req.subtype) {
    case "initialize":
      // As the Agent SDK sends it: no SDK hooks are registered (Decision 18).
      request = { subtype: "initialize", hooks: null };
      break;
    case "interrupt":
      request = { subtype: "interrupt" };
      break;
    case "set_permission_mode":
      request = { subtype: "set_permission_mode", mode: req.mode };
      break;
  }
  return line({ type: "control_request", request_id: requestId, request });
}

/**
 * The answer to a CLI → host control request: a `can_use_tool` decision, an MCP reply, or an
 * error. Fields are copied one by one, so `updatedPermissions` (a remembered allow is
 * Finisher's rule, never the CLI's) cannot ride along. The result ends in `\n`.
 */
export function encodeAnswer(requestId: string, a: CanUseToolAnswer | McpReply | { readonly error: string }): string {
  if ("error" in a) return line({ type: "control_response", response: { subtype: "error", request_id: requestId, error: a.error } });
  let response: Record<string, unknown>;
  if ("mcp_response" in a) response = { mcp_response: a.mcp_response };
  else if (a.behavior === "allow") response = { behavior: "allow", updatedInput: a.updatedInput };
  else response = { behavior: "deny", message: a.message, ...(a.interrupt === undefined ? {} : { interrupt: a.interrupt }) };
  return line({ type: "control_response", response: { subtype: "success", request_id: requestId, response } });
}
