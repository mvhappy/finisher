// Claude Code's lines as the runtime's `ProviderEvent`s (plan 7D Task 7, Decision 14). Pure but
// for its per-instance memory (the usage totals seen, the unknown names already noticed): the
// session (`session.ts`) feeds it every decoded line that passed the first-turn gate, and
// handles the control lines (requests, responses, cancels) itself.
//
// - `stream_event` text and thinking deltas → `messageDelta`/`reasoningDelta` (streamed, never
//   stored); other stream events → nothing (Task 9 reads `message_start`/`message_delta` usage).
// - `assistant` → `reasoning` per thinking block (the signature kept as `encrypted`), a
//   `message` per run of text blocks, a `toolStarted` per `tool_use`, in block order. The CLI's
//   own error turn (`model: "<synthetic>"`, `is_api_error_message`) maps to nothing: its
//   `result` carries the failure (Task 0).
// - `user` → a `toolCompleted` per `tool_result` block; text blocks (replays) → nothing.
// - A line with a non-null `parent_tool_use_id` (a subagent's: subagents are off, Decision 6)
//   → `notice {unexpectedSubagent}` only.
// - `result` → `usage` (the one place usage is mapped: `UsageSource`, Task 9's `usage.ts`
//   replaces the default), then `turnCompleted` once no steer is queued
//   (`queued_turn_count == 0`, Decision 13), or `turnFailed` when `is_error` (keyed on
//   `is_error`, never `subtype`: 2.1.321 writes `success` on the "Not logged in" turn).
// - `system/api_retry` → `notice {retry}`; `system/compact_boundary` → `notice {compacted}`;
//   `system/status`, `system/ui_invalidate`, `system/permission_denied` → nothing; `system/init`
//   → nothing (the gate checked it); any other type or subtype → `unknown`, once per name.
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { AssistantMsg, CliLine, ResultMsg, UserMsg } from "./protocol.ts";

type Event = ProviderEvent;

/** `system` subtypes that occur on every start and turn and carry nothing the record needs. */
const SILENT_SYSTEM: ReadonlySet<string> = new Set(["status", "ui_invalidate", "permission_denied"]);

/** The "Not logged in" turn's `result` text (Task 0, 2.1.321: "Not logged in · Please run /login"). */
const NOT_LOGGED_IN = /^Not logged in\b/;

export const SIGNIN_FIX = "finisherd provider signin claudeCode";

/** Where a `result`'s usage becomes a `usage` event: the seam Task 9's `usage.ts` fills. */
export interface UsageSource {
  /** The `usage` event for this `result`, or null when it adds nothing. */
  usage(result: ResultMsg): Extract<Event, { type: "usage" }> | null;
}

interface Totals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  costUsd: number;
}

const ZERO: Totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, costUsd: 0 };

/**
 * The CLI's `result.usage` and `total_cost_usd` are per process and cumulative (docs): each
 * `result` is charged what it added since this instance's last one.
 * TODO(7D Task 9): totals restored on `--resume` count from the earlier runs (CLI reference;
 * modelled by fake-claude as a resumed transcript's totals carried into the first `result`), so
 * a resumed instance's first `result` is charged them again here; `usage.ts` keeps the totals
 * across instances, and adds `costBasis` and the per-model split.
 */
export function cumulativeUsage(): UsageSource {
  let last: Totals = ZERO;
  return {
    usage(r) {
      const now: Totals = {
        input: r.usage.input_tokens,
        output: r.usage.output_tokens,
        cacheRead: r.usage.cache_read_input_tokens ?? 0,
        cacheWrite: r.usage.cache_creation_input_tokens ?? 0,
        costUsd: r.total_cost_usd,
      };
      // A total never goes back: a smaller one charges nothing and keeps the larger.
      const d = (k: keyof Totals) => Math.max(0, now[k] - last[k]);
      const delta = { input: d("input"), output: d("output"), cacheRead: d("cacheRead"), cacheWrite: d("cacheWrite") };
      const costMicros = Math.round(d("costUsd") * 1_000_000);
      last = { input: Math.max(now.input, last.input), output: Math.max(now.output, last.output), cacheRead: Math.max(now.cacheRead, last.cacheRead), cacheWrite: Math.max(now.cacheWrite, last.cacheWrite), costUsd: Math.max(now.costUsd, last.costUsd) };
      if (delta.input + delta.output + delta.cacheRead + delta.cacheWrite === 0 && costMicros === 0) return null;
      return { type: "usage", ...delta, reasoning: 0, costMicros };
    },
  };
}

/** The runtime's stop reasons for the API's. */
const STOP_REASONS: Readonly<Record<string, string>> = { end_turn: "endTurn", max_tokens: "maxTokens", stop_sequence: "stopSequence", tool_use: "toolUse", refusal: "refusal", pause_turn: "pauseTurn" };

const text200 = (s: string | undefined): string => (s ?? "").slice(0, 200);

/**
 * A failed turn's error (Decision 14): 401/403 and "Not logged in" → `PROVIDER_AUTH` (the latter
 * with the sign-in command as its fix), 429 → `PROVIDER_RATE_LIMITED`, 529 →
 * `PROVIDER_OVERLOADED`, the money backstop → `BUDGET_EXCEEDED`, a turn Finisher interrupted →
 * `SESSION_INTERRUPTED`; anything else is the CLI's own error, `PROVIDER_UNAVAILABLE`.
 */
export function resultError(r: ResultMsg, interrupting: boolean): FinError {
  const status = r.api_error_status ?? null;
  const details = { harness: "claudeCode", subtype: r.subtype.slice(0, 64), ...(status === null ? {} : { status }) };
  if (NOT_LOGGED_IN.test(r.result ?? "")) return finError("PROVIDER_AUTH", "Claude Code is not signed in on this server", { ...details, reason: "notLoggedIn" }, SIGNIN_FIX);
  if (status === 401 || status === 403) return finError("PROVIDER_AUTH", "Claude Code's sign-in was refused", details, SIGNIN_FIX);
  if (status === 429) return finError("PROVIDER_RATE_LIMITED", "Claude Code is rate limited", details);
  if (status === 529) return finError("PROVIDER_OVERLOADED", "Claude Code's model is overloaded", details);
  // TODO(7D Task 0 Step 1): the `--max-budget-usd` stop's `result` shape; modelled by
  // fake-claude as `subtype: "error_max_budget_usd"`, `is_error: true` (Task 9 maps it fully).
  if (r.subtype === "error_max_budget_usd") return finError("BUDGET_EXCEEDED", "the session's money budget is used up", { ...details, kind: "money" });
  // TODO(7D Task 0 Step 1): the `result` of a turn the `interrupt` request (or SIGINT) ended;
  // modelled by fake-claude as `subtype: "error_during_execution"`, `is_error: true`, empty text.
  if (interrupting) return finError("SESSION_INTERRUPTED", "the Claude Code turn was interrupted", details);
  return finError("PROVIDER_UNAVAILABLE", `Claude Code failed the turn: ${text200(r.result ?? r.errors?.[0])}`, { ...details, reason: "turnError" });
}

/** What one instance's lines become, with the memory the mapping needs. */
export class LineMapper {
  readonly #usage: UsageSource;
  readonly #noticed = new Set<string>();

  constructor(usage: UsageSource = cumulativeUsage()) {
    this.#usage = usage;
  }

  /** `unknown {name}` the first time `name` is seen on this instance, else nothing. */
  #unknown(name: string): Event[] {
    if (this.#noticed.has(name)) return [];
    this.#noticed.add(name);
    return [{ type: "unknown", name: name.slice(0, 64) }];
  }

  /**
   * The events of `line`. `interrupting` is set while Finisher's interrupt is in flight (a
   * failed `result` then is the interrupt's). Control lines are the session's: nothing here.
   */
  map(line: CliLine, interrupting = false): Event[] {
    switch (line.kind) {
      case "stream":
        if (line.parent_tool_use_id != null) return [];
        if (line.event.type === "content_block_delta") {
          const d = line.event.delta;
          if (d.type === "text_delta") return d.text === "" ? [] : [{ type: "messageDelta", text: d.text }];
          if (d.type === "thinking_delta") return d.thinking === "" ? [] : [{ type: "reasoningDelta", text: d.thinking }];
          if (d.type === "unknown") return this.#unknown(`stream_event/${d.name}`);
          return [];
        }
        if (line.event.type === "unknown") return this.#unknown(`stream_event/${line.event.name}`);
        return [];
      case "assistant":
        return this.#assistant(line);
      case "user":
        return this.#user(line);
      case "result":
        return this.#result(line, interrupting);
      case "apiRetry":
        return [{ type: "notice", name: "retry", details: { attempt: line.attempt, code: line.error_status ?? null, delayMs: line.retry_delay_ms } }];
      case "compactBoundary":
        return [{ type: "notice", name: "compacted", details: { trigger: line.compact_metadata?.trigger ?? null, preTokens: line.compact_metadata?.pre_tokens ?? null } }];
      case "system":
        return SILENT_SYSTEM.has(line.subtype) ? [] : this.#unknown(`system/${line.subtype}`);
      case "unknown":
        return this.#unknown(line.type);
      case "init":
      case "controlRequest":
      case "controlResponse":
      case "controlCancel":
        return [];
    }
  }

  #subagent(parent: string | null | undefined): Event[] | null {
    if (parent === null || parent === undefined) return null;
    return [{ type: "notice", name: "unexpectedSubagent", details: { parentToolUseId: parent.slice(0, 128) } }];
  }

  #assistant(l: AssistantMsg): Event[] {
    const sub = this.#subagent(l.parent_tool_use_id);
    if (sub !== null) return sub;
    if (l.is_api_error_message === true || l.message.model === "<synthetic>") return [];
    const out: Event[] = [];
    let text: { type: "text"; text: string }[] = [];
    const flush = () => {
      if (text.length > 0) out.push({ type: "message", role: "assistant", blocks: text });
      text = [];
    };
    for (const b of l.message.content) {
      switch (b.type) {
        case "text":
          if (b.text !== "") text.push({ type: "text", text: b.text });
          break;
        case "thinking":
          flush();
          out.push({ type: "reasoning", text: b.thinking, ...(b.signature === undefined ? {} : { encrypted: { signature: b.signature } }) });
          break;
        case "redacted_thinking":
          flush();
          out.push({ type: "reasoning", encrypted: { redacted: b.data } });
          break;
        case "tool_use":
          flush();
          out.push({ type: "toolStarted", providerId: b.id, tool: b.name, input: b.input });
          break;
        case "unknown":
          flush();
          out.push(...this.#unknown(`assistant/${b.name}`));
          break;
      }
    }
    flush();
    return out;
  }

  #user(l: UserMsg): Event[] {
    const sub = this.#subagent(l.parent_tool_use_id);
    if (sub !== null) return sub;
    if (typeof l.message.content === "string") return [];
    const out: Event[] = [];
    for (const b of l.message.content) {
      if (b.type === "tool_result") out.push({ type: "toolCompleted", providerId: b.tool_use_id, ok: b.is_error !== true, output: b.content ?? null });
      else if (b.type === "unknown") out.push(...this.#unknown(`user/${b.name}`));
    }
    return out;
  }

  #result(r: ResultMsg, interrupting: boolean): Event[] {
    const out: Event[] = [];
    const usage = this.#usage.usage(r);
    if (usage !== null) out.push(usage);
    if (r.is_error) {
      const e = resultError(r, interrupting);
      out.push({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details, ...(e.fix === undefined ? {} : { fix: e.fix }) } });
      return out;
    }
    // TODO(7D Task 0 Step 1): whether a steer joins the running turn or is queued as the next
    // one; modelled by fake-claude as queued (`queued_turn_count` counts it, its own `result`
    // follows). Either way the Finisher run ends at the first `result` with none queued.
    if ((r.queued_turn_count ?? 0) > 0) return out;
    const stop = r.stop_reason ?? "end_turn";
    out.push({ type: "turnCompleted", stopReason: STOP_REASONS[stop] ?? stop.slice(0, 64) });
    return out;
  }
}
