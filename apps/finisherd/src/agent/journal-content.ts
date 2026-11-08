// The agent-trace store's content (plan 7A Decisions 25, 26; spec §12.1, §12.4): bodies (inline
// up to ~4 KiB, else a blob, truncated over 4 MiB), the per-turn message deltas of an exact
// capture, and their replay, which rebuilds each request body byte for byte so its BLAKE3 is
// the turn's `bodyHash`.
//
// A turn's `messagesDelta` (`application/json`), version 1, is one of:
//   {v: 1, from, messages, keys, rest}: the body's messages after the system message are the
//     previous turn's first `from` plus `messages` (a JSON array, as text); `keys` is the
//     body's key order; `rest` (a JSON object, as text) holds every key but `messages` and
//     `tools`, which come from the turn's `system` and `tools` blobs (`null` when the body had
//     none). JSON text, because the journal stores inline JSON with its keys sorted, and the
//     replay must keep every object's key order to rebuild the bytes;
//   {v: 1, raw}: the body as sent, when it does not rebuild exactly from a delta (checked at
//     record time), or is not a JSON object with a `messages` array.
// The first turn of a run has `from: 0`. `replayBodies` is the reference reader.
import type { BlobRef, Body, StopReason } from "@finisher/core-types";
import { blake3Hex } from "@finisher/fin-node";

export type { Body, StopReason };

/** A content field over this many bytes becomes a blob (spec §12.4). */
export const INLINE_MAX = 4 * 1024;
/** The journal's blob cap; a larger value is truncated (`truncated: true`). */
export const BLOB_MAX = 4 * 1024 * 1024;
const JSON_MEDIA = "application/json";

/** What attaching a blob needs (the trace store's `Journal`). */
export interface BlobSink {
  attachBlob(bytes: Uint8Array, mediaType: string): { blob: BlobRef } | undefined;
}

export const bytesOf = (s: string) => Buffer.byteLength(s, "utf8");

/** `blake3:<hex>` of `text`'s UTF-8 bytes, unkeyed (the journal keys it). */
export const hashOf = (text: string) => `blake3:${blake3Hex(Buffer.from(text, "utf8"))}`;

/** The head of `text` that fits `max` bytes of UTF-8 (a cut character is dropped). */
function headOf(text: string, max: number): string {
  const b = Buffer.from(text, "utf8");
  if (b.length <= max) return text;
  return b.subarray(0, max).toString("utf8").replace(/�$/, "");
}

/**
 * `value` as a content field: inline when its JSON is at most `INLINE_MAX` bytes (unless
 * `alwaysBlob`), else a blob; over `BLOB_MAX`, the blob holds `{truncated: true, head}` and
 * `truncated` is set. Undefined when the store took no blob (training went off meanwhile).
 */
export function bodyOf(sink: BlobSink, value: unknown, alwaysBlob = false): { body: Body; truncated: boolean } | undefined {
  const text = JSON.stringify(value ?? null);
  if (!alwaysBlob && bytesOf(text) <= INLINE_MAX) return { body: { inline: value ?? null }, truncated: false };
  return blobOf(sink, text);
}

/** `text` (JSON) as a blob, truncated over `BLOB_MAX`. */
export function blobOf(sink: BlobSink, text: string): { body: Body; truncated: boolean } | undefined {
  let out = text;
  let truncated = false;
  if (bytesOf(out) > BLOB_MAX) {
    truncated = true;
    let room = BLOB_MAX - 64 * 1024;
    do {
      out = JSON.stringify({ truncated: true, head: headOf(text, room) });
      room = Math.floor(room * 0.8);
    } while (bytesOf(out) > BLOB_MAX);
  }
  const body = sink.attachBlob(Buffer.from(out, "utf8"), JSON_MEDIA);
  return body === undefined ? undefined : { body, truncated };
}

// Message deltas ----------------------------------------------------------------------------

type Json = Record<string, unknown>;

export type MessagesDelta =
  | { readonly v: 1; readonly from: number; readonly messages: string; readonly keys: string[]; readonly rest: string }
  | { readonly v: 1; readonly raw: string };

/** One exact request split for the trace store. */
export interface SplitRequest {
  readonly delta: MessagesDelta;
  /** The system message's JSON (`null` when the body has none), always a blob. */
  readonly system: string;
  /** The tool definitions' JSON (`null` when none), always a blob. */
  readonly tools: string;
  /** The body's messages after the system message: the next turn's base. */
  readonly messages: unknown[];
}

const isObject = (v: unknown): v is Json => v !== null && typeof v === "object" && !Array.isArray(v);

/** `request` (the body as sent) against the previous turn's messages of the run. */
export function splitRequest(request: string, previous: readonly unknown[]): SplitRequest {
  let body: unknown;
  try {
    body = JSON.parse(request);
  } catch {
    body = undefined;
  }
  if (!isObject(body) || !Array.isArray(body.messages)) return { delta: { v: 1, raw: request }, system: "null", tools: "null", messages: [] };
  const all = body.messages as unknown[];
  const head = all[0];
  const hasSystem = isObject(head) && head.role === "system";
  const messages = hasSystem ? all.slice(1) : all;
  const system = hasSystem ? JSON.stringify(head) : "null";
  const tools = JSON.stringify(body.tools ?? null);
  let from = 0;
  while (from < previous.length && from < messages.length && JSON.stringify(previous[from]) === JSON.stringify(messages[from])) from += 1;
  const rest: Json = {};
  for (const [k, v] of Object.entries(body)) if (k !== "messages" && k !== "tools") rest[k] = v;
  const delta: MessagesDelta = { v: 1, from, messages: JSON.stringify(messages.slice(from)), keys: Object.keys(body), rest: JSON.stringify(rest) };
  // Kept only when it rebuilds the exact bytes; else the body itself.
  const rebuilt = rebuild(delta, previous, system, tools);
  return { delta: rebuilt?.body === request ? delta : { v: 1, raw: request }, system, tools, messages };
}

function rebuild(d: MessagesDelta, previous: readonly unknown[], system: string, tools: string): { body: string; messages: unknown[] } | undefined {
  if ("raw" in d) {
    let body: unknown;
    try {
      body = JSON.parse(d.raw);
    } catch {
      return { body: d.raw, messages: [] };
    }
    const all = isObject(body) && Array.isArray(body.messages) ? (body.messages as unknown[]) : [];
    const first = all[0];
    return { body: d.raw, messages: isObject(first) && first.role === "system" ? all.slice(1) : all };
  }
  if (d.from > previous.length) return undefined;
  try {
    const messages = [...previous.slice(0, d.from), ...(JSON.parse(d.messages) as unknown[])];
    const rest = JSON.parse(d.rest) as Json;
    const sys = JSON.parse(system) as unknown;
    const out: Json = {};
    for (const k of d.keys) {
      if (k === "messages") out[k] = sys === null ? messages : [sys, ...messages];
      else if (k === "tools") out[k] = JSON.parse(tools) as unknown;
      else out[k] = rest[k];
    }
    return { body: JSON.stringify(out), messages };
  } catch {
    return undefined;
  }
}

/**
 * The dataset builder's replay of one run's turns, oldest first: each turn's exact request
 * body, or undefined where a delta does not apply (that example is dropped).
 */
export function replayBodies(turns: ReadonlyArray<{ delta: MessagesDelta; system: string; tools: string }>): Array<string | undefined> {
  let previous: unknown[] = [];
  return turns.map((t) => {
    const r = rebuild(t.delta, previous, t.system, t.tools);
    if (r === undefined) return undefined;
    previous = r.messages;
    return r.body;
  });
}

// The capture event ---------------------------------------------------------------------------

const STOPS: ReadonlySet<string> = new Set(["endTurn", "toolUse", "maxTokens", "stopSequence", "refusal", "other"]);

/** What a turn takes from a `capture` event (`RuntimeJournal.capture`). */
export interface CaptureMeta {
  /** The request body as sent (JSON text), or undefined when the capture carries none. */
  readonly request: string | undefined;
  /** The response as received (the stream's chunks). */
  readonly response: unknown;
  readonly stopReason: StopReason;
  readonly upstream?: string;
  readonly latencyMs: number;
  readonly ttftMs?: number;
}

const count = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? Math.round(v) : undefined);

/**
 * Reads a capture: `request` the body text (an object is serialised), `response` either the
 * chunks or `{chunks, stopReason?, upstream?, latencyMs?, ttftMs?}` (the client's `ModelResult`
 * fields; see `RuntimeJournal.capture`).
 */
export function captureMeta(request: unknown, response: unknown): CaptureMeta {
  const text = typeof request === "string" ? request : request === undefined || request === null ? undefined : JSON.stringify(request);
  if (!isObject(response) || !("chunks" in response)) return { request: text, response, stopReason: "other", latencyMs: 0 };
  const stop = typeof response.stopReason === "string" && STOPS.has(response.stopReason) ? (response.stopReason as StopReason) : "other";
  const ttft = count(response.ttftMs);
  return {
    request: text,
    response: response.chunks,
    stopReason: stop,
    ...(typeof response.upstream === "string" && response.upstream.length > 0 ? { upstream: response.upstream } : {}),
    latencyMs: count(response.latencyMs) ?? 0,
    ...(ttft === undefined ? {} : { ttftMs: ttft }),
  };
}
