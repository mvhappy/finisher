// RPC transports (plan decision 5): NDJSON over the local socket/pipe and over a
// bearer-token WebSocket on 127.0.0.1.
import { timingSafeEqual } from "node:crypto";
import { chmodSync, existsSync, rmSync } from "node:fs";
import type { IncomingMessage } from "node:http";
import { connect } from "node:net";
import * as NodeSocketServer from "@effect/platform-node/NodeSocketServer";
import { FinRpcs } from "@finisher/contracts";
import { Effect, Layer } from "effect";
import { RpcSerialization, RpcServer } from "effect/rpc";
import { SocketServer } from "effect/socket/SocketServer";
import { ServerStartError } from "./config.ts";

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
        : Effect.sync(() => {
            if (platform !== "win32") rmSync(path, { force: true });
          }),
  );

/** True when `req` carries `Authorization: Bearer <token>` (constant-time comparison). */
export function hasBearer(req: Pick<IncomingMessage, "headers">, token: string): boolean {
  const header = req.headers.authorization;
  if (typeof header !== "string" || !header.startsWith("Bearer ")) return false;
  const given = Buffer.from(header.slice("Bearer ".length));
  const expected = Buffer.from(token);
  return given.length === expected.length && timingSafeEqual(given, expected);
}

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
          (e.reason as { cause?: { code?: unknown } }).cause?.code === "EADDRINUSE" ? serverRunning(path) : e,
        ),
        Effect.tap(() =>
          Effect.sync(() => {
            if (platform !== "win32") chmodSync(path, 0o600);
          }),
        ),
      ),
    (previous) =>
      Effect.sync(() => {
        if (previous !== undefined) process.umask(previous);
      }),
  );

/**
 * WebSocket server on `127.0.0.1:0`. Requests without the bearer token get HTTP 401
 * before the upgrade (ws `verifyClient`).
 */
export const makeWebSocketServer = (token: string) =>
  NodeSocketServer.makeWebSocket({
    host: "127.0.0.1",
    port: 0,
    verifyClient: (info: { req: IncomingMessage }, cb: (ok: boolean, code?: number, message?: string) => void) => {
      if (hasBearer(info.req, token)) cb(true);
      else cb(false, 401, "Unauthorized");
    },
  });

/** The port a WebSocket `SocketServer` is bound to. */
export function portOf(server: SocketServer["Service"]): number {
  const address = server.address;
  if ("port" in address) return address.port;
  throw new Error("websocket server is not bound to an inet address");
}

/** `FinRpcs` served over one `SocketServer` with NDJSON framing. */
export function serveRpc(server: SocketServer["Service"]) {
  return RpcServer.layer(FinRpcs).pipe(
    // `fresh`: layers are memoized by reference, and each transport needs its own protocol.
    Layer.provide(Layer.fresh(RpcServer.layerProtocolSocketServer)),
    Layer.provide(Layer.succeed(SocketServer, server)),
    Layer.provide(RpcSerialization.layerNdjson),
  );
}
