// finisherd's Effect RPC server protocol over a `SocketServer` (spec §4.3, plan 5A Decisions 12
// and 13): NDJSON `Request`/`Ack`/`Interrupt`/`Ping`/`Eof` in, `Exit`/`Chunk`/`Pong` out, as
// Effect's `makeProtocolSocketServer`, plus what that one cannot do:
// - inbound lines are capped (`MAX_FRAME`): a longer line answers `FRAME_TOO_LARGE` (to the
//   request id at its start, when one can be read) and closes the connection;
// - a frame that is not JSON, not a known message, names an unknown RPC or carries a payload its
//   schema refuses answers `INVALID_REQUEST` from here, never a defect; a frame with no request
//   id to answer closes the connection;
// - client headers with the reserved `x-fin-` prefix are dropped;
// - every connection is registered in `Connections` with a `Link` that can close it, and dropped
//   before its id reaches the server's `disconnects` queue.
import type { FinError } from "@finisher/contracts";
import { Context, Deferred, Effect, Exit, Option, Queue, Schema, Scope } from "effect";
import { Rpc, type RpcGroup, RpcSerialization, RpcServer } from "effect/rpc";
import type { Socket } from "effect/socket/Socket";
import { SocketServer } from "effect/socket/SocketServer";
import { ConnectionPeer, type Connections } from "./connections.ts";
import { finError } from "./errors.ts";
import { UpgradePrincipal } from "./upgrade.ts";

/** The longest line either side reads: 16 MiB (spec §4.3). */
export const MAX_FRAME = 16 * 1024 * 1024;

/** Client header names starting with this are finisherd's own and never taken from a client. */
export const RESERVED_HEADER_PREFIX = "x-fin-";

const ID_AT_START = /"id"\s*:\s*(?:"((?:[^"\\]|\\.){1,128})"|(\d{1,20}))/;

/** The request id near the start of a frame that could not be parsed (best effort). */
export function salvageId(head: string): string | undefined {
  const m = ID_AT_START.exec(head.slice(0, 4096));
  return m === null ? undefined : (m[1] ?? m[2]);
}

/** `exit` of a request that failed with `error`, as the wire carries it. */
export function failureExit(requestId: string, error: FinError): Record<string, unknown> {
  const { code, message, details, fix } = error;
  return {
    _tag: "Exit",
    requestId,
    exit: { _tag: "Failure", cause: [{ _tag: "Fail", error: { _tag: "FinError", code, message, details, ...(fix === undefined ? {} : { fix }) } }] },
  };
}

/** Splits a byte stream into lines of at most `max` bytes (the `\n` excluded). */
export class LineSplitter {
  #parts: Buffer[] = [];
  #size = 0;
  readonly max: number;
  constructor(max: number = MAX_FRAME) {
    this.max = max;
  }

  /** The complete lines in `chunk`, or `tooLarge` with the over-long line's first bytes. */
  push(chunk: Uint8Array | string): { lines: string[]; tooLarge?: string } {
    const buf = typeof chunk === "string" ? Buffer.from(chunk, "utf8") : Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    const lines: string[] = [];
    let start = 0;
    for (;;) {
      const nl = buf.indexOf(10, start);
      const end = nl < 0 ? buf.length : nl;
      if (this.#size + (end - start) > this.max) {
        const head = Buffer.concat([...this.#parts, buf.subarray(start, Math.min(end, start + 4096))]).subarray(0, 4096);
        this.#parts = [];
        this.#size = 0;
        return { lines, tooLarge: head.toString("utf8") };
      }
      if (nl < 0) {
        if (end > start) {
          this.#parts.push(Buffer.from(buf.subarray(start, end)));
          this.#size += end - start;
        }
        return { lines };
      }
      const line =
        this.#parts.length === 0 ? buf.toString("utf8", start, end) : Buffer.concat([...this.#parts, buf.subarray(start, end)]).toString("utf8");
      this.#parts = [];
      this.#size = 0;
      lines.push(line);
      start = nl + 1;
    }
  }
}

/** Drops the reserved `x-fin-` headers; `undefined` when `headers` is not a list of string pairs. */
export function clientHeaders(headers: unknown): Array<[string, string]> | undefined {
  if (headers === undefined) return [];
  if (!Array.isArray(headers)) return undefined;
  const out: Array<[string, string]> = [];
  for (const h of headers) {
    if (!Array.isArray(h) || h.length !== 2 || typeof h[0] !== "string" || typeof h[1] !== "string") return undefined;
    if (!h[0].toLowerCase().startsWith(RESERVED_HEADER_PREFIX)) out.push([h[0], h[1]]);
  }
  return out;
}

const isId = (v: unknown): v is string | number => typeof v === "string" || (typeof v === "number" && Number.isSafeInteger(v));

type Decode = (u: unknown) => Exit.Exit<unknown, Schema.SchemaError>;

interface Client {
  readonly write: (frame: unknown) => Effect.Effect<void>;
  readonly closed: Deferred.Deferred<void>;
  /** Requests sent and not yet answered (their `Exit` not written). */
  readonly open: Set<string>;
  closing: boolean;
  /** Run once the next reply has been written (`Link.afterReply`). */
  readonly after: Array<() => void>;
}

export interface FinProtocolOptions {
  readonly maxFrame?: number;
}

/**
 * The protocol for `group` over the current `SocketServer`, registering connections in
 * `connections`. Payloads are decoded here with the same codec `RpcServer` uses (the
 * serialization's `codecFor` and the RPC's parse options), so the server's own decode cannot fail.
 */
export const makeFinProtocol = <R extends Rpc.Any>(group: RpcGroup.RpcGroup<R>, connections: Connections, options: FinProtocolOptions = {}) =>
  Effect.gen(function* () {
    const server = yield* SocketServer;
    const serialization = yield* RpcSerialization.RpcSerialization;
    const disconnects = yield* Queue.make<number>();
    const max = options.maxFrame ?? MAX_FRAME;
    const clients = new Map<number, Client>();
    const decoders = new Map<string, Decode>();
    let nextId = 0;
    let writeRequest!: (clientId: number, message: unknown) => Effect.Effect<void>;

    const decoderFor = (tag: string): Decode | undefined => {
      let d = decoders.get(tag);
      if (d === undefined) {
        const rpc = group.requests.get(tag) as unknown as Rpc.AnyWithProps | undefined;
        if (rpc === undefined) return undefined;
        const parse = Context.get(rpc.annotations, Rpc.ParseOptions);
        const codec = serialization.codecFor(rpc.payloadSchema) as Schema.Codec<unknown, unknown, never, never>;
        d = Schema.decodeUnknownExit(codec, parse);
        decoders.set(tag, d);
      }
      return d;
    };

    const invalid = (client: Client, id: string | undefined, message: string, details: Record<string, unknown> = {}) => {
      if (id === undefined) {
        Deferred.doneUnsafe(client.closed, Exit.void);
        return Effect.void;
      }
      return client.write(failureExit(id, finError("INVALID_REQUEST", message, details, "use fin and finisherd from the same Finisher release")));
    };

    /** Checks one line and hands it to the server loop. */
    const onLine = (clientId: number, client: Client, line: string): Effect.Effect<void> => {
      if (line.trim().length === 0) return Effect.void;
      let msg: Record<string, unknown>;
      try {
        const parsed: unknown = JSON.parse(line);
        if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return invalid(client, undefined, "a frame is a JSON object");
        msg = parsed as Record<string, unknown>;
      } catch {
        return invalid(client, salvageId(line), "the frame is not valid JSON");
      }
      switch (msg._tag) {
        case "Request": {
          if (!isId(msg.id)) return invalid(client, undefined, "a request needs an id");
          const id = String(msg.id);
          const tag = typeof msg.tag === "string" ? msg.tag : "";
          const decode = decoderFor(tag);
          if (decode === undefined) return invalid(client, id, `unknown RPC ${JSON.stringify(tag).slice(0, 80)}`, { tag: tag.slice(0, 80) });
          const headers = clientHeaders(msg.headers);
          if (headers === undefined) return invalid(client, id, "request headers are [name, value] string pairs", { tag });
          const decoded = decode(msg.payload);
          if (Exit.isFailure(decoded)) {
            const error = Exit.findErrorOption(decoded);
            const text = Option.isSome(error) ? error.value.message : "the payload does not match the RPC's schema";
            return invalid(client, id, `invalid ${tag} payload: ${text.slice(0, 500)}`, { tag });
          }
          client.open.add(id);
          return writeRequest(clientId, { ...msg, headers });
        }
        case "Ack":
        case "Interrupt":
          return isId(msg.requestId) ? writeRequest(clientId, msg) : invalid(client, undefined, "an Ack or Interrupt needs a requestId");
        case "Ping":
        case "Eof":
          return writeRequest(clientId, msg);
        default:
          return invalid(client, isId(msg.id) ? String(msg.id) : undefined, `unknown frame ${JSON.stringify(msg._tag)}`);
      }
    };

    const onSocket = (socket: Socket) =>
      Effect.gen(function* () {
        const scope = yield* Effect.scope;
        const clientId = nextId++;
        const writer = yield* socket.writer;
        const closed = Deferred.makeUnsafe<void>();
        const client: Client = {
          write: (frame) => Effect.orDie(writer.write(`${JSON.stringify(frame)}\n`)),
          closed,
          open: new Set(),
          closing: false,
          after: [],
        };
        const close = () => Deferred.doneUnsafe(closed, Exit.void);
        // A WebSocket carries its ticket's principal (`upgrade.ts`); a socket starts anonymous.
        const ticketed = Option.getOrUndefined(yield* Effect.serviceOption(UpgradePrincipal));
        // A pipe connection carries its client's pid and token SID (`pipe.ts`).
        const peer = Option.getOrUndefined(yield* Effect.serviceOption(ConnectionPeer));
        clients.set(clientId, client);
        connections.attach(clientId, {
          close,
          closeAfterReply: () => {
            client.closing = true;
          },
          afterReply: (f) => void client.after.push(f),
          terminate: (error) =>
            Effect.runFork(
              Effect.ensuring(
                Effect.forEach([...client.open], (id) => client.write(failureExit(id, error)), { discard: true }),
                Effect.sync(close),
              ),
            ),
        }, ticketed, peer);
        yield* Scope.addFinalizer(
          scope,
          Effect.suspend(() => {
            clients.delete(clientId);
            connections.close(clientId);
            return Queue.offer(disconnects, clientId);
          }),
        );
        const lines = new LineSplitter(max);
        const read = Effect.gen(function* () {
          const { pull } = yield* socket.reader;
          for (;;) {
            const chunks = yield* pull;
            for (const chunk of chunks) {
              const { lines: got, tooLarge } = lines.push(chunk);
              for (const line of got) {
                // A close flagged by an earlier line (five failed proves, a frame with no id)
                // stops this connection here: the rest of the chunk is not read.
                if (Deferred.isDoneUnsafe(closed)) return;
                if (client.closing) return yield* Deferred.await(closed);
                yield* onLine(clientId, client, line);
              }
              if (tooLarge !== undefined) {
                const id = salvageId(tooLarge);
                if (id !== undefined) {
                  client.open.delete(id);
                  yield* client.write(
                    failureExit(id, finError("FRAME_TOO_LARGE", `a frame is longer than ${max} bytes`, { max }, "send smaller requests")),
                  );
                }
                return;
              }
              if (Deferred.isDoneUnsafe(closed)) return;
              if (client.closing) return yield* Deferred.await(closed);
            }
          }
        }).pipe(Effect.catchReason("SocketError", "SocketCloseError", () => Effect.void), Effect.orDie);
        yield* Effect.raceFirst(read, Deferred.await(closed));
      });

    yield* Effect.forkScoped(server.run((socket) => Effect.scoped(onSocket(socket))));

    return yield* RpcServer.Protocol.make((write) => {
      writeRequest = write as (clientId: number, message: unknown) => Effect.Effect<void>;
      return Effect.succeed({
        disconnects,
        send: (clientId: number, response: { _tag: string; requestId?: unknown }) => {
          const client = clients.get(clientId);
          if (client === undefined) return Effect.void;
          if (response._tag !== "Exit") return client.write(response);
          client.open.delete(String(response.requestId));
          const after = client.after.splice(0);
          const written = client.write(response);
          const sent = after.length === 0 ? written : Effect.ensuring(written, Effect.sync(() => after.forEach((f) => f())));
          return client.closing ? Effect.ensuring(sent, Effect.sync(() => Deferred.doneUnsafe(client.closed, Exit.void))) : sent;
        },
        end: () => Effect.void,
        clientIds: Effect.sync(() => new Set(clients.keys())),
        initialMessage: Effect.succeedNone,
        supportsAck: true,
        supportsTransferables: false,
        supportsSpanPropagation: true,
        supportsNotifications: true,
        codecFor: serialization.codecFor,
      } as unknown as Omit<RpcServer.Protocol["Service"], "run">);
    });
  });
