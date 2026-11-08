// The Windows pipe transport (5A Task 11, Decision 38, spec §4.3). fin-node's native
// `PipeServer` owns the pipe: its security descriptor (this user and SYSTEM), remote clients
// rejected, `FILE_FLAG_FIRST_PIPE_INSTANCE` (a squatter fails the start with `PIPE_SQUATTED`)
// and each client's process and token checked before its first byte. Every connection it
// accepts becomes a Node `Duplex` that Effect's `NodeSocket.fromDuplex` turns into a `Socket`,
// so the protocol (`protocol.ts`: the frame cap before decoding, `INVALID_REQUEST`, the
// `Link` whose `terminate` answers the open requests and then closes) is the Unix socket's.
import { Duplex } from "node:stream";
import * as NodeSocket from "@effect/platform-node/NodeSocket";
import { PipeServer, type PipeEvent } from "@finisher/fin-node";
import { Cause, Context, Effect, Exit, Fiber, Scope } from "effect";
import * as NetAddress from "effect/net/NetAddress";
import type * as Socket from "effect/socket/Socket";
import { SocketServer } from "effect/socket/SocketServer";
import { ServerStartError, startFailure } from "./config.ts";
import { ConnectionPeer } from "./connections.ts";

/** The most a connection buffers here before its reads wait (the native read size). */
const HIGH_WATER_MARK = 64 * 1024;
/**
 * Replies buffered before `write` asks its caller to wait. Each native write is a round trip
 * to the listener's thread, so replies queue up faster than on a socket; `_writev` sends what
 * queued in one write.
 */
const WRITE_HIGH_WATER_MARK = 1024 * 1024;

/**
 * One pipe connection as a `Duplex`: `_read` pulls the next chunk from the native listener
 * (one read at a time, so nothing piles up above Node's buffer); `_write` and `_writev` resolve
 * once the bytes are in the pipe; `_destroy` closes the connection after the client has read them.
 */
export class PipeStream extends Duplex {
  readonly #server: Pick<NativePipe, "read" | "write" | "close">;
  readonly id: number;

  constructor(server: Pick<NativePipe, "read" | "write" | "close">, id: number) {
    super({ allowHalfOpen: false, readableHighWaterMark: HIGH_WATER_MARK, writableHighWaterMark: WRITE_HIGH_WATER_MARK });
    this.#server = server;
    this.id = id;
  }

  override _read(): void {
    this.#server.read(this.id).then(
      (chunk) => {
        if (!this.destroyed) this.push(chunk);
      },
      (e: unknown) => this.destroy(e as Error),
    );
  }

  override _write(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void {
    this.#server.write(this.id, chunk).then(() => callback(), callback);
  }

  override _writev(chunks: Array<{ chunk: Buffer }>, callback: (error?: Error | null) => void): void {
    const data = chunks.length === 1 ? chunks[0]!.chunk : Buffer.concat(chunks.map((c) => c.chunk));
    this.#server.write(this.id, data).then(() => callback(), callback);
  }

  override _destroy(error: Error | null, callback: (error?: Error | null) => void): void {
    this.#server.close(this.id);
    callback(error);
  }
}

/** What `pipe.ts` uses of the native listener (tests pass a fake: `native`). */
export type NativePipe = Pick<PipeServer, "onEvent" | "read" | "write" | "close" | "shutdown">;

export interface PipeServerOptions {
  /** SIDs allowed beside this user (sub-project 7). */
  readonly allow?: ReadonlyArray<string>;
  readonly maxConnections?: number;
  /** A client the listener disconnected before reading from it (for the audit log). */
  readonly onRefused?: (refusal: Extract<PipeEvent, { type: "refused" }>) => void;
  /** Makes the listener (default: fin-node's `PipeServer`); tests drive a fake through it. */
  readonly native?: (name: string, options: ConstructorParameters<typeof PipeServer>[1]) => NativePipe;
}

/** `PIPE_SQUATTED` (exit 1) with a fix in words, or the start failure the error is. */
export function pipeStartError(name: string, e: unknown): ServerStartError {
  if ((e as { code?: unknown }).code !== "PIPE_SQUATTED") return startFailure(e);
  return new ServerStartError(
    "PIPE_SQUATTED",
    `another process already holds the pipe ${name}`,
    { pipe: name },
    "Another program has taken the name of this Finisher server's pipe. Close that program or restart the computer, then start the Finisher server again.",
  );
}

/**
 * A `SocketServer` over the native pipe `name`. Each connection's handler sees its client's pid
 * and SID as `ConnectionPeer`. Closing the scope shuts the listener down (every connection, and
 * the name freed).
 */
export const makePipeSocketServer = (name: string, options: PipeServerOptions = {}) =>
  Effect.gen(function* () {
    const native = yield* Effect.acquireRelease(
      Effect.try({
        try: () =>
          (options.native ?? ((n, o) => new PipeServer(n, o)))(name, {
            ...(options.allow === undefined ? {} : { allow: [...options.allow] }),
            ...(options.maxConnections === undefined ? {} : { maxConnections: options.maxConnections }),
          }),
        catch: (e) => pipeStartError(name, e),
      }),
      (server) => Effect.sync(() => server.shutdown()),
    );
    type Accepted = Extract<PipeEvent, { type: "connection" }>;
    const pending: Accepted[] = [];
    let onConnection = (a: Accepted) => void pending.push(a);
    native.onEvent((event) => {
      if (event.type === "connection") onConnection(event);
      else options.onRefused?.(event);
    });

    const run = <R, E, _>(handler: (socket: Socket.Socket) => Effect.Effect<_, E, R>) =>
      Effect.gen(function* () {
        const scope = yield* Scope.make();
        const services = Context.omit(Scope.Scope)(yield* Effect.context<R>());
        const trackFiber = Fiber.runIn(scope);
        const previous = onConnection;
        onConnection = ({ id, pid, sid }) => {
          const socket = NodeSocket.fromDuplex(
            Effect.acquireRelease(
              Effect.sync(() => new PipeStream(native, id)),
              (stream) =>
                Effect.sync(() => {
                  // Written replies go out first (`end`), then the connection closes.
                  if (!stream.destroyed) stream.end(() => stream.destroy());
                }),
            ),
          );
          const context = Context.add(services, ConnectionPeer, { pid, sid }) as Context.Context<unknown>;
          trackFiber(
            Effect.runForkWith(context)(
              Effect.catchCause(Effect.flatMap(socket, handler), (cause) =>
                Cause.hasInterruptsOnly(cause) ? Effect.void : Effect.logWarning("pipe connection failed", cause),
              ) as Effect.Effect<unknown, never, unknown>,
            ),
          );
        };
        for (const a of pending.splice(0)) onConnection(a);
        return yield* Effect.callback<never>(() =>
          Effect.sync(() => {
            onConnection = previous;
          }),
        ).pipe(Effect.ensuring(Scope.close(scope, Exit.void)));
      });

    return SocketServer.of({ address: NetAddress.unixPathAddress(name), run });
  });
