// The built-in loop's conversation as the model sees it (plan 7A Decision 10; spec §3.4): a tool
// result cut to fit, and the conversation rebuilt from a session's stored nodes when the session
// is opened again (a resume after an interrupt or a restart), so no tool runs twice: a call that
// never got its result has the synthetic one recovery wrote.
import type { Node } from "@finisher/contracts";
import type { ModelMessage, ToolCall } from "./request.ts";

export interface CutLimits {
  readonly resultBytes: number;
  readonly headBytes: number;
  readonly tailBytes: number;
}

/** A tool result as the model reads it: JSON, cut to head + note + tail past `resultBytes`. */
export function renderResult(providerId: string, a: { readonly ok: boolean; readonly output: unknown }, l: CutLimits): string {
  const body = a.ok ? a.output : { ok: false, ...(a.output !== null && typeof a.output === "object" && !Array.isArray(a.output) ? a.output : { output: a.output }) };
  const text = typeof body === "string" ? body : (JSON.stringify(body) ?? "null");
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= l.resultBytes) return text;
  const cut = bytes.length - l.headBytes - l.tailBytes;
  const head = bytes.subarray(0, l.headBytes).toString("utf8");
  const tail = bytes.subarray(bytes.length - l.tailBytes).toString("utf8");
  return `${head}\n[… ${cut} bytes cut here; the whole result is kept in this session as tool call ${providerId}'s result]\n${tail}`;
}

const asText = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v ?? null));

/** The conversation of a session's nodes (user prompts, replies, tool calls and results). */
export function historyFromNodes(nodes: ReadonlyArray<Node>): ModelMessage[] {
  const out: ModelMessage[] = [];
  let reply: { text?: string; toolCalls: ToolCall[]; reasoning?: unknown[] } | null = null;
  let reasoning: unknown[] | undefined;
  const answered = new Set<string>();
  const flush = () => {
    if (reply === null) return;
    out.push({
      role: "assistant",
      ...(reply.text === undefined ? {} : { text: reply.text }),
      ...(reply.toolCalls.length === 0 ? {} : { toolCalls: reply.toolCalls }),
      ...(reply.reasoning === undefined ? {} : { reasoning: reply.reasoning }),
    });
    reply = null;
  };
  const open = () => {
    if (reply === null) {
      reply = { toolCalls: [], ...(reasoning === undefined ? {} : { reasoning }) };
      reasoning = undefined;
    }
    return reply;
  };
  for (const n of nodes) {
    const p = (n.payload ?? {}) as Record<string, unknown>;
    switch (n.kind) {
      case "userMessage":
        flush();
        out.push({ role: "user", content: [{ type: "text", text: typeof p.text === "string" ? p.text : "" }] });
        break;
      case "reasoning":
        flush();
        if (Array.isArray(p.encrypted)) reasoning = p.encrypted;
        break;
      case "assistantMessage": {
        flush();
        const blocks = Array.isArray(p.blocks) ? (p.blocks as Array<{ type?: string; text?: string }>) : [];
        const text = blocks.filter((b) => b.type === "text").map((b) => b.text ?? "").join("");
        open().text = text;
        break;
      }
      case "toolCall":
        if (n.providerId === undefined) break;
        open().toolCalls.push({ id: n.providerId, name: typeof p.tool === "string" ? p.tool : "fin", arguments: JSON.stringify(p.input ?? {}) });
        break;
      case "toolResult": {
        flush();
        if (n.providerId === undefined) break;
        answered.add(n.providerId);
        const body = n.payload === undefined ? `[the result is stored as ${n.blobRef ?? "a blob"}]` : asText(p.ok === false ? { ok: false, ...((p.output ?? {}) as object) } : p.output);
        out.push({ role: "tool", toolCallId: n.providerId, content: body });
        break;
      }
      default:
        break;
    }
  }
  flush();
  // A call without a result (it should have a synthetic one) is answered so the request is valid.
  const fixed: ModelMessage[] = [];
  for (const m of out) {
    fixed.push(m);
    if (m.role !== "assistant" || m.toolCalls === undefined) continue;
    for (const c of m.toolCalls) {
      if (!answered.has(c.id)) fixed.push({ role: "tool", toolCallId: c.id, content: JSON.stringify({ ok: false, code: "SESSION_INTERRUPTED" }) });
    }
  }
  return fixed;
}
