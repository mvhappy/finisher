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
//   turn's trace-store delta over), and not streamed as a reply.
// - **Its result** is a `summary` event (a `summary` node `{prompt, text}`), emitted after the
//   call's `usage`. The node is the boundary a rebuild (`historyFromNodes`) honours: before it
//   the conversation is whole, so a crash mid-compaction resumes with the same history.
// - **A compaction that would not fit** (its own prompt past the stop ratio), or that fails (an
//   error, or no text), ends the run with `MODEL_CONTEXT_EXCEEDED`, as the stop does. A gate
//   refusal stays what it is.
import { readFileSync } from "node:fs";
import { FinError, type ProviderEvent } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { ModelResult } from "./compat.ts";
import type { ModelMessage, ModelRequest } from "./request.ts";

/** The prompt's version, kept on every `summary` node. */
export const COMPACTION_PROMPT_ID = "compaction-v1";
/** Finisher's own text; a change is a new version (a new file), never an edit. */
export const COMPACTION_PROMPT = readFileSync(new URL("./prompts/compaction-v1.md", import.meta.url), "utf8").trim();

const SUMMARY_HEAD = "[A summary of this session's earlier runs, which you wrote when the conversation was compacted.]";

/** The user-role message that stands for the runs a summary covers, live and rebuilt alike. */
export function summaryMessage(text: string): ModelMessage {
  return { role: "user", content: [{ type: "text", text: `${SUMMARY_HEAD}\n\n${text}` }] };
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
  /** The messages of the runs before the current one. */
  readonly older: readonly ModelMessage[];
  /** The prompt tokens of a request (the loop's estimate). */
  readonly tokens: (req: ModelRequest) => number;
  /** The most prompt tokens a call may have (the stop ratio of the window). */
  readonly fits: number;
  readonly window: number;
  /** One quiet model call (gated, charged, with retries); throws as the loop's does. */
  readonly call: (req: ModelRequest) => Promise<{ result: ModelResult } | { refused: FinError }>;
  readonly emit: (event: ProviderEvent) => void;
  readonly aborted: () => boolean;
}

export type Compacted = { readonly message: ModelMessage } | { readonly refused: FinError } | { readonly failed: FinError };

const exceeded = (reason: "compactionTooLarge" | "compactionFailed", details: Record<string, unknown>) =>
  finError("MODEL_CONTEXT_EXCEEDED", reason === "compactionTooLarge" ? "the conversation is too long even to summarise" : "the conversation could not be summarised", { reason, ...details }, "start a new session");

/** Runs one compaction: the summary's message, a gate refusal, or the failure that ends the run. */
export async function compact(c: Compaction): Promise<Compacted> {
  const req = compactionRequest(c.next, c.older);
  const tokens = c.tokens(req);
  if (tokens > c.fits) return { failed: exceeded("compactionTooLarge", { tokens, window: c.window }) };
  let answer: Awaited<ReturnType<Compaction["call"]>>;
  try {
    answer = await c.call(req);
  } catch (e) {
    if (c.aborted()) throw e;
    const code = e instanceof FinError ? e.code : (e as { code?: unknown } | null)?.code;
    return { failed: exceeded("compactionFailed", { window: c.window, ...(typeof code === "string" ? { cause: code } : {}) }) };
  }
  if ("refused" in answer) return answer;
  const r = answer.result;
  // The journal's order, as for any call: the capture, then the usage it joins.
  c.emit(captureOf(r, "compaction"));
  c.emit({ type: "usage", ...r.usage, ...(r.costMicros === undefined ? {} : { costMicros: r.costMicros }) });
  const text = r.text.trim();
  if (text === "") return { failed: exceeded("compactionFailed", { window: c.window, cause: "emptySummary" }) };
  c.emit({ type: "summary", prompt: COMPACTION_PROMPT_ID, text });
  return { message: summaryMessage(text) };
}
