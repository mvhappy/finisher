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
//
// Limits (review of Task 7, M2): sockets that have not upgraded are capped on their own
// (`MAX_UNAUTHENTICATED`) and closed if they do not upgrade within the header timeout, so a
// loopback slowloris cannot use up the WebSockets' room. Once they are all taken, a new socket
// first closes those that have waited longer than `AT_CAP_GRACE_MS` (a short header timeout
// at the cap; review of Tasks 8–9): the slots are not reserved for anyone, so any local user
// can still hold them by reopening sockets faster than that, which blocks new app WebSockets
// meanwhile (TASKS records the residual); WebSockets are capped per ticket owner
// (`MAX_WS_PER_OWNER`: a credential, or one `local` connection), for `local` as a whole and in
// all, with `RESERVED_WS` of them only for `device` and `console` principals (the app and the
// owner), so a principal minting tickets cannot lock out the UI.
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { Duplex } from "node:stream";
import { PROTOCOL_VERSION, WS_PROTOCOL_PREFIX, WS_TICKET_PREFIX } from "@finisher/contracts";
import { Context, Effect, Exit, Fiber, Scope } from "effect";
import * as NetAddress from "effect/net/NetAddress";
import * as Socket from "effect/socket/Socket";
import { SocketServer, SocketServerError, SocketServerOpenError } from "effect/socket/SocketServer";
import { WebSocketServer, type WebSocket } from "ws";
import type { Principal } from "./principal.ts";
import { principalKeyOf, type Tickets } from "./auth/tickets.ts";
import { hostAllowed, type ListenerPolicy, loopbackPolicy, MAX_UNAUTHENTICATED, originAllowed, RPC_PATH } from "./http/policy.ts";

export { RPC_PATH } from "./http/policy.ts";
export const WS_PROTOCOL = `${WS_PROTOCOL_PREFIX}${PROTOCOL_VERSION}` as const;

export type UpgradeFailure = "badPath" | "badVersion" | "badHost" | "badOrigin" | "noTicket" | "badTicket" | "tooMany";

export type UpgradeResult =
  /** `owner`: who minted the ticket (`ownerOf` in `tickets.ts`), for the per-owner cap. */
  | { ok: true; principal: Principal; owner: string; protocol: typeof WS_PROTOCOL }
  | { ok: false; status: 401 | 403 | 404 | 426 | 503; reason: UpgradeFailure; body?: unknown };

export type UpgradeCheckOptions =
  | {
      /** The port the server is bound to (known once it listens): the loopback listener's lists. */
      readonly port: () => number;
      readonly tickets: Tickets;
      /** Configured origins (`settings.json` `server.origins`), read at each upgrade. */
      readonly origins: () => ReadonlyArray<string>;
    }
  | {
      /** The listener's own Host and Origin lists (5B Decision 11: the remote listener's). */
      readonly policy: ListenerPolicy;
      readonly tickets: Tickets;
    };

export { loopbackHosts, normalOrigin } from "./http/policy.ts";

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
  const policy = "policy" in opts ? opts.policy : loopbackPolicy(opts.port, opts.origins);
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
    if (!hostAllowed(policy, req.headers.host)) return { ok: false, status: 403, reason: "badHost" };
    if (!originAllowed(policy, req.headers.origin)) return { ok: false, status: 403, reason: "badOrigin" };
    const tickets = protocols.filter((p) => p.startsWith(WS_TICKET_PREFIX));
    if (tickets.length !== 1) return { ok: false, status: 401, reason: "noTicket" };
    const redeemed = opts.tickets.redeem(tickets[0]!.slice(WS_TICKET_PREFIX.length));
    if (redeemed === undefined) return { ok: false, status: 401, reason: "badTicket" };
    return { ok: true, principal: redeemed.principal, owner: redeemed.owner, protocol: WS_PROTOCOL };
  };
}

/** The principal of the WebSocket a protocol connection runs on (its ticket's). */
export class UpgradePrincipal extends Context.Service<UpgradePrincipal, Principal>()("finisherd/UpgradePrincipal") {}

const STATUS_TEXT: Record<number, string> = {
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  426: "Upgrade Required",
  503: "Service Unavailable",
};

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
  /** How long a client may take to upgrade, headers included (default `HEADERS_TIMEOUT_MS`). */
  readonly headersTimeoutMs?: number;
  /** Open sockets that have not upgraded at most (default `MAX_UNAUTHENTICATED`). */
  readonly maxUnauthenticated?: number;
  /** At the cap, how long a socket may have waited before a new one closes it (default `AT_CAP_GRACE_MS`). */
  readonly atCapGraceMs?: number;
  /** WebSockets at most, every principal together (default `MAX_WEBSOCKETS`). */
  readonly maxWebSockets?: number;
  /**
   * The WebSocket slots, shared by every listener (5B Decision 12: 64 in all, 16 only for
   * `device` and `console`); default a pool of this listener's own.
   */
  readonly slots?: WebSocketSlots;
  /** Pre-upgrade sockets per remote address at most (5B: the remote listener's 4); default none. */
  readonly perAddress?: number;
  /** The listener's policy: its `maxUnauthenticated` and `perAddress` apply unless given above. */
  readonly policy?: ListenerPolicy;
  /** A socket's remote address (tests inject addresses); default the socket's own. */
  readonly remoteOf?: (socket: Duplex) => string;
  /** Plain HTTP requests (5B's front, `http/front.ts`); default 5A's 404, or 426 on `/rpc`. */
  readonly onRequest?: (req: IncomingMessage, res: ServerResponse) => void;
  /** The port to listen on (default 0: any). */
  readonly port?: number;
}

/** Before authentication a client gets this long to upgrade (review of Task 6, finding 8). */
export const HEADERS_TIMEOUT_MS = 10_000;
/** Open sockets that have not upgraded yet, at most: past it a new one is closed at once (`http/policy.ts`). */
export { MAX_UNAUTHENTICATED } from "./http/policy.ts";
/** Once `MAX_UNAUTHENTICATED` are open, the header timeout of the ones already open. */
export const AT_CAP_GRACE_MS = 1_000;
/** WebSockets at most: the UI and a few tools, never a flood. */
export const MAX_WEBSOCKETS = 64;
/** Of `MAX_WEBSOCKETS`, the ones only `device` and `console` principals may take. */
export const RESERVED_WS = 16;
/** WebSockets per ticket owner (a credential, or one `local` connection). */
export const MAX_WS_PER_OWNER = 8;
/** WebSockets of `local` as a whole (every `fin` and tool of the user). */
export const MAX_WS_LOCAL = 32;

/** The principal kinds the reserved WebSockets are for. */
const RESERVED_KINDS: ReadonlySet<string> = new Set(["device", "console"]);

/**
 * Live WebSockets by owner and by principal: `acquire` answers the release of a slot, or
 * `undefined` when a cap is reached.
 */
export class WebSocketSlots {
  readonly #byOwner = new Map<string, number>();
  readonly #byPrincipal = new Map<string, number>();
  #total = 0;
  readonly #max: number;

  constructor(max: number = MAX_WEBSOCKETS) {
    this.#max = max;
  }

  acquire(principal: Principal, owner: string): (() => void) | undefined {
    const key = principalKeyOf(principal);
    const reserved = RESERVED_KINDS.has(principal.kind);
    const room = reserved ? this.#max : Math.max(0, this.#max - RESERVED_WS);
    if (this.#total >= room) return undefined;
    if ((this.#byOwner.get(owner) ?? 0) >= MAX_WS_PER_OWNER) return undefined;
    const perPrincipal = principal.kind === "local" ? MAX_WS_LOCAL : MAX_WS_PER_OWNER;
    if ((this.#byPrincipal.get(key) ?? 0) >= perPrincipal) return undefined;
    const bump = (m: Map<string, number>, k: string, d: number) => {
      const n = (m.get(k) ?? 0) + d;
      if (n === 0) m.delete(k);
      else m.set(k, n);
    };
    bump(this.#byOwner, owner, 1);
    bump(this.#byPrincipal, key, 1);
    this.#total++;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      bump(this.#byOwner, owner, -1);
      bump(this.#byPrincipal, key, -1);
      this.#total--;
    };
  }

  get size(): number {
    return this.#total;
  }
}

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

    const timeout = options.headersTimeoutMs ?? HEADERS_TIMEOUT_MS;
    const maxUnauthenticated = options.maxUnauthenticated ?? options.policy?.maxUnauthenticated ?? MAX_UNAUTHENTICATED;
    const atCapGrace = options.atCapGraceMs ?? AT_CAP_GRACE_MS;
    const maxWebSockets = options.maxWebSockets ?? MAX_WEBSOCKETS;
    const slots = options.slots ?? new WebSocketSlots(maxWebSockets);
    const perAddress = options.perAddress ?? options.policy?.perAddress;
    const remoteOf = options.remoteOf ?? ((s: Duplex) => (s as Duplex & { remoteAddress?: string }).remoteAddress ?? "");
    /** Pre-upgrade sockets per remote address (only counted with `perAddress`). */
    const byAddress = new Map<string, number>();
    /** Sockets that have not upgraded: when each opened, and what marks it upgraded (it stops counting). */
    const unauthenticated = new Map<Duplex, { readonly openedAt: number; readonly upgraded: () => void }>();
    const http: Server = yield* Effect.acquireRelease(
      Effect.sync(() => {
        // Slow or idle clients before the upgrade are cut off (checked every second); the
        // connection count bounds the rest.
        const plain = (req: IncomingMessage, res: ServerResponse) => {
          // 5A: nothing is served over plain HTTP.
          const status = req.url?.split("?")[0] === RPC_PATH ? 426 : 404;
          res.writeHead(status, { "content-type": "application/json", connection: "close" });
          res.end(status === 426 ? JSON.stringify({ protocol: { min: PROTOCOL_VERSION, max: PROTOCOL_VERSION } }) : "");
        };
        const server = createServer({ headersTimeout: timeout, requestTimeout: timeout, connectionsCheckingInterval: 1000 }, options.onRequest ?? plain);
        // A backstop only: the caps below decide first.
        server.maxConnections = maxUnauthenticated + maxWebSockets;
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
    // Every socket starts unauthenticated: capped on its own, and closed unless it upgrades in
    // time, whether it sends half a request or nothing at all.
    http.on("connection", (socket: Duplex) => {
      // One remote address holds at most `perAddress` of them (5B), so one LAN host cannot hold
      // the pool; checked first, so a refused socket evicts nobody.
      const address = perAddress === undefined ? undefined : remoteOf(socket);
      if (address !== undefined && (byAddress.get(address) ?? 0) >= perAddress!) {
        socket.destroy();
        return;
      }
      if (unauthenticated.size >= maxUnauthenticated) {
        // At the cap the header timeout is short: the ones that have waited past it go first.
        const now = Date.now();
        for (const [old, p] of [...unauthenticated]) {
          if (now - p.openedAt < atCapGrace) continue;
          p.upgraded();
          old.destroy();
        }
      }
      if (unauthenticated.size >= maxUnauthenticated) {
        socket.destroy();
        return;
      }
      if (address !== undefined) byAddress.set(address, (byAddress.get(address) ?? 0) + 1);
      const timer = setTimeout(() => socket.destroy(), timeout);
      timer.unref();
      let counted = true;
      const upgraded = () => {
        clearTimeout(timer);
        unauthenticated.delete(socket);
        if (address !== undefined && counted) {
          counted = false;
          const n = (byAddress.get(address) ?? 1) - 1;
          if (n <= 0) byAddress.delete(address);
          else byAddress.set(address, n);
        }
      };
      unauthenticated.set(socket, { openedAt: Date.now(), upgraded });
      socket.once("close", upgraded);
    });
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
      const release = slots.acquire(principal, result.owner);
      if (release === undefined) {
        const tooMany = { ok: false, status: 503, reason: "tooMany" } as const;
        options.onRefused?.(tooMany);
        refuse(socket, tooMany.status, undefined);
        return;
      }
      socket.once("close", release);
      unauthenticated.get(socket)?.upgraded();
      wss.handleUpgrade(req, socket, head, (conn) => {
        // Paused until a handler reads it, so nothing is dropped.
        conn.pause();
        onConnection({ conn, principal });
      });
    });
    yield* Effect.callback<void, SocketServerError>((resume) => {
      const fail = (cause: unknown) => resume(Effect.fail(new SocketServerError({ reason: new SocketServerOpenError({ cause }) })));
      http.once("error", fail);
      http.listen(options.port ?? 0, options.host ?? "127.0.0.1", () => {
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
