// The built-in agent's model client (plan 7A Decisions 7–9, with the decision taken after the
// spec): one OpenAI-compatible Chat Completions client, two presets. `openrouter` sends the
// user's key and OpenRouter's extensions (reasoning, cache breakpoints, reported cost and the
// upstream provider); `local` (Ollama, LM Studio, any user-run server) sends no key. The
// `openai` SDK is transport and SSE parsing only: Finisher builds the body (`request.ts`), the
// bytes the SDK sends are captured as they leave, and `bodyHash` is their BLAKE3. The client
// owns retries (the SDK's are off) and never runs a tool.
import { FinError } from "@finisher/contracts";
import OpenAI, { APIConnectionError, APIError } from "openai";
import { finError } from "../../errors.ts";
import { blake3 } from "../blake3.ts";
import { type Caching, type Usage, ZERO_USAGE } from "../catalogue/catalogue.ts";
import type { Clock } from "../clock.ts";
import { systemClock } from "../clock.ts";
import { type BodyOptions, chatBody, type ModelRequest, type Preset, type ToolCall } from "./request.ts";

export type { ModelRequest, Preset };

/** Calls `f` with the key for one request (Task 5's gateway in 7A-2; a constant in tests). */
export type KeySource = <A>(f: (key: string) => Promise<A>) => Promise<A>;

/** The local preset's key source: there is no key. */
export const noKey: KeySource = (f) => f("");

export type StopReason = "endTurn" | "toolUse" | "maxTokens" | "stopSequence" | "refusal" | "other";

export interface ModelResult {
  readonly text: string;
  readonly toolCalls: ToolCall[];
  /** Reasoning text as streamed, for display. */
  readonly reasoningText: string;
  /** The provider's reasoning details, rebuilt from the deltas; sent back unchanged next call. */
  readonly reasoning: unknown[];
  readonly stopReason: StopReason;
  readonly usage: Usage;
  /** What the provider charged, in micros (OpenRouter's usage accounting). */
  readonly costMicros?: number;
  /** The provider that served the call, as OpenRouter names it. */
  readonly upstream?: string;
  readonly generationId?: string;
  /** The exact request body as sent, and the response chunks as received. */
  readonly capture: { readonly request: string; readonly response: unknown[] };
  /** BLAKE3 of the request body bytes, lower-case hex. */
  readonly bodyHash: string;
  readonly latencyMs: number;
  readonly ttftMs?: number;
}

export type ModelStreamEvent =
  | { readonly type: "textDelta"; readonly text: string }
  | { readonly type: "reasoningDelta"; readonly text: string }
  /** The attempt failed and the same body is sent again after `delayMs`: discard its partial output. */
  | { readonly type: "retry"; readonly attempt: number; readonly code: string; readonly delayMs: number }
  | { readonly type: "done"; readonly result: ModelResult };

export interface ModelClient {
  readonly preset: Preset;
  stream(req: ModelRequest, key: KeySource, signal?: AbortSignal): AsyncIterable<ModelStreamEvent>;
  /** The body that is sent for `req`, for capture. */
  readonly wireBody: (req: ModelRequest) => Record<string, unknown>;
}

export interface ClientOptions {
  readonly preset: Preset;
  readonly baseUrl: string;
  /** The model's caching class (the catalogue's); default `none`. */
  readonly cachingFor?: (model: string) => Caching;
  readonly clock?: Clock;
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
  /** Attempts per call, the first included (Decision 9: 5). */
  readonly maxAttempts?: number;
  /** Per-request timeout in ms. */
  readonly timeoutMs?: number;
}

const RETRY_STATUS = new Set([408, 429, 500, 502, 503, 504]);
const MAX_DELAY_MS = 60_000;
const LOOPBACK = new Set(["127.0.0.1", "::1", "[::1]", "localhost"]);

/**
 * Checks a base URL: `https`, or `http` on loopback (the fake model server, a local server on
 * this machine). The local preset also takes `http` on another host: it sends no key.
 */
export function checkBaseUrl(preset: Preset, raw: string): URL {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw finError("PROVIDER_UNAVAILABLE", "the model server's base URL is not a URL", { preset }, "set an http(s) URL");
  }
  const loopback = LOOPBACK.has(url.hostname);
  const ok = url.protocol === "https:" || (url.protocol === "http:" && (loopback || preset === "local"));
  if (!ok || url.username !== "" || url.password !== "") {
    throw finError("PROVIDER_UNAVAILABLE", `the ${preset} base URL must be https (or http on this machine)`, { preset, protocol: url.protocol }, "use an https URL");
  }
  return url;
}

const defaultSleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(t);
      reject(signal.reason);
    });
  });

/** `retry-after` in seconds or as an HTTP date, in ms; undefined when absent or unreadable. */
export function retryAfterMs(value: string | null | undefined, now: number): number | undefined {
  if (value === null || value === undefined || value.trim() === "") return undefined;
  const secs = Number(value);
  if (Number.isFinite(secs) && secs >= 0) return Math.round(secs * 1000);
  const at = Date.parse(value);
  return Number.isNaN(at) ? undefined : Math.max(0, at - now);
}

interface Failure {
  readonly error: FinError;
  readonly retryable: boolean;
  readonly retryAfter?: number;
}

const CONTEXT = /context[ _-]?(length|window)|maximum context|too many tokens|prompt is too long/i;

/** Maps an SDK or transport error to Finisher's codes (spec §15). */
function classify(e: unknown, now: number): Failure {
  if (e instanceof FinError) return { error: e, retryable: false };
  if (e instanceof SyntaxError) {
    return { error: finError("PROVIDER_PROTOCOL", "the model server sent an event that is not JSON", {}), retryable: false };
  }
  if (e instanceof APIError) {
    const status = e.status;
    const body = (e.error ?? {}) as { code?: unknown; message?: unknown };
    const message = typeof body.message === "string" ? body.message : e.message;
    const details = { status: status ?? null, providerCode: typeof body.code === "string" || typeof body.code === "number" ? body.code : null };
    if (e instanceof APIConnectionError) {
      // APIConnectionError and its timeout subclass: the connection failed or was reset.
      return { error: finError("PROVIDER_OVERLOADED", "the connection to the model server failed", details), retryable: true };
    }
    if (status === undefined) {
      // An error chunk inside a 200 stream (OpenRouter: `finish_reason: "error"`).
      return { error: finError("PROVIDER_OVERLOADED", `the model stream failed: ${message}`, details), retryable: true };
    }
    if (status === 401 || status === 403) {
      return { error: finError("PROVIDER_AUTH", "the model server refused the key", details, "check the OpenRouter key (`finisherd provider key set openrouter`)"), retryable: false };
    }
    if (status === 402) {
      return { error: finError("PROVIDER_AUTH", "the OpenRouter key has no credit left", { ...details, reason: "credits" }, "add credit or raise the key's limit at OpenRouter"), retryable: false };
    }
    if (status === 429) {
      const after = retryAfterMs(e.headers?.get("retry-after"), now);
      return { error: finError("PROVIDER_RATE_LIMITED", "the model server is rate limiting", { ...details, retryAfterMs: after ?? null }), retryable: true, ...(after === undefined ? {} : { retryAfter: after }) };
    }
    if (RETRY_STATUS.has(status)) {
      const after = retryAfterMs(e.headers?.get("retry-after"), now);
      return { error: finError("PROVIDER_OVERLOADED", `the model server answered ${status}`, details), retryable: true, ...(after === undefined ? {} : { retryAfter: after }) };
    }
    if ((status === 400 || status === 413) && (CONTEXT.test(message) || body.code === "context_length_exceeded")) {
      return { error: finError("MODEL_CONTEXT_EXCEEDED", "the request does not fit the model's context window", details, "start a new run or session"), retryable: false };
    }
    if (status === 404) return { error: finError("MODEL_UNKNOWN", "the model server does not know the model", details), retryable: false };
    return { error: finError("PROVIDER_PROTOCOL", `the model server refused the request (${status})`, details), retryable: false };
  }
  // A reset socket or a body cut short (`fetch` failing mid-stream).
  return { error: finError("PROVIDER_OVERLOADED", "the model stream was cut short", { cause: e instanceof Error ? e.name : typeof e }), retryable: true };
}

const STOP: Record<string, StopReason> = {
  stop: "endTurn",
  tool_calls: "toolUse",
  function_call: "toolUse",
  length: "maxTokens",
  content_filter: "refusal",
};

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
      out.push({ type: "textDelta", text: delta.content });
    }
    const r = typeof delta.reasoning === "string" ? delta.reasoning : typeof delta.reasoning_content === "string" ? delta.reasoning_content : "";
    if (r !== "") {
      this.firstAt ??= now;
      this.reasoningText += r;
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
        if (typeof fn.arguments === "string") call.arguments += fn.arguments;
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

  usageOf(): Usage {
    const u = this.usage ?? {};
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

  costMicros(): number | undefined {
    const c = this.usage?.cost;
    return typeof c === "number" && Number.isFinite(c) && c >= 0 ? Math.round(c * 1_000_000) : undefined;
  }
}

export function openAICompatibleClient(opts: ClientOptions): ModelClient {
  const base = checkBaseUrl(opts.preset, opts.baseUrl).toString().replace(/\/+$/, "");
  const clock = opts.clock ?? systemClock;
  const sleep = opts.sleep ?? defaultSleep;
  const maxAttempts = opts.maxAttempts ?? 5;
  const bodyOptions = (model: string): BodyOptions => ({ preset: opts.preset, caching: opts.cachingFor?.(model) ?? "none" });
  const wireBody = (req: ModelRequest) => chatBody(req, bodyOptions(req.model));

  /** One attempt: the SDK built per request, with the key, no retries and no logging. */
  async function* attempt(body: Record<string, unknown>, key: KeySource, sent: { text?: string }, signal?: AbortSignal) {
    const started = clock.now();
    const stream = await key(async (k) => {
      const client = new OpenAI({
        apiKey: opts.preset === "openrouter" ? k : "local",
        baseURL: base,
        maxRetries: 0,
        timeout: opts.timeoutMs ?? 600_000,
        logLevel: "off",
        fetch: async (url: string | URL | Request, init?: RequestInit) => {
          const headers = new Headers(init?.headers);
          for (const h of [...headers.keys()]) if (h.startsWith("x-stainless-")) headers.delete(h);
          if (opts.preset === "local") headers.delete("authorization");
          if (typeof init?.body === "string") sent.text = init.body;
          return fetch(url, { ...init, headers });
        },
      });
      const params = body as unknown as OpenAI.Chat.Completions.ChatCompletionCreateParamsStreaming;
      return client.chat.completions.create(params, signal === undefined ? {} : { signal });
    });
    yield { started };
    for await (const chunk of stream) yield { chunk: chunk as unknown as Record<string, unknown> };
  }

  async function* streamCall(req: ModelRequest, key: KeySource, signal?: AbortSignal): AsyncIterable<ModelStreamEvent> {
    const body = wireBody(req);
    for (let n = 1; ; n++) {
      const sent: { text?: string } = {};
      const a = new Assembly();
      let started = clock.now();
      try {
        for await (const step of attempt(body, key, sent, signal)) {
          if ("started" in step) {
            started = step.started;
            continue;
          }
          const out: ModelStreamEvent[] = [];
          a.add(step.chunk, clock.now(), out);
          yield* out;
        }
        if (a.finish === null) throw new TypeError("the stream ended before the model finished");
        if (a.finish === "error") throw new APIError(undefined, { message: "the stream ended with an error" }, undefined, undefined);
      } catch (e) {
        if (signal?.aborted) throw finError("SESSION_INTERRUPTED", "the model call was interrupted", {});
        const f = classify(e, clock.now());
        if (!f.retryable || n >= maxAttempts) throw f.error;
        const delayMs = Math.min(MAX_DELAY_MS, f.retryAfter ?? 1000 * 2 ** (n - 1));
        yield { type: "retry", attempt: n, code: f.error.code, delayMs };
        await sleep(delayMs, signal);
        continue;
      }
      const request = sent.text ?? JSON.stringify(body);
      const done = clock.now();
      const calls = a.calls.filter((c) => c !== undefined);
      const stopReason: StopReason = a.finish !== null ? (STOP[a.finish] ?? "other") : "other";
      const costMicros = opts.preset === "openrouter" ? a.costMicros() : undefined;
      yield {
        type: "done",
        result: {
          text: a.text,
          toolCalls: calls,
          reasoningText: a.reasoningText,
          reasoning: a.reasoning,
          stopReason,
          usage: a.usage ? a.usageOf() : ZERO_USAGE,
          ...(costMicros === undefined ? {} : { costMicros }),
          ...(a.upstream === undefined ? {} : { upstream: a.upstream }),
          ...(a.id === undefined ? {} : { generationId: a.id }),
          capture: { request, response: a.chunks },
          bodyHash: blake3(Buffer.from(request, "utf8")).toString("hex"),
          latencyMs: done - started,
          ...(a.firstAt === undefined ? {} : { ttftMs: a.firstAt - started }),
        },
      };
      return;
    }
  }

  return { preset: opts.preset, stream: streamCall, wireBody };
}

/** The OpenRouter preset (the catalogue's `openrouterBaseUrl`). */
export const openrouterClient = (baseUrl: string, opts: Omit<ClientOptions, "preset" | "baseUrl"> = {}) =>
  openAICompatibleClient({ ...opts, preset: "openrouter", baseUrl });

/** The local preset: a user-run OpenAI-compatible server (Ollama, LM Studio), no key. */
export const localClient = (baseUrl: string, opts: Omit<ClientOptions, "preset" | "baseUrl"> = {}) =>
  openAICompatibleClient({ ...opts, preset: "local", baseUrl });
