// The model request in Finisher's own neutral form, and its Chat Completions body (plan 7A
// Decisions 7 and 8). Finisher builds every body itself; the SDK only carries it. One
// OpenAI-compatible API, two presets: `openrouter` (the user's key; reasoning and explicit cache
// breakpoints where the catalogue says) and `local` (Ollama, LM Studio or another server the
// user runs; no key, plain Chat Completions).
import type { Caching } from "../catalogue/catalogue.ts";

export type Preset = "openrouter" | "local";

export type InputBlock = { type: "text"; text: string } | { type: "image"; mediaType: string; data: string };

export interface ToolCall {
  readonly id: string;
  readonly name: string;
  /** The arguments as the model wrote them (JSON text). */
  readonly arguments: string;
}

export type ModelMessage =
  | { readonly role: "user"; readonly content: readonly InputBlock[] }
  | {
      readonly role: "assistant";
      readonly text?: string;
      readonly toolCalls?: readonly ToolCall[];
      /** The provider's reasoning details, opaque, sent back unchanged. */
      readonly reasoning?: readonly unknown[];
    }
  | { readonly role: "tool"; readonly toolCallId: string; readonly content: string };

export interface ToolDef {
  readonly name: string;
  readonly description: string;
  /** JSON Schema of the arguments. */
  readonly parameters: Record<string, unknown>;
}

export interface ReasoningSpec {
  readonly effort?: "minimal" | "low" | "medium" | "high";
  readonly maxTokens?: number;
}

/** One model call, neutral. `model` is the wire id (`Catalogue.wireModel`). */
export interface ModelRequest {
  readonly system: string;
  readonly tools: readonly ToolDef[];
  readonly messages: readonly ModelMessage[];
  readonly model: string;
  readonly maxTokens: number;
  readonly temperature?: number;
  readonly reasoning?: ReasoningSpec;
}

export interface BodyOptions {
  readonly preset: Preset;
  /** The model's caching class (the catalogue's); `explicit` adds `cache_control` breakpoints. */
  readonly caching: Caching;
}

const EPHEMERAL = { type: "ephemeral" } as const;

type Part = Record<string, unknown>;

function parts(content: readonly InputBlock[]): Part[] {
  return content.map((b) =>
    b.type === "text" ? { type: "text", text: b.text } : { type: "image_url", image_url: { url: `data:${b.mediaType};base64,${b.data}` } },
  );
}

/** A message whose content is one string unless it must be parts (images, a breakpoint). */
function wireMessage(m: ModelMessage, breakpoint: boolean): Record<string, unknown> {
  switch (m.role) {
    case "user": {
      const ps = parts(m.content);
      if (breakpoint && ps.length > 0) ps[ps.length - 1] = { ...ps[ps.length - 1], cache_control: EPHEMERAL };
      const plain = !breakpoint && ps.every((p) => p.type === "text") && ps.length === 1;
      return { role: "user", content: plain ? (ps[0]!.text as string) : ps };
    }
    case "tool":
      return {
        role: "tool",
        tool_call_id: m.toolCallId,
        content: breakpoint ? [{ type: "text", text: m.content, cache_control: EPHEMERAL }] : m.content,
      };
    case "assistant": {
      const out: Record<string, unknown> = { role: "assistant", content: m.text ?? null };
      if (m.toolCalls && m.toolCalls.length > 0) {
        out.tool_calls = m.toolCalls.map((c) => ({ id: c.id, type: "function", function: { name: c.name, arguments: c.arguments } }));
      }
      if (m.reasoning && m.reasoning.length > 0) out.reasoning_details = m.reasoning;
      return out;
    }
  }
}

/**
 * The Chat Completions body for `req`, keys in a fixed order (so the same request always
 * serialises to the same bytes). For `caching: explicit` on OpenRouter: a breakpoint on the
 * system block (which also covers the tools, which precede it in the cached prefix) and one on
 * the last message, the most stable prefix the next call will repeat; two of the four allowed.
 */
export function chatBody(req: ModelRequest, opts: BodyOptions): Record<string, unknown> {
  const explicit = opts.preset === "openrouter" && opts.caching === "explicit";
  const system = explicit ? { role: "system", content: [{ type: "text", text: req.system, cache_control: EPHEMERAL }] } : { role: "system", content: req.system };
  const last = req.messages.length - 1;
  const messages = [system, ...req.messages.map((m, i) => wireMessage(m, explicit && i === last && m.role !== "assistant"))];
  const body: Record<string, unknown> = { model: req.model, messages };
  if (req.tools.length > 0) {
    body.tools = req.tools.map((t) => ({ type: "function", function: { name: t.name, description: t.description, parameters: t.parameters } }));
  }
  body.max_tokens = req.maxTokens;
  if (req.temperature !== undefined) body.temperature = req.temperature;
  if (opts.preset === "openrouter" && req.reasoning) {
    body.reasoning = req.reasoning.maxTokens !== undefined ? { max_tokens: req.reasoning.maxTokens } : { effort: req.reasoning.effort ?? "medium" };
  }
  body.stream = true;
  // Usage on the last chunk: OpenRouter always sends it (this is a no-op there); local servers
  // need it.
  body.stream_options = { include_usage: true };
  return body;
}
