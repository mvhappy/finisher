// Media bytes over HTTP (6B-1 plan Task 17, Decisions 13 and 14; spec 6 §7.3): `GET` and `HEAD`
// `/media/cap/<token>/<file>`, routed by `upgrade.ts` before the WebSocket path. Checked in order:
//   1. `Host` and `Origin` are the listener's (403, as the HTTP front: DNS rebinding);
//   2. the method is `GET` or `HEAD` (405);
//   3. the path is exactly `/media/cap/<token>/<file>`, the token is live (not unknown, expired
//      or revoked) and was minted for exactly this `<file>` once decoded, and the entry the token
//      names still lists the file in its `meta.json`: every failure is one 404, byte for byte,
//      so nothing says which of them it was (`..`, `%2e%2e`, `meta.json` and every name the
//      entry does not list are among them).
// The file is read by `MediaCache.readFile` (fin-shm's checked opener, no link at any segment),
// 1 MiB at a time, the entry pinned `http:<n>` until the response ends (an evicted entry would
// otherwise vanish mid-stream). Single `bytes=` ranges answer 206, a range past the end 416;
// a `Range` that is not one range of bytes is ignored (200). A reader that stops reading for
// `STALL_MS` is cut off. `Cache-Control: private, max-age=600, immutable` (an entry is
// content-addressed and never changes) and `Cross-Origin-Resource-Policy: same-origin`.
import type { IncomingMessage, ServerResponse } from "node:http";
import { MEDIA_READ_MAX, type MediaCache } from "@finisher/fin-node";
import { hostRefused, originRefused } from "../http/front.ts";
import { hostAllowed, type ListenerPolicy, originAllowed } from "../http/policy.ts";
import type { CapGrant, CapTokens } from "./cap-tokens.ts";

export const MEDIA_PREFIX = "/media/";
/** A reader that takes nothing for this long is cut off (its pin and socket are released). */
export const STALL_MS = 60_000;

export interface MediaHttpDeps {
  readonly tokens: CapTokens;
  /** The cache, read per request (`undefined`: it did not open, every request is a 404). */
  readonly cache: () => MediaCache | undefined;
  readonly stallMs?: number;
}

/** Handles a request `isMediaRequest` accepted; `authorised` is called once its token is live. */
export type MediaRequestHandler = (req: IncomingMessage, res: ServerResponse, authorised: () => void) => void;

/** Whether `url` (a request target) is the media route's: everything under `/media/`. */
export function isMediaRequest(url: string | undefined): boolean {
  return url !== undefined && (url.startsWith(MEDIA_PREFIX) || url === "/media" || url.startsWith("/media?"));
}

const TYPES: Readonly<Record<string, string>> = {
  mp4: "video/mp4",
  m4v: "video/mp4",
  webm: "video/webm",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  json: "application/json",
};

/** The `Content-Type` of an entry file, by its extension; `application/octet-stream` for the rest. */
export function contentTypeOf(file: string): string {
  const dot = file.lastIndexOf(".");
  return (dot < 0 ? undefined : TYPES[file.slice(dot + 1).toLowerCase()]) ?? "application/octet-stream";
}

export type RangeAnswer = { readonly kind: "whole" } | { readonly kind: "range"; readonly start: number; readonly end: number } | { readonly kind: "unsatisfiable" };

const RANGE = /^bytes=(\d*)-(\d*)$/i;
/** More digits than this are past any file's end. */
const MAX_DIGITS = 15;

/**
 * A `Range` header against a file of `size` bytes: one `bytes=a-b`, `a-` or `-n` is a range
 * (clamped to the file) or unsatisfiable (416); any other header, several ranges included, is
 * ignored (the whole file).
 */
export function parseRange(header: string | undefined, size: number): RangeAnswer {
  if (header === undefined) return { kind: "whole" };
  const m = RANGE.exec(header);
  if (m === null || (m[1] === "" && m[2] === "")) return { kind: "whole" };
  const [first, last] = [m[1]!, m[2]!];
  if (first === "") {
    // A suffix: the last `n` bytes.
    const n = last.length > MAX_DIGITS ? size : Number(last);
    if (n === 0 || size === 0) return { kind: "unsatisfiable" };
    return { kind: "range", start: Math.max(0, size - n), end: size - 1 };
  }
  if (first.length > MAX_DIGITS) return { kind: "unsatisfiable" };
  const start = Number(first);
  const end = last === "" ? size - 1 : last.length > MAX_DIGITS ? size - 1 : Number(last);
  if (start >= size || (last !== "" && last.length <= MAX_DIGITS && start > end)) return { kind: "unsatisfiable" };
  return { kind: "range", start, end: Math.min(end, size - 1) };
}

/** The same bytes for every refusal of a token: nothing but the status says anything. */
const NOT_FOUND = { "Cache-Control": "no-store", "Content-Length": "0", "X-Content-Type-Options": "nosniff" } as const;

function notFound(res: ServerResponse): void {
  if (res.headersSent) return void res.destroy();
  res.writeHead(404, NOT_FOUND);
  res.end();
}

function refuse(res: ServerResponse, status: number, e: { code: string; message: string; fix?: string | undefined }): void {
  const text = JSON.stringify({ error: { code: e.code, message: e.message, details: {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } });
  res.writeHead(status, { "Content-Type": "application/json", "Content-Length": String(Buffer.byteLength(text)), "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff", Connection: "close" });
  res.end(text);
}

/** `/media/cap/<token>/<file>` → the token and the decoded file; `undefined` for any other shape. */
function parsePath(url: string): { token: string; file: string } | undefined {
  const target = url.split(/[?#]/, 1)[0]!;
  const parts = target.split("/");
  if (parts.length !== 5 || parts[0] !== "" || parts[1] !== "media" || parts[2] !== "cap" || parts[3] === "" || parts[4] === "") return undefined;
  try {
    return { token: parts[3]!, file: decodeURIComponent(parts[4]!) };
  } catch {
    return undefined;
  }
}

let seq = 0;

export function makeMediaHttp(deps: MediaHttpDeps, policy: ListenerPolicy): MediaRequestHandler {
  const stallMs = deps.stallMs ?? STALL_MS;
  return (req, res, authorised) => {
    const host = typeof req.headers.host === "string" ? req.headers.host : undefined;
    if (!hostAllowed(policy, host)) return refuse(res, 403, hostRefused());
    const origin = typeof req.headers.origin === "string" ? req.headers.origin : undefined;
    if (!originAllowed(policy, origin)) return refuse(res, 403, originRefused());
    const method = (req.method ?? "GET").toUpperCase();
    if (method !== "GET" && method !== "HEAD") {
      res.writeHead(405, { Allow: "GET, HEAD", "Content-Length": "0", "Cache-Control": "no-store" });
      return void res.end();
    }
    const asked = parsePath(req.url ?? "");
    const grant = asked === undefined ? undefined : deps.tokens.resolve(asked.token);
    if (asked === undefined || grant === undefined || grant.file !== asked.file) return notFound(res);
    // A token that checks out is a client the server has reason to serve: its socket no longer
    // counts against the pre-upgrade pool or its header timeout (a stream outlives both).
    authorised();
    void serve(deps, grant, req, res, method === "HEAD", stallMs).catch(() => notFound(res));
  };
}

/** Resolves when `res` drains, closes or `stallMs` passes (then it is cut off); false when it is gone. */
function drained(res: ServerResponse, stallMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    if (res.destroyed) return resolve(false);
    const done = (ok: boolean) => {
      clearTimeout(timer);
      res.off("drain", onDrain);
      res.off("close", onClose);
      resolve(ok);
    };
    const onDrain = () => done(true);
    const onClose = () => done(false);
    const timer = setTimeout(() => {
      res.destroy();
      done(false);
    }, stallMs);
    res.once("drain", onDrain);
    res.once("close", onClose);
  });
}

async function serve(deps: MediaHttpDeps, grant: CapGrant, req: IncomingMessage, res: ServerResponse, head: boolean, stallMs: number): Promise<void> {
  const cache = deps.cache();
  if (cache === undefined) return notFound(res);
  const holder = `http:${++seq}`;
  let pinned = false;
  try {
    // Pin first, then look: an entry that is evicted between the two is a miss, never a vanishing file.
    pinned = await cache.pin(grant.key, holder);
    const meta = pinned ? await cache.lookup(grant.key) : null;
    const entry = meta?.files.find((f) => f.name === grant.file);
    if (entry === undefined) return notFound(res);
    const size = entry.bytes;
    const etag = `"${entry.blake3}"`;
    let answer = parseRange(typeof req.headers.range === "string" ? req.headers.range : undefined, size);
    // `If-Range` names a validator: anything but this file's own is a stale copy, sent whole.
    const ifRange = req.headers["if-range"];
    if (answer.kind !== "whole" && ifRange !== undefined && ifRange !== etag) answer = { kind: "whole" };
    const headers: Record<string, string> = {
      "Content-Type": contentTypeOf(grant.file),
      "Accept-Ranges": "bytes",
      ETag: etag,
      "Cache-Control": "private, max-age=600, immutable",
      "Cross-Origin-Resource-Policy": "same-origin",
      "X-Content-Type-Options": "nosniff",
    };
    if (answer.kind === "unsatisfiable") {
      res.writeHead(416, { ...headers, "Content-Range": `bytes */${size}`, "Content-Length": "0" });
      return void res.end();
    }
    const [start, end] = answer.kind === "range" ? [answer.start, answer.end] : [0, size - 1];
    const length = size === 0 ? 0 : end - start + 1;
    // The first read comes before the headers: a file that does not open (a link, one larger
    // than listed) is a 404, not half an answer.
    const first = length === 0 ? Buffer.alloc(0) : await cache.readFile(grant.key, grant.file, start, Math.min(length, MEDIA_READ_MAX));
    if (first === null || (length > 0 && first.length === 0)) return notFound(res);
    res.writeHead(answer.kind === "range" ? 206 : 200, {
      ...headers,
      ...(answer.kind === "range" ? { "Content-Range": `bytes ${start}-${end}/${size}` } : {}),
      "Content-Length": String(length),
    });
    if (head) return void res.end();
    let closed = false;
    res.once("close", () => (closed = true));
    let sent = 0;
    let chunk: Buffer | null = first;
    while (chunk !== null && !closed) {
      if (chunk.length === 0) break;
      sent += chunk.length;
      if (!res.write(chunk) && !(await drained(res, stallMs))) return;
      if (sent >= length) break;
      chunk = await cache.readFile(grant.key, grant.file, start + sent, Math.min(length - sent, MEDIA_READ_MAX));
    }
    // A file shorter than listed, or gone: the length promised was not sent, so the client must see an abort.
    if (sent < length || chunk === null) return void res.destroy();
    res.end();
  } finally {
    if (pinned) void cache.unpin(grant.key, holder).catch(() => undefined);
  }
}
