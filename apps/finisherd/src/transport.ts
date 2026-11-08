// RPC transports (plan decision 5): NDJSON over the local socket/pipe and over a ticketed
// WebSocket on 127.0.0.1 (`upgrade.ts`, 5A Task 6).
import { chmodSync, existsSync, rmSync } from "node:fs";
import { connect } from "node:net";
import * as NodeSocketServer from "@effect/platform-node/NodeSocketServer";
import { FinRpcs } from "@finisher/contracts";
import { Effect, Layer } from "effect";
import { RpcSerialization, RpcServer } from "effect/rpc";
import { SocketServer } from "effect/socket/SocketServer";
import { ServerStartError, startFailure } from "./config.ts";
import { type Connections, ConnectionsTag } from "./connections.ts";
import { RpcSource, type Source } from "./diagnostics.ts";
import { HandlersLive } from "./handlers.ts";
import { makeFinProtocol } from "./protocol.ts";

/** One server per server-data: a live finisherd already owns the socket. */
function serverRunning(socket: string): ServerStartError {
  return new ServerStartError("SERVER_RUNNING", `another finisherd is already serving ${socket}`);
}

/** Resolves true if something accepts a connection on `path` within `timeoutMs`. */
export function isLive(path: string, timeoutMs = 1000): Promise<boolean> {
  return new Promise((resolve) => {
    const conn = connect({ path });
    const done = (live: boolean) => {
      clearTimeout(timer);
      conn.destroy();
      resolve(live);
    };
    const timer = setTimeout(() => done(false), timeoutMs);
    conn.once("connect", () => done(true));
    conn.once("error", () => done(false));
  });
}

/**
 * Makes `path` available for listening: fails if a live server answers on it, otherwise
 * removes a stale Unix socket file. Named pipes vanish with their owner, so only the
 * liveness check applies on Windows.
 */
export const claimSocketPath = (
  path: string,
  platform: NodeJS.Platform = process.platform,
): Effect.Effect<void, ServerStartError> =>
  Effect.flatMap(
    Effect.promise(() => (platform !== "win32" && !existsSync(path) ? Promise.resolve(false) : isLive(path))),
    (live) =>
      live
        ? Effect.fail(serverRunning(path))
        : Effect.try({
            try: () => {
              if (platform !== "win32") rmSync(path, { force: true });
            },
            catch: startFailure,
          }),
  );

/**
 * Local socket server at `path`. The socket is created under umask 0177 (so it is 0600 from
 * the start) and chmod'ed 0600 again once listening; `EADDRINUSE` means `SERVER_RUNNING`.
 */
export const makeLocalSocketServer = (path: string, platform: NodeJS.Platform = process.platform) =>
  Effect.acquireUseRelease(
    Effect.sync(() => (platform === "win32" ? undefined : process.umask(0o177))),
    () =>
      NodeSocketServer.make({ path }).pipe(
        Effect.mapError((e) =>
          (e.reason as { cause?: { code?: unknown } }).cause?.code === "EADDRINUSE"
            ? serverRunning(path)
            : startFailure((e.reason as { cause?: unknown }).cause ?? e),
        ),
        Effect.tap(() =>
          Effect.try({
            try: () => {
              if (platform !== "win32") chmodSync(path, 0o600);
            },
            catch: startFailure,
          }),
        ),
      ),
    (previous) =>
      Effect.sync(() => {
        if (previous !== undefined) process.umask(previous);
      }),
  );

/** Error codes of a peer that went away while the server was reading or writing (Unix, Windows pipes). */
const PEER_GONE = new Set(["ECONNRESET", "EPIPE", "ERR_STREAM_PREMATURE_CLOSE", "ERROR_NO_DATA", "ERROR_BROKEN_PIPE"]);

/**
 * True when every reason of `cause` is an interrupt or a read/write error of a peer that closed its end:
 * `fin` writes its `cli.command` and leaves without reading the reply, so the reply meets a closed socket.
 * That is a normal close, not an error worth a stack trace in the log.
 */
export function isPeerClose(cause: { readonly reasons: ReadonlyArray<unknown> }): boolean {
  const codeOf = (e: unknown): unknown => {
    const err = e as { code?: unknown; cause?: { code?: unknown }; reason?: { cause?: { code?: unknown } } } | null;
    return err?.reason?.cause?.code ?? err?.cause?.code ?? err?.code;
  };
  return (
    cause.reasons.length > 0 &&
    cause.reasons.every((r) => {
      // The protocol's read loop dies with the `SocketError` (a defect), a write fails with it.
      const reason = r as { _tag?: string; error?: unknown; defect?: unknown };
      if (reason._tag === "Interrupt") return true;
      const code = codeOf(reason._tag === "Die" ? reason.defect : reason.error);
      return (reason._tag === "Fail" || reason._tag === "Die") && typeof code === "string" && PEER_GONE.has(code);
    })
  );
}

/** `server` whose connections end quietly when the peer simply goes away ([`isPeerClose`]). */
export function quietPeerClose(server: SocketServer["Service"]): SocketServer["Service"] {
  return SocketServer.of({
    address: server.address,
    run: (handler) =>
      server.run((socket) => Effect.catchCause(handler(socket), (cause) => (isPeerClose(cause) ? Effect.void : Effect.failCause(cause)))),
  });
}

/**
 * `FinRpcs` served over one `SocketServer` with NDJSON framing (`protocol.ts`), by handlers of its
 * own that know the transport's `source` (plan D, Decision 24: the local socket is `fin`, the
 * WebSocket `ui`) and its `connections` (5A Decision 12). `fresh`: layers are memoized by
 * reference, so a shared `HandlersLive` would serve both transports with the first one's source.
 */
export function serveRpc(server: SocketServer["Service"], source: Source, connections: Connections) {
  return RpcServer.layer(FinRpcs).pipe(
    Layer.provide(
      Layer.fresh(HandlersLive).pipe(Layer.provide(Layer.mergeAll(Layer.succeed(RpcSource, source), Layer.succeed(ConnectionsTag, connections)))),
    ),
    // Each transport needs its own protocol.
    Layer.provide(Layer.effect(RpcServer.Protocol)(makeFinProtocol(FinRpcs, connections))),
    Layer.provide(Layer.succeed(SocketServer, source === "fin" ? quietPeerClose(server) : server)),
    Layer.provide(RpcSerialization.layerNdjson),
  );
}
