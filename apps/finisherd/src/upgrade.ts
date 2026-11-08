// The WebSocket upgrade (spec §4.3, plan 5A Decision 24). The bearer token is gone: a client
// offers `Sec-WebSocket-Protocol: finisher.v2, finisher.ticket.<ticket>` with a one-time ticket
// it minted over an authenticated connection (`auth.ticket`). Checked in this order:
//   1. the path is `/rpc` (404);
//   2. `finisher.v2` is offered (426 with `{"protocol":{"min":2,"max":2}}`);
//   3. `Host` is `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` (403): DNS rebinding;
//   4. `Origin` is absent (not a browser), one of those origins, or in `settings.json`
//      `server.origins` (403);
//   5. exactly one ticket is offered and it is live (401); taking it uses it up.
// Host and Origin are checked before the ticket is taken, so a page cannot burn tickets. A
// ticket in the URL (`?ticket=`) is never read: query strings end up in logs.
//
// `ws` runs in `noServer` mode behind our own `node:http` server on `127.0.0.1:0`, so the
// upgrade request is ours; each accepted socket carries its ticket's principal
// (`UpgradePrincipal`) into the protocol (`protocol.ts`).
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { Duplex } from "node:stream";
import { PROTOCOL_VERSION, WS_PROTOCOL_PREFIX, WS_TICKET_PREFIX } from "@finisher/contracts";
import { Context, Effect, Exit, Fiber, Scope } from "effect";
import * as NetAddress from "effect/net/NetAddress";
import * as Socket from "effect/socket/Socket";
import { SocketServer, SocketServerError, SocketServerOpenError } from "effect/socket/SocketServer";
import { WebSocketServer, type WebSocket } from "ws";
import type { Principal } from "./principal.ts";
import type { Tickets } from "./auth/tickets.ts";

export const RPC_PATH = "/rpc";
export const WS_PROTOCOL = `${WS_PROTOCOL_PREFIX}${PROTOCOL_VERSION}` as const;

export type UpgradeFailure = "badPath" | "badVersion" | "badHost" | "badOrigin" | "noTicket" | "badTicket";

export type UpgradeResult =
  | { ok: true; principal: Principal; protocol: typeof WS_PROTOCOL }
  | { ok: false; status: 401 | 403 | 404 | 426; reason: UpgradeFailure; body?: unknown };

export interface UpgradeCheckOptions {
  /** The port the server is bound to (known once it listens). */
  readonly port: () => number;
  readonly tickets: Tickets;
  /** Configured origins (`settings.json` `server.origins`), read at each upgrade. */
  readonly origins: () => ReadonlyArray<string>;
}

/** `origin` as `new URL` normalises it (lower-cased, default port dropped), or `undefined`. */
export function normalOrigin(origin: string): string | undefined {
  try {
    const o = new URL(origin).origin;
    return o === "null" ? undefined : o;
  } catch {
    return undefined;
  }
}

/** The loopback names a local client may use for this server (Host and Origin). */
export function loopbackHosts(port: number): ReadonlyArray<string> {
  return [`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`];
}

/** The offered subprotocols, in order (repeated headers are joined by Node with `, `). */
function offered(req: Pick<IncomingMessage, "headers">): string[] {
  const raw = req.headers["sec-websocket-protocol"];
  const text = Array.isArray(raw) ? raw.join(",") : (raw ?? "");
  return text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p.length > 0);
}

export function makeUpgradeCheck(opts: UpgradeCheckOptions): (req: Pick<IncomingMessage, "headers" | "url">) => UpgradeResult {
  return (req) => {
    let path: string;
    try {
      path = new URL(req.url ?? "", "http://127.0.0.1").pathname;
    } catch {
      path = "";
    }
    if (path !== RPC_PATH) return { ok: false, status: 404, reason: "badPath" };
    const protocols = offered(req);
    if (!protocols.includes(WS_PROTOCOL)) {
      return { ok: false, status: 426, reason: "badVersion", body: { protocol: { min: PROTOCOL_VERSION, max: PROTOCOL_VERSION } } };
    }
    const port = opts.port();
    const hosts = loopbackHosts(port);
    const host = req.headers.host?.toLowerCase();
    if (host === undefined || !hosts.includes(host)) return { ok: false, status: 403, reason: "badHost" };
    const origin = req.headers.origin;
    if (origin !== undefined) {
      const allowed = new Set([...hosts.map((h) => `http://${h}`), ...opts.origins().map(normalOrigin)]);
      const got = normalOrigin(origin);
      if (got === undefined || !allowed.has(got)) return { ok: false, status: 403, reason: "badOrigin" };
    }
    const tickets = protocols.filter((p) => p.startsWith(WS_TICKET_PREFIX));
    if (tickets.length !== 1) return { ok: false, status: 401, reason: "noTicket" };
    const principal = opts.tickets.take(tickets[0]!.slice(WS_TICKET_PREFIX.length));
    if (principal === undefined) return { ok: false, status: 401, reason: "badTicket" };
    return { ok: true, principal, protocol: WS_PROTOCOL };
  };
}

/** The principal of the WebSocket a protocol connection runs on (its ticket's). */
export class UpgradePrincipal extends Context.Service<UpgradePrincipal, Principal>()("finisherd/UpgradePrincipal") {}

const STATUS_TEXT: Record<number, string> = { 401: "Unauthorized", 403: "Forbidden", 404: "Not Found", 426: "Upgrade Required" };

/** Writes a plain HTTP refusal on a socket that asked to upgrade, then ends it. */
function refuse(socket: Duplex, status: number, body: unknown): void {
  const text = body === undefined ? "" : JSON.stringify(body);
  const head = [
    `HTTP/1.1 ${status} ${STATUS_TEXT[status] ?? "Error"}`,
    "Connection: close",
    ...(status === 426 ? [`Sec-WebSocket-Protocol: ${WS_PROTOCOL}`, "Upgrade: websocket"] : []),
    "Content-Type: application/json",
    `Content-Length: ${Buffer.byteLength(text)}`,
  ];
  socket.end(`${head.join("\r\n")}\r\n\r\n${text}`);
}

export interface WebSocketServerOptions {
  readonly check: (req: IncomingMessage) => UpgradeResult;
  /** Called for each refused upgrade (the audit). */
  readonly onRefused?: (result: Extract<UpgradeResult, { ok: false }>) => void;
  readonly maxPayload: number;
  readonly host?: string;
  /** How long a client may take to send its request headers (default `HEADERS_TIMEOUT_MS`). */
  readonly headersTimeoutMs?: number;
  /** Open TCP connections at most, WebSockets included (default `MAX_HTTP_CONNECTIONS`). */
  readonly maxConnections?: number;
}

/** Before authentication a client gets this long to send its headers (review of Task 6, finding 8). */
export const HEADERS_TIMEOUT_MS = 10_000;
/** Open connections to the loopback WebSocket listener at most: the UI and a few tools, never a flood. */
export const MAX_HTTP_CONNECTIONS = 64;

/**
 * A `SocketServer` on `127.0.0.1:0` whose sockets passed `check`. `bind` reports the port once
 * listening (the check needs it for the Host allow-list).
 */
export const makeTicketedWebSocketServer = (options: WebSocketServerOptions) =>
  Effect.gen(function* () {
    const wss = new WebSocketServer({ noServer: true, maxPayload: options.maxPayload, handleProtocols: () => WS_PROTOCOL });
    type Accepted = { conn: WebSocket; principal: Principal };
    const pending: Accepted[] = [];
    let onConnection = (a: Accepted) => void pending.push(a);

    const http: Server = yield* Effect.acquireRelease(
      Effect.sync(() => {
        const timeout = options.headersTimeoutMs ?? HEADERS_TIMEOUT_MS;
        // Slow or idle clients before the upgrade are cut off (checked every second); the
        // connection count bounds the rest.
        const server = createServer({ headersTimeout: timeout, requestTimeout: timeout, connectionsCheckingInterval: 1000 }, (req, res) => {
          // Nothing is served over plain HTTP.
          const status = req.url?.split("?")[0] === RPC_PATH ? 426 : 404;
          res.writeHead(status, { "content-type": "application/json", connection: "close" });
          res.end(status === 426 ? JSON.stringify({ protocol: { min: PROTOCOL_VERSION, max: PROTOCOL_VERSION } }) : "");
        });
        server.maxConnections = options.maxConnections ?? MAX_HTTP_CONNECTIONS;
        return server;
      }),
      (server) =>
        Effect.callback<void>((resume) => {
          for (const c of wss.clients) c.terminate();
          for (const a of pending.splice(0)) a.conn.terminate();
          wss.close();
          server.closeAllConnections();
          server.close(() => resume(Effect.void));
        }),
    );
    http.on("upgrade", (req: IncomingMessage, socket: Duplex, head: Buffer) => {
      socket.on("error", () => socket.destroy());
      let result: UpgradeResult;
      try {
        result = options.check(req);
      } catch {
        result = { ok: false, status: 401, reason: "badTicket" };
      }
      if (!result.ok) {
        options.onRefused?.(result);
        refuse(socket, result.status, result.body);
        return;
      }
      const { principal } = result;
      wss.handleUpgrade(req, socket, head, (conn) => {
        // Paused until a handler reads it, so nothing is dropped.
        conn.pause();
        onConnection({ conn, principal });
      });
    });
    yield* Effect.callback<void, SocketServerError>((resume) => {
      const fail = (cause: unknown) => resume(Effect.fail(new SocketServerError({ reason: new SocketServerOpenError({ cause }) })));
      http.once("error", fail);
      http.listen(0, options.host ?? "127.0.0.1", () => {
        http.off("error", fail);
        resume(Effect.void);
      });
    });
    const bound = http.address();
    if (bound === null || typeof bound === "string") return yield* Effect.die(new Error("the WebSocket server is not bound to an inet address"));
    const address = yield* Effect.orDie(Effect.fromResult(NetAddress.inetAddressFromIpString(bound.address, bound.port)));

    const run = <R, E, _>(handler: (socket: Socket.Socket) => Effect.Effect<_, E, R>) =>
      Effect.gen(function* () {
        const scope = yield* Scope.make();
        const services = Context.omit(Scope.Scope)(yield* Effect.context<R>());
        const trackFiber = Fiber.runIn(scope);
        const previous = onConnection;
        onConnection = ({ conn, principal }) => {
          // `R` minus `Scope` is what `services` holds: the handler's own scope is per socket.
          const context = Context.add(services, UpgradePrincipal, principal) as Context.Context<unknown>;
          const socket = Socket.fromWebSocket(
            Effect.acquireRelease(Effect.succeed(conn as unknown as globalThis.WebSocket), (c) => Effect.sync(() => c.close())),
          );
          trackFiber(
            Effect.runForkWith(context)(
              Effect.catchCause(Effect.flatMap(socket, handler), (cause) => Effect.logDebug("websocket connection ended", cause)) as Effect.Effect<unknown, never, unknown>,
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

    return { server: SocketServer.of({ address, run }), port: bound.port };
  });
