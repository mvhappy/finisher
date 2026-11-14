// The Codex app-server client (sub-project 7E plan Task 3, Decision 3; Review Focus 2 and 6):
// JSON-RPC over the app-server's stdio, one JSON object per `\n`-terminated line each way
// (`--listen stdio://`; Codex writes no `"jsonrpc"` member and needs none).
//
// - **Framing:** 7D's `lineFramer` (≤ 16 MiB a line, a `\r` before the `\n` dropped, blank lines
//   skipped, bytes buffered so a split UTF-8 sequence survives).
// - **Requests:** numeric ids from 1, a pending map keyed by id (responses may come in any
//   order), each result decoded with its method's generated schema. **Only `ALLOWED_METHODS`
//   is ever sent**: any other method of the generated `ClientRequest` union (the logins, logout,
//   OAuth, config writes, `fs/*`, `command/exec*`, `thread/fork`, … and whatever a re-pin adds) is
//   refused at `request` before anything is written. An error response fails its request only:
//   `PROVIDER_PROTOCOL {method, reason: "rpcError", rpcError: {code, message}}`, which the adapter
//   reads (`-32600 "no rollout found …"` is a lost transcript, Decision 17).
// - **The end:** the server's EOF fails every pending request `PROVIDER_EXITED {method, reason:
//   "eof"}` and ends `notifications`; a request after it fails at once.
// - **Drift fails closed:** a line that is not JSON-RPC, a response to an id never sent, a line
//   over the cap, or a known notification or server request missing a needed field is
//   `PROVIDER_PROTOCOL`: pending requests and `notifications` fail with it and the client stops
//   (a bad server request is answered `-32602` first). A method the pin's schema does not list
//   for the adapter is an `unknown` notification or request, never an error.
// - **Server requests** go to one `ServerRequestPort` (held until `serve`); its reply is typed to
//   the request's method (a reply for another method is answered `-32603`), and a request Codex
//   resolves itself (`serverRequest/resolved`) stops its handler.
//
// Portions adapted from T3 Code's `effect-codex-app-server` (`src/protocol.ts`,
// `src/_internal/shared.ts`) at pingdotgg/t3code@5f85768394f417f5bd098ca7fe4fc41ef07a7cda: the
// message routing, the pending map failed at termination, the single termination, and releasing
// a handler on `serverRequest/resolved`. Rewritten for Effect 4.0.6, Finisher's errors and the
// generated schemas. Its licence (THIRD_PARTY_NOTICES.md, "T3 Code"):
//
//   MIT License
//
//   Copyright (c) 2026 T3 Tools Inc.
//
//   Permission is hereby granted, free of charge, to any person obtaining a copy
//   of this software and associated documentation files (the "Software"), to deal
//   in the Software without restriction, including without limitation the rights
//   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//   copies of the Software, and to permit persons to whom the Software is
//   furnished to do so, subject to the following conditions:
//
//   The above copyright notice and this permission notice shall be included in all
//   copies or substantial portions of the Software.
//
//   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//   SOFTWARE.
import type { Readable, Writable } from "node:stream";
import { FinError } from "@finisher/contracts";
import { Cause, Effect, Exit, type Fiber, Queue, Schema, Stream } from "effect";
import { finError } from "../../errors.ts";
import { lineFramer, MAX_LINE_BYTES } from "../claude/protocol.ts";
import * as P from "./generated/protocol.ts";

/**
 * The client requests Finisher may send (Decision 3), and nothing else. Exactly the generator's
 * `CLIENT_METHODS` (`scripts/check-codex-protocol.mjs` checks both, and that each is a method of
 * the pinned schema). `thread/inject_items` joins only if Task 0 adopts it (Decision 17).
 */
export const ALLOWED_METHODS: ReadonlySet<P.ClientMethod> = new Set<P.ClientMethod>([
  "initialize",
  "thread/start",
  "thread/resume",
  "thread/revert",
  "thread/read",
  "turn/start",
  "turn/steer",
  "turn/interrupt",
  "model/list",
  "account/read",
  "config/read",
  "configRequirements/read",
  "skills/list",
  "hooks/list",
  "plugin/installed",
  "mcpServerStatus/list",
  "app/installed",
  "experimentalFeature/list",
]);

// ---- types ----

type Requests = typeof P.CLIENT_REQUESTS;
type Notifications = typeof P.SERVER_NOTIFICATIONS;
type ServerRequests = typeof P.SERVER_REQUESTS;
type TypeOf<S> = S extends Schema.Top ? S["Type"] : undefined;

export type ClientMethod = P.ClientMethod;
/** A method the adapter has schemas for (the allow-list). */
export type AllowedMethod = keyof Requests;
/** What `request(m, …)` takes: `never` for a method off the allow-list. */
export type ParamsOf<M extends ClientMethod> = M extends AllowedMethod ? P.Closed<TypeOf<Requests[M]["params"]>> : never;
export type ResultOf<M extends ClientMethod> = M extends AllowedMethod ? Requests[M]["result"]["Type"] : never;

export type KnownNotification = { [M in keyof Notifications]: { readonly method: M; readonly params: Notifications[M]["Type"] } }[keyof Notifications];
/** A notification the adapter does not decode (`name` is its method): the mapper's notice. */
export interface UnknownNotification {
  readonly method: "unknown";
  readonly name: string;
  readonly params: unknown;
}
export type ServerNotification = KnownNotification | UnknownNotification;

export type ServerRequestMethod = keyof ServerRequests;
export type KnownServerRequest = {
  [M in ServerRequestMethod]: { readonly id: P.RequestId; readonly method: M; readonly params: ServerRequests[M]["params"]["Type"] };
}[ServerRequestMethod];
export interface UnknownServerRequest {
  readonly id: P.RequestId;
  readonly method: "unknown";
  readonly name: string;
  readonly params: unknown;
}
export type ServerRequest = KnownServerRequest | UnknownServerRequest;

export interface RpcErrorBody {
  readonly code: number;
  readonly message: string;
}
/** The answer to a server request: a result typed to its method, or a JSON-RPC error. */
export type ServerReply =
  | { [M in ServerRequestMethod]: { readonly method: M; readonly result: P.Closed<ServerRequests[M]["result"]["Type"]> } }[ServerRequestMethod]
  | { readonly error: RpcErrorBody };

/** Where every server request goes (Task 11's `codex/permissions.ts`). */
export interface ServerRequestPort {
  request(r: ServerRequest): Effect.Effect<ServerReply>;
}

/** The app-server's pipes (a `SandboxedChild`'s, or a test's). */
export interface RpcIo {
  readonly stdin: Pick<Writable, "write" | "end" | "on">;
  readonly stdout: Pick<Readable, "on">;
}

export interface CodexRpc {
  request<M extends ClientMethod>(m: M, p: ParamsOf<M>): Effect.Effect<ResultOf<M>, FinError>;
  /**
   * As `request`, with the result also as Codex sent it: the generated schemas keep only the keys
   * they list (`Config` lists 25 of `config/read`'s), and the capability check (Task 8) reads
   * others (`features`, `project_doc_max_bytes`, an unknown managed requirement).
   */
  requestRaw<M extends ClientMethod>(m: M, p: ParamsOf<M>): Effect.Effect<{ readonly result: ResultOf<M>; readonly raw: unknown }, FinError>;
  notify(m: (typeof P.CLIENT_NOTIFICATIONS)[number]): Effect.Effect<void>;
  /** The server's notifications in order; ends at its EOF, fails `PROVIDER_PROTOCOL` on drift. */
  readonly notifications: Stream.Stream<ServerNotification, FinError>;
  /** Starts answering server requests (those that came before are dispatched now). */
  serve(port: ServerRequestPort): void;
  /** Closes the server's stdin (it exits 0, Task 0); pending requests fail at its EOF. */
  end(): void;
}

/** A routed line. */
export type Incoming =
  | { readonly kind: "response"; readonly id: P.RequestId; readonly result: unknown }
  | { readonly kind: "error"; readonly id: P.RequestId | null; readonly error: RpcErrorBody }
  | { readonly kind: "notification"; readonly notification: ServerNotification }
  | { readonly kind: "request"; readonly request: ServerRequest };

// ---- decoding (never throws) ----

const name64 = (s: string): string => s.slice(0, 64);
const plain = (u: unknown): u is Record<string, unknown> => typeof u === "object" && u !== null && !Array.isArray(u);
const isId = (u: unknown): u is P.RequestId => typeof u === "string" || (typeof u === "number" && Number.isSafeInteger(u));
const protocol = (message: string, details: Record<string, unknown>) => finError("PROVIDER_PROTOCOL", message, details);

function decodeWith<A>(schema: Schema.Top, raw: unknown, what: string, method: string): A | FinError {
  try {
    const r = Schema.decodeUnknownResult(schema as Schema.Codec<A>)(raw);
    if (r._tag === "Success") return r.success;
    return protocol(`a Codex ${what} does not match the ${P.CODEX_PROTOCOL_VERSION} protocol`, { method, issue: String(r.failure).slice(0, 500) });
  } catch (e) {
    // a stack overflow on pathological nesting
    return protocol(`a Codex ${what} could not be decoded`, { method, issue: String(e).slice(0, 200) });
  }
}

/** The result of `method` (an allowed one), decoded; a missing needed field is `PROVIDER_PROTOCOL`. */
export function decodeResult<M extends AllowedMethod>(method: M, raw: unknown): ResultOf<M> | FinError {
  if (!Object.hasOwn(P.CLIENT_REQUESTS, method)) return protocol("a result for a method Finisher does not send", { method: name64(String(method)) });
  return decodeWith<ResultOf<M>>(P.CLIENT_REQUESTS[method].result, raw, `${method} result`, method);
}

/** A server notification: decoded when the adapter uses it, else `unknown`. */
export function decodeNotification(method: string, params: unknown): ServerNotification | FinError {
  if (!Object.hasOwn(P.SERVER_NOTIFICATIONS, method)) return { method: "unknown", name: name64(method), params };
  const m = method as keyof Notifications;
  const p = decodeWith<KnownNotification["params"]>(P.SERVER_NOTIFICATIONS[m], params, `${method} notification`, method);
  return p instanceof FinError ? p : ({ method: m, params: p } as KnownNotification);
}

/** A server request: decoded when it is one of the ten, else `unknown`. */
export function decodeServerRequest(id: P.RequestId, method: string, params: unknown): ServerRequest | FinError {
  if (!Object.hasOwn(P.SERVER_REQUESTS, method)) return { id, method: "unknown", name: name64(method), params };
  const m = method as ServerRequestMethod;
  const p = decodeWith<KnownServerRequest["params"]>(P.SERVER_REQUESTS[m].params, params, `${method} request`, method);
  return p instanceof FinError ? p : ({ id, method: m, params: p } as KnownServerRequest);
}

/** One line from the server, routed (after T3's `routeMessage`) and decoded. */
export function decodeIncoming(line: string): Incoming | FinError {
  let msg: unknown;
  try {
    msg = JSON.parse(line);
  } catch {
    return protocol("a Codex line is not JSON", { length: line.length });
  }
  if (!plain(msg)) return protocol("a Codex line is not a JSON-RPC object", {});
  if ("method" in msg) {
    if (typeof msg.method !== "string") return protocol("a Codex message has a method that is not a string", {});
    if ("id" in msg) {
      if (!isId(msg.id)) return protocol("a Codex request has an id that is not a string or an integer", { method: name64(msg.method) });
      const r = decodeServerRequest(msg.id, msg.method, msg.params);
      // the id stays with the error, so the client can refuse the request before it stops
      return r instanceof FinError ? finError(r.code, r.message, { ...(r.details as object), requestId: msg.id }) : { kind: "request", request: r };
    }
    const n = decodeNotification(msg.method, msg.params);
    return n instanceof FinError ? n : { kind: "notification", notification: n };
  }
  if ("error" in msg) {
    const e = msg.error;
    if (!(msg.id === null || isId(msg.id)) || !plain(e) || !Number.isSafeInteger(e.code) || typeof e.message !== "string")
      return protocol("a Codex error response is malformed", {});
    return { kind: "error", id: msg.id, error: { code: e.code as number, message: e.message.slice(0, 2000) } };
  }
  if ("result" in msg && isId(msg.id)) return { kind: "response", id: msg.id, result: msg.result };
  return protocol("a Codex line is neither a request, a notification nor a response", { keys: Object.keys(msg).slice(0, 8).map(name64) });
}

// ---- the client ----

interface Pending {
  readonly method: AllowedMethod;
  /** Resume with `{result, raw}` (`requestRaw`). */
  readonly raw: boolean;
  readonly resume: (e: Effect.Effect<never, FinError> | Effect.Effect<unknown, FinError>) => void;
}

/** Starts reading `io.stdout` at once; nothing is written until a call. */
export function codexRpc(io: RpcIo, o: { readonly maxLineBytes?: number } = {}): CodexRpc {
  return new Rpc(io, o.maxLineBytes ?? MAX_LINE_BYTES);
}

const exited = (method: string, reason: "eof" | "stdinClosed") =>
  finError("PROVIDER_EXITED", reason === "eof" ? `Codex exited before it answered ${method}` : `Codex's input is closed: ${method} was not sent`, { method, reason });

class Rpc implements CodexRpc {
  readonly #io: RpcIo;
  readonly #queue: Queue.Queue<ServerNotification, FinError | Cause.Done>;
  readonly #pending = new Map<number, Pending>();
  readonly #handlers = new Map<string, Fiber.Fiber<ServerReply>>();
  readonly #held: ServerRequest[] = [];
  readonly notifications: Stream.Stream<ServerNotification, FinError>;
  #nextId = 1;
  #port: ServerRequestPort | null = null;
  /** Why the client stopped: the server's EOF, or the protocol error that stopped it. */
  #stopped: "eof" | FinError | null = null;
  #stdinEnded = false;

  constructor(io: RpcIo, maxLineBytes: number) {
    this.#io = io;
    this.#queue = Effect.runSync(Queue.unbounded<ServerNotification, FinError | Cause.Done>());
    this.notifications = Stream.fromQueue(this.#queue);
    const framer = lineFramer(maxLineBytes, "codex");
    const take = (lines: (string | FinError)[]) => {
      for (const l of lines) {
        if (this.#stopped !== null) return;
        if (l instanceof FinError) this.#fail(l);
        else this.#route(decodeIncoming(l));
      }
    };
    io.stdout.on("data", (b: Buffer | string) => take(framer.push(b)));
    io.stdout.on("end", () => {
      take(framer.end());
      this.#eof();
    });
    io.stdout.on("error", () => this.#eof());
    io.stdin.on("error", () => 0);
  }

  request<M extends ClientMethod>(m: M, p: ParamsOf<M>): Effect.Effect<ResultOf<M>, FinError> {
    return this.#call(m, p, false) as Effect.Effect<ResultOf<M>, FinError>;
  }

  requestRaw<M extends ClientMethod>(m: M, p: ParamsOf<M>): Effect.Effect<{ readonly result: ResultOf<M>; readonly raw: unknown }, FinError> {
    return this.#call(m, p, true) as Effect.Effect<{ readonly result: ResultOf<M>; readonly raw: unknown }, FinError>;
  }

  #call<M extends ClientMethod>(m: M, p: ParamsOf<M>, raw: boolean): Effect.Effect<unknown, FinError> {
    return Effect.suspend(() => {
      if (!ALLOWED_METHODS.has(m) || !Object.hasOwn(P.CLIENT_REQUESTS, m))
        return Effect.fail(protocol(`Finisher never sends ${JSON.stringify(name64(String(m)))} to Codex`, { method: name64(String(m)), reason: "methodNotAllowed" }));
      const method = m as AllowedMethod;
      const stopped = this.#stopped;
      if (stopped !== null) return Effect.fail(stopped === "eof" ? exited(method, "eof") : stopped);
      if (this.#stdinEnded) return Effect.fail(exited(method, "stdinClosed"));
      return Effect.callback<unknown, FinError>((resume) => {
        const id = this.#nextId++;
        this.#pending.set(id, { method, raw, resume: resume as Pending["resume"] });
        this.#write({ id, method, ...(p === undefined ? {} : { params: p }) }, (ok) => {
          if (ok || !this.#pending.delete(id)) return;
          resume(Effect.fail(exited(method, "stdinClosed")));
        });
        return Effect.sync(() => void this.#pending.delete(id));
      });
    });
  }

  notify(m: (typeof P.CLIENT_NOTIFICATIONS)[number]): Effect.Effect<void> {
    return Effect.sync(() => {
      if (this.#stopped === null && !this.#stdinEnded) this.#write({ method: m });
    });
  }

  serve(port: ServerRequestPort): void {
    this.#port = port;
    for (const r of this.#held.splice(0)) this.#dispatch(r);
  }

  end(): void {
    if (this.#stdinEnded) return;
    this.#stdinEnded = true;
    try {
      this.#io.stdin.end();
    } catch {
      // already destroyed: its EOF follows
    }
  }

  #write(msg: Record<string, unknown>, done?: (ok: boolean) => void): void {
    try {
      this.#io.stdin.write(`${JSON.stringify(msg)}\n`, (e) => done?.(e === null || e === undefined));
    } catch {
      done?.(false);
    }
  }

  #route(m: Incoming | FinError): void {
    if (m instanceof FinError) return this.#fail(m);
    switch (m.kind) {
      case "response":
      case "error": {
        const id = m.id;
        if (typeof id !== "number" || id < 1 || id >= this.#nextId)
          return this.#fail(protocol("Codex answered a request Finisher never sent", { id: typeof id === "string" ? name64(id) : id }));
        const p = this.#pending.get(id);
        if (p === undefined) return; // abandoned (its caller was interrupted)
        this.#pending.delete(id);
        if (m.kind === "error") {
          p.resume(Effect.fail(protocol(`Codex refused ${p.method}: ${m.error.message.slice(0, 200)}`, { method: p.method, reason: "rpcError", rpcError: m.error })));
          return;
        }
        const r = decodeResult(p.method, m.result);
        p.resume(r instanceof FinError ? Effect.fail(r) : Effect.succeed(p.raw ? { result: r, raw: m.result } : r));
        return;
      }
      case "notification": {
        const n = m.notification;
        // Codex settles a request itself when its turn ends: a handler still running stops.
        if (n.method === "serverRequest/resolved") this.#handlers.get(JSON.stringify(n.params.requestId))?.interruptUnsafe();
        Queue.offerUnsafe(this.#queue, n);
        return;
      }
      case "request":
        if (this.#port === null) this.#held.push(m.request);
        else this.#dispatch(m.request);
        return;
    }
  }

  #dispatch(r: ServerRequest): void {
    const key = JSON.stringify(r.id);
    const fiber = Effect.runFork(this.#port!.request(r));
    this.#handlers.set(key, fiber);
    fiber.addObserver((exit) => {
      if (this.#handlers.get(key) === fiber) this.#handlers.delete(key);
      if (this.#stopped !== null) return;
      if (Exit.isSuccess(exit)) this.#answer(r, exit.value);
      else if (!Cause.hasInterruptsOnly(exit.cause)) this.#write({ id: r.id, error: { code: -32603, message: "Finisher failed to answer this request" } });
    });
  }

  #answer(r: ServerRequest, reply: ServerReply): void {
    if ("error" in reply) return this.#write({ id: r.id, error: { code: reply.error.code, message: reply.error.message } });
    if (r.method === "unknown" || reply.method !== r.method)
      return this.#write({ id: r.id, error: { code: -32603, message: `Finisher has no answer for ${r.method === "unknown" ? r.name : r.method}` } });
    this.#write({ id: r.id, result: reply.result });
  }

  /** Drift: every pending request and the stream fail with `e`; nothing more is read. */
  #fail(e: FinError): void {
    if (this.#stopped !== null) return;
    // a server request that did not decode is refused before the client stops
    const requestId = (e.details as { requestId?: unknown } | null)?.requestId;
    if (isId(requestId)) this.#write({ id: requestId, error: { code: -32602, message: e.message } });
    this.#stop(e);
    Queue.failCauseUnsafe(this.#queue, Cause.fail(e));
  }

  #eof(): void {
    if (this.#stopped !== null) return;
    this.#stop("eof");
    Queue.endUnsafe(this.#queue);
  }

  #stop(why: "eof" | FinError): void {
    this.#stopped = why;
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const p of pending) p.resume(Effect.fail(why === "eof" ? exited(p.method, "eof") : why));
    for (const f of this.#handlers.values()) f.interruptUnsafe();
    this.#handlers.clear();
    this.#held.length = 0;
  }
}
