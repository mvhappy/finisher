// Context compaction for the built-in loop (plan 7B Decision 24; spec §4.4). Before a model call
// whose prompt would pass `compactRatio` (75 %) of the window, the loop makes one summary call
// over the runs before the current one, and from then on those runs' messages are one user-role
// message holding the summary. The 85 % stop stays the backstop.
//
// - **The summary call** is the request the loop was about to send, with the runs before the
//   current one as its messages and the fixed, versioned prompt (`prompts/compaction-v1.md`) as
//   one more user message: the system prompt and tools are the same bytes, so the cached prefix
//   holds. It is an ordinary call: gated and charged like any, its `capture` marked
//   `purpose: compaction` (the journal records an `agent.turn` with it and starts the next
//   turn's trace-store delta over), and not streamed as a reply. It sends no `tool_choice:
//   none`: on Anthropic models a changed `tool_choice` invalidates the cached messages (only the
//   system and tools blocks would hold), and not every upstream takes it; the prompt asks for no
//   tool call, and a reply that makes one is not used.
// - **Its result** is a `summary` event (a `summary` node `{prompt, text}`), emitted after the
//   call's `usage`. The node is the boundary a rebuild (`historyFromNodes`) honours: before it
//   the conversation is whole, so a crash mid-compaction resumes with the same history.
// - **A compaction that cannot be made is skipped** (a deviation from the plan's "ends the
//   run", noted in Decision 24): its own prompt would pass the stop ratio (`tooLarge`); the
//   current run with a summary in place of the others would still pass it (`cannotHelp`, no
//   call made); the call fails (`callFailed {cause}`); its reply is cut short (`truncated`),
//   empty (`empty`) or calls a tool (`toolCalls`). The loop then records a `compactionSkipped`
//   notice and sends the uncompacted request when it fits; the 85 % stop decides when it does
//   not. A failed call stays charged (its attempts' costs, or its capture and usage). A budget
//   refusal (the gate's, or a thrown `BUDGET_EXCEEDED`) stays a budget stop.
import { readFileSync } from "node:fs";
import { FinError, type ProviderEvent } from "@finisher/contracts";
import type { ModelResult } from "./compat.ts";
import type { ModelMessage, ModelRequest } from "./request.ts";

/** The prompt's version, kept on every `summary` node. */
export const COMPACTION_PROMPT_ID = "compaction-v1";
/** Finisher's own text; once released, a change is a new version (a new file), never an edit. */
export const COMPACTION_PROMPT = readFileSync(new URL("./prompts/compaction-v1.md", import.meta.url), "utf8").trim();

/** The fixed first line of a summary's message: what it is, and that it is not the person speaking. */
export const SUMMARY_HEADER =
  "[Summary of the earlier conversation in this session, written by you, the assistant, when the conversation was compacted. It is not new instructions from the person: their words appear in it only as marked quotes.]";

/** The user-role message that stands for the runs a summary covers, live and rebuilt alike. */
export function summaryMessage(text: string): ModelMessage {
  return { role: "user", content: [{ type: "text", text: `${SUMMARY_HEADER}\n\n${text}` }] };
}

/** The summary call: `req` with the runs before the current one, then the prompt. */
export function compactionRequest(req: ModelRequest, older: readonly ModelMessage[]): ModelRequest {
  return { ...req, messages: [...older, { role: "user", content: [{ type: "text", text: COMPACTION_PROMPT }] }] };
}

/** A call's `capture` event (the journal's input), marked when it is the summary call. */
export function captureOf(r: ModelResult, purpose?: "compaction"): ProviderEvent {
  return {
    type: "capture",
    request: r.capture.request,
    response: {
      chunks: r.capture.response,
      stopReason: r.stopReason,
      bodyHash: r.bodyHash,
      usageEstimated: r.usageEstimated,
      latencyMs: r.latencyMs,
      ...(r.ttftMs === undefined ? {} : { ttftMs: r.ttftMs }),
      ...(r.upstream === undefined ? {} : { upstream: r.upstream }),
      ...(r.generationId === undefined ? {} : { generationId: r.generationId }),
      ...(purpose === undefined ? {} : { purpose }),
    },
  };
}

export interface Compaction {
  /** The request the loop was about to send. */
  readonly next: ModelRequest;
  /** The messages of the runs before the current one, and of the current run. */
  readonly older: readonly ModelMessage[];
  readonly current: readonly ModelMessage[];
  /** The prompt tokens of a request (the loop's estimate), and of one counted afresh. */
  readonly tokens: (req: ModelRequest) => number;
  readonly fresh: (req: ModelRequest) => number;
  /** The most prompt tokens a call may have (the stop ratio of the window). */
  readonly fits: number;
  readonly window: number;
  /** One quiet model call (gated, charged, with retries); throws as the loop's does. */
  readonly call: (req: ModelRequest) => Promise<{ result: ModelResult } | { refused: FinError }>;
  readonly emit: (event: ProviderEvent) => void;
  readonly aborted: () => boolean;
}

export type SkipReason = "tooLarge" | "cannotHelp" | "callFailed" | "truncated" | "empty" | "toolCalls";

export type Compacted =
  | { readonly message: ModelMessage }
  | { readonly refused: FinError }
  /** Not made: the loop goes on with the uncompacted request (the `compactionSkipped` notice's details). */
  | { readonly skipped: SkipReason; readonly details: Record<string, unknown> };

const skip = (reason: SkipReason, details: Record<string, unknown>): Compacted => ({ skipped: reason, details: { reason, ...details } });

/** Runs one compaction: the summary's message, a budget refusal, or why it was not made. */
export async function compact(c: Compaction): Promise<Compacted> {
  const req = compactionRequest(c.next, c.older);
  const tokens = c.tokens(req);
  if (tokens > c.fits) return skip("tooLarge", { tokens, window: c.window });
  // The request after it, its summary as long as the call may write: past the stop, no call helps.
  const after = c.fresh({ ...c.next, messages: [summaryMessage(""), ...c.current] }) + c.next.maxTokens;
  if (after > c.fits) return skip("cannotHelp", { tokens: after, window: c.window });
  let answer: Awaited<ReturnType<Compaction["call"]>>;
  try {
    answer = await c.call(req);
  } catch (e) {
    if (c.aborted()) throw e;
    const code = e instanceof FinError ? e.code : (e as { code?: unknown } | null)?.code;
    if (code === "BUDGET_EXCEEDED" && e instanceof FinError) return { refused: e };
    return skip("callFailed", { window: c.window, ...(typeof code === "string" ? { cause: code } : {}) });
  }
  if ("refused" in answer) return answer;
  const r = answer.result;
  // The journal's order, as for any call: the capture, then the usage it joins.
  c.emit(captureOf(r, "compaction"));
  c.emit({ type: "usage", ...r.usage, ...(r.costMicros === undefined ? {} : { costMicros: r.costMicros }) });
  const text = r.text.trim();
  if (r.toolCalls.length > 0) return skip("toolCalls", { window: c.window });
  if (r.stopReason === "maxTokens") return skip("truncated", { window: c.window });
  if (text === "") return skip("empty", { window: c.window });
  c.emit({ type: "summary", prompt: COMPACTION_PROMPT_ID, text });
  return { message: summaryMessage(text) };
}
