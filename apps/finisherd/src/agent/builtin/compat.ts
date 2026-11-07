// The built-in agent's model client (plan 7A Decisions 7–9; spec §4.4 with F1): one
// OpenAI-compatible Chat Completions client, two presets. `openrouter` talks to
// `https://openrouter.ai/api/v1` only, with the user's key and OpenRouter's extensions
// (reasoning, cache breakpoints, opt-in routing, reported cost, the upstream provider);
// `local` (Ollama, LM Studio, a URL the owner set) sends a fixed placeholder bearer and nothing
// OpenRouter-specific. The `openai` SDK is transport and SSE parsing only: Finisher builds the
// body (`request.ts`), the bytes the SDK sends are captured as they leave, and `bodyHash` is
// their BLAKE3 (`blake3:<hex>`, fin-node), which the journal keys when it records the turn.
//
// The client owns retries (the SDK's are off) and never runs a tool. Every attempt that fails
// reports what it may have cost (review B2): usage when the stream reported it, else an
// estimate from what it streamed, and the generation id for `settleGeneration`. An attempt
// that already streamed more than a little output is not retried (its partial output was
// shown and charged); it fails.
import { blake3Hex } from "@finisher/fin-node";
import OpenAI from "openai";
import { finError } from "../../errors.ts";
import { type Caching, type Usage, ZERO_USAGE } from "../catalogue/catalogue.ts";
import type { Clock } from "../clock.ts";
import { systemClock } from "../clock.ts";
import { type BodyOptions, chatBody, type ModelRequest, type Preset, type Routing, type ToolCall } from "./request.ts";
import { abortableSleep, checkBaseUrl, classify, OPENROUTER_BASE_URL } from "./transport.ts";

export type { ModelRequest, Preset };
export { checkBaseUrl, OPENROUTER_BASE_URL, retryAfterMs } from "./transport.ts";

/** Calls `f` with the key for one request (Task 5's gateway in 7A-2; a constant in tests). */
export type KeySource = <A>(f: (key: string) => Promise<A>) => Promise<A>;

/** The local preset's key source: there is no key (the client sends a fixed placeholder). */
export const noKey: KeySource = (f) => f("");

/** The bearer value a local server receives (spec §4.4: a fixed placeholder, no secret). */
export const LOCAL_BEARER = "finisher-local";

export type StopReason = "endTurn" | "toolUse" | "maxTokens" | "stopSequence" | "refusal" | "other";

/** What one failed attempt may have cost. */
export interface AttemptCost {
  readonly generationId?: string;
  /** null: nothing streamed and nothing reported (nothing to charge but `settleGeneration`). */
  readonly usage: Usage | null;
  /** The usage is an estimate (characters ÷ 4), not the provider's. */
  readonly usageEstimated: boolean;
  /** What the provider reported it charged, in micros. */
  readonly costMicros?: number;
}

export interface ModelResult {
  readonly text: string;
  readonly toolCalls: ToolCall[];
  /** Reasoning text as streamed, for display. */
  readonly reasoningText: string;
  /** The provider's reasoning details, rebuilt from the deltas; sent back unchanged next call. */
  readonly reasoning: unknown[];
  readonly stopReason: StopReason;
  readonly usage: Usage;
  /** The server reported no usage: tokens are estimated (characters ÷ 4). */
  readonly usageEstimated: boolean;
  /** What the provider charged, in micros (OpenRouter's usage accounting). */
  readonly costMicros?: number;
  /** The provider that served the call, as OpenRouter names it (the journal normalises it). */
  readonly upstream?: string;
  readonly generationId?: string;
  /** The exact request body as sent, and the response chunks as received. */
  readonly capture: { readonly request: string; readonly response: unknown[] };
  /** `blake3:<hex>` of the request body bytes, unkeyed. */
  readonly bodyHash: string;
  readonly latencyMs: number;
  readonly ttftMs?: number;
}

export type ModelStreamEvent =
  | { readonly type: "textDelta"; readonly text: string }
  | { readonly type: "reasoningDelta"; readonly text: string }
  /** The attempt failed and the same body is sent again after `delayMs`: discard its output, charge `cost`. */
  | { readonly type: "retry"; readonly attempt: number; readonly code: string; readonly delayMs: number; readonly cost: AttemptCost }
  /** The last attempt failed (the stream then throws `code`): charge `cost`. */
  | { readonly type: "failed"; readonly attempt: number; readonly code: string; readonly cost: AttemptCost }
  | { readonly type: "done"; readonly result: ModelResult };

export interface ModelClient {
  readonly preset: Preset;
  /** The URL warnings for the owner (a local server over plain http on the network). */
  readonly warnings: readonly string[];
  stream(req: ModelRequest, key: KeySource, signal?: AbortSignal): AsyncIterable<ModelStreamEvent>;
  /** The body that is sent for `req`, for capture. */
  readonly wireBody: (req: ModelRequest) => Record<string, unknown>;
}

export interface ClientOptions {
  readonly preset: Preset;
  /** The local preset's base URL (the owner's `agents.local.baseUrl`). Ignored for openrouter. */
  readonly baseUrl?: string;
  /** Tests only: an `http` loopback base URL for the openrouter preset (the fake model server). */
  readonly testBaseUrl?: string;
  /** The model's caching class (the catalogue's); default `none`. */
  readonly cachingFor?: (model: string) => Caching;
  /** OpenRouter routing per model (settings); absent: every upstream allowed. */
  readonly routingFor?: (model: string) => Routing | undefined;
  readonly clock?: Clock;
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
  /** Attempts per call, the first included (Decision 9: 5). */
  readonly maxAttempts?: number;
  /** Per-request total timeout in ms (default 10 min). */
  readonly timeoutMs?: number;
  /** The longest gap between two chunks once the stream has started, in ms (default 60 s). */
  readonly idleMs?: number;
  /** An attempt that streamed more characters than this is not retried (default 512). */
  readonly retryStreamedChars?: number;
  /** The longest `retry-after` honoured; a longer one fails with the code (default 10 min). */
  readonly maxRetryAfterMs?: number;
}

const MAX_BACKOFF_MS = 60_000;

const STOP: Record<string, StopReason> = {
  stop: "endTurn",
  tool_calls: "toolUse",
  function_call: "toolUse",
  length: "maxTokens",
  content_filter: "refusal",
};

const estimate = (chars: number) => Math.ceil(chars / 4);

/** Accumulates one attempt's chunks into a result. */
class Assembly {
  text = "";
  reasoningText = "";
  readonly reasoning: Record<string, unknown>[] = [];
  readonly calls: { id: string; name: string; arguments: string }[] = [];
  readonly chunks: unknown[] = [];
  finish: string | null = null;
  usage: Record<string, unknown> | null = null;
  upstream: string | undefined;
  id: string | undefined;
  firstAt: number | undefined;
  /** Characters of output streamed (text, reasoning, tool arguments). */
  streamed = 0;

  add(chunk: Record<string, unknown>, now: number, out: ModelStreamEvent[]): void {
    this.chunks.push(chunk);
    if (typeof chunk.id === "string") this.id ??= chunk.id;
    if (typeof chunk.provider === "string") this.upstream = chunk.provider;
    if (chunk.usage && typeof chunk.usage === "object") this.usage = chunk.usage as Record<string, unknown>;
    const choice = (Array.isArray(chunk.choices) ? chunk.choices[0] : undefined) as Record<string, unknown> | undefined;
    if (!choice) return;
    if (typeof choice.finish_reason === "string") this.finish = choice.finish_reason;
    const delta = (choice.delta ?? {}) as Record<string, unknown>;
    if (typeof delta.content === "string" && delta.content !== "") {
      this.firstAt ??= now;
      this.text += delta.content;
      this.streamed += delta.content.length;
      out.push({ type: "textDelta", text: delta.content });
    }
    const r = typeof delta.reasoning === "string" ? delta.reasoning : typeof delta.reasoning_content === "string" ? delta.reasoning_content : "";
    if (r !== "") {
      this.firstAt ??= now;
      this.reasoningText += r;
      this.streamed += r.length;
      out.push({ type: "reasoningDelta", text: r });
    }
    if (Array.isArray(delta.reasoning_details)) for (const d of delta.reasoning_details) this.addReasoning(d);
    if (Array.isArray(delta.tool_calls)) {
      this.firstAt ??= now;
      for (const raw of delta.tool_calls as Record<string, unknown>[]) {
        const i = typeof raw.index === "number" ? raw.index : this.calls.length;
        const call = (this.calls[i] ??= { id: "", name: "", arguments: "" });
        const fn = (raw.function ?? {}) as Record<string, unknown>;
        if (typeof raw.id === "string" && raw.id !== "") call.id = raw.id;
        if (typeof fn.name === "string") call.name += fn.name;
        if (typeof fn.arguments === "string") {
          call.arguments += fn.arguments;
          this.streamed += fn.arguments.length;
        }
      }
    }
  }

  /** Deltas of one detail block (same `type` and `index`) are joined; other fields kept. */
  private addReasoning(d: unknown): void {
    if (d === null || typeof d !== "object") return;
    const detail = d as Record<string, unknown>;
    const last = this.reasoning.at(-1);
    if (last && last.type === detail.type && last.index === detail.index) {
      for (const [k, v] of Object.entries(detail)) {
        if ((k === "text" || k === "summary" || k === "data") && typeof v === "string" && typeof last[k] === "string") last[k] = (last[k] as string) + v;
        else if (v !== null && v !== undefined) last[k] = v;
      }
    } else {
      this.reasoning.push({ ...detail });
    }
  }

  reportedUsage(): Usage | null {
    if (this.usage === null) return null;
    const u = this.usage;
    const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.round(v) : 0);
    const prompt = (u.prompt_tokens_details ?? {}) as Record<string, unknown>;
    const completion = (u.completion_tokens_details ?? {}) as Record<string, unknown>;
    const cacheRead = n(prompt.cached_tokens);
    const cacheWrite = n(prompt.cache_write_tokens);
    return {
      input: Math.max(0, n(u.prompt_tokens) - cacheRead - cacheWrite),
      output: n(u.completion_tokens),
      cacheRead,
      cacheWrite,
      reasoning: n(completion.reasoning_tokens),
    };
  }

  /** The usage reported, else estimated from the request bytes and what streamed; null when nothing streamed. */
  usageOrEstimate(requestBytes: number): { usage: Usage | null; estimated: boolean } {
    const reported = this.reportedUsage();
    if (reported) return { usage: reported, estimated: false };
    if (this.streamed === 0) return { usage: null, estimated: false };
    return { usage: { ...ZERO_USAGE, input: estimate(requestBytes), output: estimate(this.streamed) }, estimated: true };
  }

  costMicros(): number | undefined {
    const c = this.usage?.cost;
    return typeof c === "number" && Number.isFinite(c) && c >= 0 ? Math.round(c * 1_000_000) : undefined;
  }
}

/** Raised by the idle timer: no chunk for `idleMs`. */
class IdleTimeout extends Error {}

export function openAICompatibleClient(opts: ClientOptions): ModelClient {
  const raw = opts.preset === "openrouter" ? (opts.testBaseUrl ?? OPENROUTER_BASE_URL) : opts.baseUrl;
  if (raw === undefined) throw finError("PROVIDER_UNAVAILABLE", "the local preset needs a base URL", { preset: "local" }, "set it with `finisherd provider local set`");
  const { url, warnings } = checkBaseUrl(opts.preset, raw, opts.testBaseUrl !== undefined);
  const base = url.toString().replace(/\/+$/, "");
  const clock = opts.clock ?? systemClock;
  const sleep = opts.sleep ?? abortableSleep;
  const maxAttempts = opts.maxAttempts ?? 5;
  const idleMs = opts.idleMs ?? 60_000;
  const retryChars = opts.retryStreamedChars ?? 512;
  const maxRetryAfter = opts.maxRetryAfterMs ?? 600_000;
  const bodyOptions = (model: string): BodyOptions => {
    const routing = opts.routingFor?.(model);
    return { preset: opts.preset, caching: opts.cachingFor?.(model) ?? "none", ...(routing ? { routing } : {}) };
  };
  const wireBody = (req: ModelRequest) => chatBody(req, bodyOptions(req.model));

  /** One attempt: the SDK built per request, with the key, no retries, no logging, an idle timer. */
  async function* attempt(body: Record<string, unknown>, key: KeySource, sent: { text?: string }, ctl: AbortController) {
    const started = clock.now();
    const stream = await key(async (k) => {
      const client = new OpenAI({
        apiKey: opts.preset === "openrouter" ? k : LOCAL_BEARER,
        baseURL: base,
        maxRetries: 0,
        timeout: opts.timeoutMs ?? 600_000,
        logLevel: "off",
        fetch: async (u: string | URL | Request, init?: RequestInit) => {
          const headers = new Headers(init?.headers);
          for (const h of [...headers.keys()]) if (h.startsWith("x-stainless-")) headers.delete(h);
          if (typeof init?.body === "string") sent.text = init.body;
          return fetch(u, { ...init, headers });
        },
      });
      const params = body as unknown as OpenAI.Chat.Completions.ChatCompletionCreateParamsStreaming;
      return client.chat.completions.create(params, { signal: ctl.signal });
    });
    yield { started };
    let timer: NodeJS.Timeout | undefined;
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => ctl.abort(new IdleTimeout(`no data for ${idleMs} ms`)), idleMs);
    };
    try {
      for await (const chunk of stream) {
        arm();
        yield { chunk: chunk as unknown as Record<string, unknown> };
      }
    } finally {
      clearTimeout(timer);
    }
  }

  async function* streamCall(req: ModelRequest, key: KeySource, signal?: AbortSignal): AsyncIterable<ModelStreamEvent> {
    const body = wireBody(req);
    for (let n = 1; ; n++) {
      const sent: { text?: string } = {};
      const a = new Assembly();
      const ctl = new AbortController();
      const onAbort = () => ctl.abort(signal?.reason);
      signal?.addEventListener("abort", onAbort, { once: true });
      let started = clock.now();
      try {
        for await (const step of attempt(body, key, sent, ctl)) {
          if ("started" in step) {
            started = step.started;
            continue;
          }
          const out: ModelStreamEvent[] = [];
          a.add(step.chunk, clock.now(), out);
          yield* out;
        }
        if (ctl.signal.aborted && ctl.signal.reason instanceof IdleTimeout) throw ctl.signal.reason;
        if (a.finish === null) throw new TypeError("the stream ended before the model finished");
        if (a.finish === "error") throw finError("PROVIDER_OVERLOADED", "the model stream ended with an error", {});
      } catch (e) {
        signal?.removeEventListener("abort", onAbort);
        const bytes = Buffer.byteLength(sent.text ?? "", "utf8");
        const { usage, estimated } = a.usageOrEstimate(bytes);
        const reported = opts.preset === "openrouter" ? a.costMicros() : undefined;
        const cost: AttemptCost = {
          ...(a.id === undefined ? {} : { generationId: a.id }),
          usage,
          usageEstimated: estimated,
          ...(reported === undefined ? {} : { costMicros: reported }),
        };
        if (signal?.aborted) {
          yield { type: "failed", attempt: n, code: "SESSION_INTERRUPTED", cost };
          throw finError("SESSION_INTERRUPTED", "the model call was interrupted", {});
        }
        const idle = e instanceof IdleTimeout || (ctl.signal.aborted && ctl.signal.reason instanceof IdleTimeout);
        const f = idle ? { error: finError("PROVIDER_OVERLOADED", `the model stream stalled for ${idleMs} ms`, { idleMs }), retryable: true } : classify(e, clock.now());
        const delayMs = ("retryAfter" in f ? f.retryAfter : undefined) ?? Math.min(MAX_BACKOFF_MS, 1000 * 2 ** (n - 1));
        const retry = f.retryable && n < maxAttempts && a.streamed <= retryChars && delayMs <= maxRetryAfter;
        if (!retry) {
          yield { type: "failed", attempt: n, code: f.error.code, cost };
          throw f.error;
        }
        yield { type: "retry", attempt: n, code: f.error.code, delayMs, cost };
        await sleep(delayMs, signal);
        continue;
      }
      signal?.removeEventListener("abort", onAbort);
      const request = sent.text ?? JSON.stringify(body);
      const requestBytes = Buffer.byteLength(request, "utf8");
      const done = clock.now();
      const stopReason: StopReason = a.finish !== null ? (STOP[a.finish] ?? "other") : "other";
      const { usage, estimated } = a.usageOrEstimate(requestBytes);
      const costMicros = opts.preset === "openrouter" ? a.costMicros() : undefined;
      yield {
        type: "done",
        result: {
          text: a.text,
          toolCalls: a.calls.filter((c) => c !== undefined),
          reasoningText: a.reasoningText,
          reasoning: a.reasoning,
          stopReason,
          usage: usage ?? { ...ZERO_USAGE, input: estimate(requestBytes) },
          usageEstimated: usage === null || estimated,
          ...(costMicros === undefined ? {} : { costMicros }),
          ...(a.upstream === undefined ? {} : { upstream: a.upstream }),
          ...(a.id === undefined ? {} : { generationId: a.id }),
          capture: { request, response: a.chunks },
          bodyHash: `blake3:${blake3Hex(Buffer.from(request, "utf8"))}`,
          latencyMs: done - started,
          ...(a.firstAt === undefined ? {} : { ttftMs: a.firstAt - started }),
        },
      };
      return;
    }
  }

  return { preset: opts.preset, warnings, stream: streamCall, wireBody };
}

/** The OpenRouter preset (`https://openrouter.ai/api/v1`; `testBaseUrl` for the fake server only). */
export const openrouterClient = (opts: Omit<ClientOptions, "preset" | "baseUrl"> = {}) => openAICompatibleClient({ ...opts, preset: "openrouter" });

/** The local preset: a user-run OpenAI-compatible server (Ollama, LM Studio), no key. */
export const localClient = (baseUrl: string, opts: Omit<ClientOptions, "preset" | "baseUrl" | "testBaseUrl"> = {}) =>
  openAICompatibleClient({ ...opts, preset: "local", baseUrl });

/**
 * What OpenRouter finally charged for a generation (`GET /generation?id=…`): settles an attempt
 * whose stream ended before its usage chunk (review B2). Undefined when OpenRouter does not
 * know it (yet).
 */
export async function settleGeneration(
  id: string,
  key: KeySource,
  opts: { testBaseUrl?: string; fetch?: typeof fetch } = {},
): Promise<{ costMicros: number } | undefined> {
  const { url } = checkBaseUrl("openrouter", opts.testBaseUrl ?? OPENROUTER_BASE_URL, opts.testBaseUrl !== undefined);
  const target = `${url.toString().replace(/\/+$/, "")}/generation?id=${encodeURIComponent(id)}`;
  const res = await key((k) => (opts.fetch ?? fetch)(target, { headers: { authorization: `Bearer ${k}`, accept: "application/json" } }));
  if (res.status === 404) return undefined;
  if (!res.ok) throw finError(res.status === 401 || res.status === 403 ? "PROVIDER_AUTH" : "PROVIDER_UNAVAILABLE", `the generation lookup answered ${res.status}`, { status: res.status });
  const doc = (await res.json()) as { data?: { total_cost?: unknown } };
  const cost = doc.data?.total_cost;
  return typeof cost === "number" && Number.isFinite(cost) && cost >= 0 ? { costMicros: Math.round(cost * 1_000_000) } : undefined;
}
