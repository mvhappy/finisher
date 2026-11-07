// The model client's transport rules (plan 7A Decisions 8 and 9; review M1, M4, L2, L3): which
// base URLs each preset may use, how a failure maps to Finisher's codes, how long to wait
// before a retry, and the sleep that waits.
import { FinError } from "@finisher/contracts";
import { APIConnectionError, APIError } from "openai";
import { finError } from "../../errors.ts";
import { OPENROUTER_HOST } from "../catalogue/catalogue.ts";
import type { Preset } from "./request.ts";

/** The openrouter preset's base URL: pinned, never from a setting (review M1). */
export const OPENROUTER_BASE_URL = `https://${OPENROUTER_HOST}/api/v1`;

const LOOPBACK = new Set(["127.0.0.1", "::1", "[::1]", "localhost"]);

export const isLoopback = (url: URL) => LOOPBACK.has(url.hostname);

/**
 * The base URL a client may use, and warnings for the owner.
 * - `openrouter`: `https://openrouter.ai/…` only. `testLoopback` (the fake model server)
 *   allows `http` on loopback; nothing a person or an agent configures can reach that option.
 * - `local`: the owner's `agents.local.baseUrl` (human-only, never agent-writable): `http` or
 *   `https`, no credentials in the URL; plain `http` to another machine is allowed with a
 *   warning (prompts and outputs cross the network unencrypted).
 */
export function checkBaseUrl(preset: Preset, raw: string, testLoopback = false): { url: URL; warnings: string[] } {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw finError("PROVIDER_UNAVAILABLE", "the model server's base URL is not a URL", { preset }, "set an http(s) URL");
  }
  if (url.username !== "" || url.password !== "") {
    throw finError("PROVIDER_UNAVAILABLE", "a base URL may not carry credentials", { preset }, "remove the user and password from the URL");
  }
  if (preset === "openrouter") {
    const pinned = url.protocol === "https:" && url.hostname === OPENROUTER_HOST && url.port === "";
    const test = testLoopback && url.protocol === "http:" && isLoopback(url);
    if (!pinned && !test) {
      throw finError("PROVIDER_UNAVAILABLE", `the openrouter preset talks to ${OPENROUTER_BASE_URL} only`, { preset }, "use the local preset for another server");
    }
    return { url, warnings: [] };
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw finError("PROVIDER_UNAVAILABLE", "the local model server's base URL must be http or https", { preset, protocol: url.protocol }, "set an http(s) URL");
  }
  const warnings =
    url.protocol === "http:" && !isLoopback(url)
      ? ["the local model server is reached over plain http on the network: prompts and outputs travel unencrypted"]
      : [];
  return { url, warnings };
}

/** A sleep the abort signal ends early; its listener is removed whichever comes first (L2). */
export function abortableSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const onAbort = () => {
      clearTimeout(t);
      reject(signal?.reason);
    };
    const t = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/** `retry-after` in seconds or as an HTTP date, in ms; undefined when absent or unreadable. */
export function retryAfterMs(value: string | null | undefined, now: number): number | undefined {
  if (value === null || value === undefined || value.trim() === "") return undefined;
  const secs = Number(value);
  if (Number.isFinite(secs) && secs >= 0) return Math.round(secs * 1000);
  const at = Date.parse(value);
  return Number.isNaN(at) ? undefined : Math.max(0, at - now);
}

export interface Failure {
  readonly error: FinError;
  readonly retryable: boolean;
  readonly retryAfter?: number;
}

const RETRY_STATUS = new Set([408, 429, 500, 502, 503, 504]);
const CONTEXT = /context[ _-]?(length|window)|maximum context|too many tokens|prompt is too long/i;

/** Maps an SDK or transport error to Finisher's codes (spec §15). */
export function classify(e: unknown, now: number): Failure {
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
    const after = retryAfterMs(e.headers?.get("retry-after"), now);
    if (status === 429) {
      return { error: finError("PROVIDER_RATE_LIMITED", "the model server is rate limiting", { ...details, retryAfterMs: after ?? null }), retryable: true, ...(after === undefined ? {} : { retryAfter: after }) };
    }
    if (RETRY_STATUS.has(status)) {
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
