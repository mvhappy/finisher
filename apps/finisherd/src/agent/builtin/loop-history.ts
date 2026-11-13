// The built-in loop's conversation as the model sees it (plan 7A Decision 10; spec §3.4): a tool
// result cut to fit, and the conversation rebuilt from a session's stored nodes when the session
// is opened again (a resume after an interrupt or a restart), so no tool runs twice: a call that
// never got its result has the synthetic one recovery wrote.
//
// The rebuild matches the live conversation byte for byte (review M7): a `usage` node starts a
// model call (the loop emits a call's `usage` before its content), so the call's reasoning,
// text and tool calls form one assistant message, followed by its results in call order, each
// rendered with the same limits as live. Calls the loop answered itself come from its
// `localToolResult` notices (at their place in the reply), and steers enter where its
// `steersTaken` notices say. (Not rebuilt: a turn's facts, which are not stored, and the exact
// text of an interrupted call's result, which recovery writes.) The nodes of a rolled-back run
// (`rolledBack`, 7B Task 8) are left out: the conversation is as it was at the end of the run
// the session was rolled back to. A `summary` node (7B Decision 24) replaces every message of
// the runs before its own with the summary's message, as the loop did live; before it the
// conversation is whole. A rollback to a run the summary covers rolls its run back too, so the
// covered runs come back whole. A run's first `userMessage` starts it (its turn's text, whatever
// its trigger: a prompt, an idle steer, held steers); later ones in the run are steers. Hooks'
// context (7F Decision 16) is joined back: the `hookContext` notices of a run before its first
// `userMessage`, as the block after its text, and a steer of context (`trigger: "hook"`) as a steer.
import type { Node } from "@finisher/contracts";
import { hookContextBlock } from "../hook-context.ts";
import { summaryMessage } from "./compact.ts";
import type { ModelMessage, ToolCall } from "./request.ts";

export interface CutLimits {
  readonly resultBytes: number;
  readonly headBytes: number;
  readonly tailBytes: number;
}

/** What a call without a result is answered with, live and rebuilt alike. */
export const INTERRUPTED_RESULT = JSON.stringify({ ok: false, code: "SESSION_INTERRUPTED" });

/** A tool result as the model reads it: JSON, cut to head + note + tail past `resultBytes`. */
export function renderResult(providerId: string, a: { readonly ok: boolean; readonly output: unknown }, l: CutLimits): string {
  const body = a.ok ? a.output : { ok: false, ...(a.output !== null && typeof a.output === "object" && !Array.isArray(a.output) ? a.output : { output: a.output }) };
  const text = typeof body === "string" ? body : (JSON.stringify(body) ?? "null");
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= l.resultBytes) return text;
  const cut = bytes.length - l.headBytes - l.tailBytes;
  const head = bytes.subarray(0, l.headBytes).toString("utf8");
  const tail = bytes.subarray(bytes.length - l.tailBytes).toString("utf8");
  return `${head}\n[… ${cut} bytes cut here; the result is kept in this session as tool call ${providerId}'s result]\n${tail}`;
}

/** `output`, or — past `max` bytes of JSON — its first `max` bytes as text, marked (review M6). */
export function boundOutput(output: unknown, max: number): unknown {
  const text = typeof output === "string" ? output : (JSON.stringify(output) ?? "null");
  const bytes = Buffer.byteLength(text, "utf8");
  if (bytes <= max) return output;
  return { truncated: true, bytes, head: Buffer.from(text, "utf8").subarray(0, max).toString("utf8") };
}

type Slot = { readonly call: ToolCall; readonly index?: number; readonly content?: string };
interface Reply {
  /** The run whose model call this is. */
  readonly run: string;
  text?: string;
  readonly slots: Slot[];
  reasoning?: unknown[];
}

/** The conversation of a session's nodes (user prompts, replies, tool calls and results). */
export function historyFromNodes(nodes: ReadonlyArray<Node>, limits: CutLimits): ModelMessage[] {
  const out: ModelMessage[] = [];
  /** The run of each message of `out` (`""`: a summary, of no run). */
  const runs: string[] = [];
  const put = (m: ModelMessage, run: string) => {
    out.push(m);
    runs.push(run);
  };
  let run = "";
  const results = new Map<string, string>();
  const steers: string[] = [];
  /** `hookContext` texts of the run whose first message is still to come. */
  const context: { run: string; text: string }[] = [];
  /** Runs whose first `userMessage` (the turn's text) was read. */
  const opened = new Set<string>();
  let reply: Reply | null = null;
  let reasoning: unknown[] | undefined;
  let pending: Reply[] = [];
  const open = (): Reply => {
    if (reply === null) {
      reply = { run, slots: [], ...(reasoning === undefined ? {} : { reasoning }) };
      reasoning = undefined;
    }
    return reply;
  };
  /** Ends the model call being read; its results are written once they are all read. */
  const flush = () => {
    if (reply === null && reasoning !== undefined) open();
    if (reply !== null) pending.push(reply);
    reply = null;
  };
  /** Whether the model call being read has content already (a second message is another call). */
  const started = (): boolean => {
    const r = reply as Reply | null;
    return r !== null && (r.text !== undefined || r.slots.length > 0);
  };
  const write = () => {
    for (const r of pending) {
      // The calls in reply order: the locally answered at their index, the announced around them.
      const calls: Slot[] = [];
      const placed = r.slots.filter((s) => s.index !== undefined).sort((a, b) => a.index! - b.index!);
      const rest = r.slots.filter((s) => s.index === undefined);
      for (const p of placed) {
        while (calls.length < p.index! && rest.length > 0) calls.push(rest.shift()!);
        calls.push(p);
      }
      calls.push(...rest);
      if (r.text === undefined && calls.length === 0 && r.reasoning === undefined) continue;
      put({
        role: "assistant",
        ...(r.text === undefined ? {} : { text: r.text }),
        ...(calls.length === 0 ? {} : { toolCalls: calls.map((s) => s.call) }),
        ...(r.reasoning === undefined ? {} : { reasoning: r.reasoning }),
      }, r.run);
      for (const s of calls) put({ role: "tool", toolCallId: s.call.id, content: s.content ?? results.get(s.call.id) ?? INTERRUPTED_RESULT }, r.run);
    }
    pending = [];
  };
  for (const n of nodes) {
    if (n.rolledBack === true) continue;
    run = n.runId;
    const p = (n.payload ?? {}) as Record<string, unknown>;
    switch (n.kind) {
      case "userMessage": {
        const text = typeof p.text === "string" ? p.text : "";
        if (opened.has(run) && (p.trigger === "steer" || p.trigger === "hook")) {
          // A steer enters the conversation where the loop took it (`steersTaken`).
          steers.push(text);
          break;
        }
        flush();
        write();
        opened.add(run);
        const own = context.splice(0).filter((c) => c.run === run).map((c) => c.text);
        const suffix = own.length === 0 ? "" : `\n\n${hookContextBlock(own)}`;
        put({ role: "user", content: [{ type: "text", text: text + suffix }] }, run);
        break;
      }
      case "usage":
        flush();
        break;
      case "notice": {
        const hook = p.hookContext as { text?: unknown } | undefined;
        if (typeof hook?.text === "string") context.push({ run, text: hook.text });
        const taken = (p.steersTaken as { count?: unknown } | undefined)?.count;
        if (typeof taken === "number" && taken > 0) {
          flush();
          write();
          put({ role: "user", content: [{ type: "text", text: steers.splice(0, taken).join("\n\n") }] }, run);
        }
        const local = p.localToolResult as { providerId?: unknown; tool?: unknown; arguments?: unknown; index?: unknown; content?: unknown } | undefined;
        if (local && typeof local.providerId === "string" && typeof local.index === "number" && typeof local.content === "string") {
          const call = { id: local.providerId, name: typeof local.tool === "string" ? local.tool : "", arguments: typeof local.arguments === "string" ? local.arguments : "" };
          open().slots.push({ call, index: local.index, content: local.content });
        }
        break;
      }
      case "reasoning":
        if (started()) flush();
        if (Array.isArray(p.encrypted)) reasoning = p.encrypted;
        break;
      case "assistantMessage": {
        if (started()) flush();
        const blocks = Array.isArray(p.blocks) ? (p.blocks as Array<{ type?: string; text?: string }>) : [];
        open().text = blocks.filter((b) => b.type === "text").map((b) => b.text ?? "").join("");
        break;
      }
      case "toolCall":
        if (n.providerId === undefined) break;
        open().slots.push({ call: { id: n.providerId, name: typeof p.tool === "string" ? p.tool : "fin", arguments: p.input === undefined ? "{}" : (JSON.stringify(p.input) ?? "null") } });
        break;
      case "toolResult": {
        if (n.providerId === undefined) break;
        const body = n.payload === undefined ? `[the result is stored as ${n.blobRef ?? "a blob"}]` : renderResult(n.providerId, { ok: p.ok !== false, output: p.output }, limits);
        results.set(n.providerId, body);
        break;
      }
      case "summary": {
        if (typeof p.text !== "string") break;
        flush();
        write();
        const first = runs.indexOf(run);
        const cut = first === -1 ? out.length : first;
        out.splice(0, cut, summaryMessage(p.text));
        runs.splice(0, cut, "");
        break;
      }
      default:
        break;
    }
  }
  flush();
  write();
  return out;
}
