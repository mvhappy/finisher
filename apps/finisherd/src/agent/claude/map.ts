// Claude Code's lines as the runtime's `ProviderEvent`s (plan 7D Task 7, Decision 14). Pure but
// for its per-instance memory (the usage totals seen, the unknown names already noticed): the
// session (`session.ts`) feeds it every decoded line that passed the first-turn gate, and
// handles the control lines (requests, responses, cancels) itself.
//
// - `stream_event` text and thinking deltas → `messageDelta`/`reasoningDelta` (streamed, never
//   stored); `message_start`/`message_delta` usage → a live `usage` (`usage.ts`); others → nothing.
// - `assistant` → `reasoning` per thinking block (the signature kept as `encrypted`), a
//   `message` per run of text blocks, a `toolStarted` per `tool_use`, in block order. The CLI's
//   own error turn (`model: "<synthetic>"`, `is_api_error_message`) maps to nothing: its
//   `result` carries the failure (Task 0).
// - `user` → a `toolCompleted` per `tool_result` block; text blocks (replays) → nothing.
// - A line with a non-null `parent_tool_use_id` (a subagent's: subagents are off, Decision 6)
//   → `notice {unexpectedSubagent}` only.
// - `result` → `usage` and `notice {modelUsage}` (`UsageSource`, `usage.ts`), a
//   `notice {cliDenied}` per `permission_denials` entry no permission prompt asked about (the
//   CLI refused it itself: a Write/Edit outside the working directory), then the turn's end,
//   keyed on `terminal_reason` before `is_error` (Decision 14): while Finisher interrupts, the
//   `aborted_*` result is `turnCompleted {stopReason: "interrupted"}` (as the built-in loop ends
//   an interrupted turn), never a failure; otherwise `turnFailed` when `is_error` (never keyed on `subtype`:
//   2.1.321 writes `success` on the "Not logged in" turn), else `turnCompleted` (Decision 13:
//   a held steer's next CLI turn is the session's to withhold, `session.ts`; `queued_turn_count`
//   is not read).
// - `rate_limit_event` → nothing while its `status` is `allowed*`, else `notice {rateLimit}`.
// - `system/api_retry` → `notice {retry: {attempt, code, delayMs}}` (`code` as the built-in agent's); `system/compact_boundary` → `notice {compacted}`;
//   `system/status`, `system/ui_invalidate`, `system/permission_denied` → nothing; `system/init`
//   → nothing (the gate checked it); a background task's line → `notice {unsolicitedTurn}`
//   (Decision 5 turns them off); any other type or subtype → `unknown`, once per name.
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { AssistantMsg, CliLine, RateLimit, ResultMsg, UserMsg } from "./protocol.ts";
import { ClaudeUsage, type UsageSource } from "./usage.ts";

export type { UsageSource } from "./usage.ts";

type Event = ProviderEvent;

/** `system` subtypes that occur on every start and turn and carry nothing the record needs. */
// Verified on 2.1.321 (7D Task 0): `thinking_tokens {estimated_tokens, estimated_tokens_delta}`
// comes with every thinking block (s2a, s2c, s3A, s3C, s3D); its estimate is not usage.
const SILENT_SYSTEM: ReadonlySet<string> = new Set(["status", "ui_invalidate", "permission_denied", "thinking_tokens"]);

/**
 * Verified on 2.1.321 (7D Task 0, s3E): the `system` lines of a background task, which starts a
 * turn of its own after a `result`; `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` keeps them away.
 */
const BACKGROUND_SYSTEM: ReadonlySet<string> = new Set(["task_started", "task_updated", "task_notification", "background_tasks_changed"]);

/** The input fields that name a refused call's file. */
const PATH_KEYS = ["file_path", "notebook_path", "path"] as const;

/** One `result.permission_denials` entry, as far as it is readable. */
export interface Denial {
  readonly tool_name: string;
  readonly tool_use_id?: string;
  readonly tool_input?: Readonly<Record<string, unknown>>;
}

/** What the mapper asks the instance's permission prompts (`RequestPort`). */
export interface PromptMemory {
  /** Whether a prompt of this turn named the call (else its denial is the CLI's own). */
  asked(d: Denial): boolean;
  /** A `result` ended the turn: the turn's prompts are forgotten; the events of the prompts it withdrew. */
  turnEnded(): readonly Event[] | void;
}

const NO_PROMPTS: PromptMemory = { asked: () => false, turnEnded: () => {} };

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** A readable entry (a string `tool_name`; the other fields kept when well typed), or null. */
function denialOf(v: unknown): Denial | null {
  if (!isRecord(v) || typeof v.tool_name !== "string") return null;
  return { tool_name: v.tool_name, ...(typeof v.tool_use_id === "string" ? { tool_use_id: v.tool_use_id } : {}), ...(isRecord(v.tool_input) ? { tool_input: v.tool_input } : {}) };
}

/** The "Not logged in" turn's `result` text (Task 0, 2.1.321: "Not logged in · Please run /login"). */
const NOT_LOGGED_IN = /^Not logged in\b/;

export const SIGNIN_FIX = "finisherd provider signin claudeCode";

/** The runtime's stop reasons for the API's. */
const STOP_REASONS: Readonly<Record<string, string>> = { end_turn: "endTurn", max_tokens: "maxTokens", stop_sequence: "stopSequence", tool_use: "toolUse", refusal: "refusal", pause_turn: "pauseTurn" };

/** A retried status as the built-in agent's retry notice names it (`builtin/transport.ts`), so one shape reaches consumers. */
export function retryCode(status: number | null): string {
  if (status === 401 || status === 403) return "PROVIDER_AUTH";
  if (status === 429) return "PROVIDER_RATE_LIMITED";
  if (status === null || status === 408 || status >= 500) return "PROVIDER_OVERLOADED";
  return "PROVIDER_UNAVAILABLE";
}

const text200 = (s: string | undefined): string => (s ?? "").slice(0, 200);

/**
 * A failed turn's error (Decision 14): 401/403 and "Not logged in" → `PROVIDER_AUTH` (the latter
 * with the sign-in command as its fix), 429 → `PROVIDER_RATE_LIMITED`, 529 →
 * `PROVIDER_OVERLOADED`, the money backstop → `BUDGET_EXCEEDED`, a turn Finisher interrupted →
 * `SESSION_INTERRUPTED`; anything else is the CLI's own error, `PROVIDER_UNAVAILABLE`.
 */
/**
 * Whether `r` is the end of a turn Finisher's interrupt stopped. Verified on 2.1.321 (7D Task 0,
 * s1b, s2b): `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason:
 * "aborted_tools"`, no `result` key, `stop_reason: "tool_use"`. Only while Finisher interrupts
 * (review of part B): an abort it did not ask for fails the turn (fail closed: a held steer
 * then starts no run of its own). TODO(7D Task 0 Step 1): an interrupt while the model streams
 * with no tool running (another `aborted_*` reason?); modelled by fake-claude as
 * `error_during_execution` with no `terminal_reason`, which counts too.
 */
export function isInterruptEnd(r: ResultMsg, interrupting: boolean): boolean {
  if (!interrupting || !r.is_error) return false;
  if (r.terminal_reason?.startsWith("aborted") === true) return true;
  return r.subtype === "error_during_execution" && r.terminal_reason === undefined;
}

export function resultError(r: ResultMsg, interrupting: boolean): FinError {
  const status = r.api_error_status ?? null;
  const details = { harness: "claudeCode", subtype: r.subtype.slice(0, 64), ...(status === null ? {} : { status }), ...(r.terminal_reason === undefined ? {} : { terminalReason: r.terminal_reason.slice(0, 64) }) };
  if (NOT_LOGGED_IN.test(r.result ?? "")) return finError("PROVIDER_AUTH", "Claude Code is not signed in on this server", { ...details, reason: "notLoggedIn" }, SIGNIN_FIX);
  if (status === 401 || status === 403) return finError("PROVIDER_AUTH", "Claude Code's sign-in was refused", details, SIGNIN_FIX);
  if (status === 429) return finError("PROVIDER_RATE_LIMITED", "Claude Code is rate limited", details);
  if (status === 529) return finError("PROVIDER_OVERLOADED", "Claude Code's model is overloaded", details);
  // Verified on 2.1.321 (7D Task 0, s1f): the `--max-budget-usd` stop is `subtype:
  // "error_max_budget_usd"`, `terminal_reason: "budget_exhausted"`, `is_error: true`, no `result`.
  if (r.subtype === "error_max_budget_usd" || r.terminal_reason === "budget_exhausted") return finError("BUDGET_EXCEEDED", "the session's money budget is used up", { ...details, kind: "money" });
  // An interrupted turn ends as `interrupted` (`LineMapper`), never here; an error the CLI
  // reported while an interrupt was in flight is still named as the interrupt's.
  if (interrupting) return finError("SESSION_INTERRUPTED", "the Claude Code turn was interrupted", details);
  return finError("PROVIDER_UNAVAILABLE", `Claude Code failed the turn: ${text200(r.result ?? r.errors?.[0])}`, { ...details, reason: "turnError" });
}

/** What one instance's lines become, with the memory the mapping needs. */
export class LineMapper {
  readonly #usage: UsageSource;
  readonly #prompts: PromptMemory;
  readonly #noticed = new Set<string>();

  /** `prompts`: what this instance's permission prompts named (`RequestPort.asked`). */
  constructor(usage: UsageSource = new ClaudeUsage(), prompts: PromptMemory = NO_PROMPTS) {
    this.#usage = usage;
    this.#prompts = prompts;
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
      case "stream": {
        if (line.parent_tool_use_id != null) return [];
        const live = this.#usage.stream(line.event);
        if (live !== null) return [live];
        if (line.event.type === "content_block_delta") {
          const d = line.event.delta;
          if (d.type === "text_delta") return d.text === "" ? [] : [{ type: "messageDelta", text: d.text }];
          if (d.type === "thinking_delta") return d.thinking === "" ? [] : [{ type: "reasoningDelta", text: d.thinking }];
          if (d.type === "unknown") return this.#unknown(`stream_event/${d.name}`);
          return [];
        }
        if (line.event.type === "unknown") return this.#unknown(`stream_event/${line.event.name}`);
        return [];
      }
      case "assistant":
        return this.#assistant(line);
      case "user":
        return this.#user(line);
      case "result":
        return this.#result(line, interrupting);
      case "rateLimit":
        return this.#rateLimit(line);
      case "apiRetry":
        return [{ type: "notice", name: "retry", details: { attempt: line.attempt, code: retryCode(line.error_status ?? null), delayMs: line.retry_delay_ms } }];
      case "compactBoundary":
        return [{ type: "notice", name: "compacted", details: { trigger: line.compact_metadata?.trigger ?? null, preTokens: line.compact_metadata?.pre_tokens ?? null } }];
      case "system":
        if (SILENT_SYSTEM.has(line.subtype)) return [];
        if (BACKGROUND_SYSTEM.has(line.subtype)) return [{ type: "notice", name: "unsolicitedTurn", details: { harness: "claudeCode", line: `system/${line.subtype}` } }];
        return this.#unknown(`system/${line.subtype}`);
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

  #rateLimit(l: RateLimit): Event[] {
    const i = l.rate_limit_info;
    if (i.status.startsWith("allowed")) return [];
    return [{ type: "notice", name: "rateLimit", details: { harness: "claudeCode", status: i.status.slice(0, 64), window: i.rateLimitType?.slice(0, 64) ?? null, resetsAt: i.resetsAt ?? null, utilization: i.utilization ?? null } }];
  }

  /**
   * A `cliDenied` notice per refused call no permission prompt named (Decision 18, Task 0); an
   * unreadable entry is skipped, counted in one `malformedDenials` notice. The turn's prompts are
   * forgotten afterwards.
   */
  #cliDenied(r: ResultMsg): Event[] {
    const out: Event[] = [];
    let malformed = 0;
    for (const raw of r.permission_denials ?? []) {
      const d = denialOf(raw);
      if (d === null) {
        malformed++;
        continue;
      }
      if (this.#prompts.asked(d)) continue;
      const path = PATH_KEYS.map((k) => d.tool_input?.[k]).find((v): v is string => typeof v === "string");
      out.push({ type: "notice", name: "cliDenied", details: { harness: "claudeCode", tool: d.tool_name.slice(0, 64), toolUseId: d.tool_use_id?.slice(0, 128) ?? null, ...(path === undefined ? {} : { path: path.slice(0, 1024) }) } });
    }
    if (malformed > 0) out.push({ type: "notice", name: "malformedDenials", details: { harness: "claudeCode", count: malformed } });
    out.push(...(this.#prompts.turnEnded() ?? []));
    return out;
  }

  #result(r: ResultMsg, interrupting: boolean): Event[] {
    const out: Event[] = [];
    out.push(...this.#usage.usage(r));
    out.push(...this.#cliDenied(r));
    if (isInterruptEnd(r, interrupting)) {
      // The interrupt's own end (Decision 14): the run is stopped, not failed.
      out.push({ type: "turnCompleted", stopReason: "interrupted" });
      return out;
    }
    if (r.is_error) {
      const e = resultError(r, interrupting);
      out.push({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details, ...(e.fix === undefined ? {} : { fix: e.fix }) } });
      return out;
    }
    // Verified on 2.1.321 (7D Task 0, s1b/s1d): `queued_turn_count` is 0 even with a steer
    // queued, so it decides nothing; the session holds steers and withholds this end itself.
    const stop = r.stop_reason ?? "end_turn";
    out.push({ type: "turnCompleted", stopReason: STOP_REASONS[stop] ?? stop.slice(0, 64) });
    return out;
  }
}
