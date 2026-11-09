// The HTTP front (5B Decision 11): one request handler for the loopback listener (5A's
// `node:http` on 127.0.0.1) and the remote TLS listener (Task 10). In order:
//   1. `Host` must be one the listener serves (`HOST_REFUSED`, 403): DNS rebinding;
//   2. `Origin` absent, or one the listener serves (`ORIGIN_REFUSED`, 403);
//   3. `OPTIONS` of a known path: a CORS preflight for the allowed origin;
//   4. `/rpc` without an upgrade is 426 (as in 5A); a path the front has no route for is 404;
//   5. a `POST` body is JSON (`Content-Type: application/json`), at most 16 KiB.
// Every answer carries `Connection: close`, `Cache-Control: no-store`,
// `X-Content-Type-Options: nosniff`, the current DPoP nonce (`DPoP-Nonce`) and CORS headers for
// an allowed origin only; none sets a cookie. Errors are `{error: {code, message, details,
// fix?}}` with the status of Decision 33. A refusal before a route (Host, Origin, body) of a
// pairing or token path is reported to `onRefused` (the audit: `auth.fail` with the remote
// address, aggregated). Nothing here logs a body, a header value or a token.
import type { IncomingMessage, ServerResponse } from "node:http";
import { DEVICE_ERROR_CODES, FinError, PROTOCOL_VERSION } from "@finisher/contracts";
import type { Audit } from "../control/audit.ts";
import { finError, toFinError } from "../errors.ts";
import { corsHeaders, preflightHeaders } from "./cors.ts";
import { hostAllowed, type ListenerPolicy, normalOrigin, originAllowed, RPC_PATH } from "./policy.ts";

/** The largest request body the front reads. */
export const MAX_BODY_BYTES = 16 * 1024;
export { RPC_PATH };

export interface HttpCtx {
  readonly listener: ListenerPolicy["name"];
  /** The peer's address (the audit's only). */
  readonly remote: string;
  readonly url: URL;
  /** The request's `Host`, lower case (one the listener serves). */
  readonly host: string;
  /** The request's normalised `Origin`, when a browser sent one (allowed by now). */
  readonly origin?: string;
}

export interface HttpAnswer {
  readonly status: number;
  readonly body?: unknown;
  readonly headers?: Readonly<Record<string, string>>;
}

/** A route: the request, its parsed JSON body (`undefined` for a `GET`), the context. */
export type Route = (req: IncomingMessage, body: unknown, ctx: HttpCtx) => Promise<HttpAnswer>;

/** What the front needs of the server nonces (`auth/nonces.ts`, Task 7). */
export interface NonceSource {
  current(): string;
}

/** The credential kind an audit row of a refusal on `path` names, or none (not an auth endpoint). */
export function credentialKindOf(path: string): "pairing" | "dpop" | undefined {
  if (path.startsWith("/pair/")) return "pairing";
  if (path.startsWith("/auth/")) return "dpop";
  return undefined;
}

export interface FrontRefusal {
  readonly code: string;
  readonly reason: string;
  readonly remote: string;
  readonly credentialKind: "pairing" | "dpop";
}

export interface FrontOptions {
  readonly onRefused?: (r: FrontRefusal) => void;
  /** The peer's address (tests inject one); default the socket's. */
  readonly remoteOf?: (req: IncomingMessage) => string;
}

/** The status of `code` (Decision 33), else by the error's class. */
export function statusOf(code: string): number {
  const known = (DEVICE_ERROR_CODES as Record<string, { status?: number }>)[code]?.status;
  if (known !== undefined) return known;
  if (code === "INVALID_REQUEST" || code === "INVALID_NAME") return 400;
  if (code === "NOT_FOUND") return 404;
  if (code === "UNAUTHENTICATED") return 401;
  return 500;
}

const invalid = (status: number, reason: string, message: string): HttpAnswer => ({
  status,
  body: { error: { code: "INVALID_REQUEST", message, details: { reason } } },
});

const hostRefused = () =>
  finError("HOST_REFUSED", "this server does not answer to that host name", {}, "Reach the server by one of the addresses its owner shared with you.");
const originRefused = () =>
  finError("ORIGIN_REFUSED", "this server does not accept requests from that web page", {}, "Open Finisher from the page or app its owner set up for this server.");

function errorBody(e: FinError): unknown {
  // An internal error's text may name what failed inside; over HTTP it says nothing.
  if (e.code === "INTERNAL") return { error: { code: "INTERNAL", message: "the server could not answer this request", details: {} } };
  return { error: { code: e.code, message: e.message, details: e.details ?? {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } };
}

/** Reads at most `MAX_BODY_BYTES`: the body, or `"tooLarge"` (the rest is not read). */
function readBody(req: IncomingMessage): Promise<Buffer | "tooLarge"> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (c: Buffer) => {
      size += c.length;
      if (size > MAX_BODY_BYTES) {
        req.removeAllListeners("data");
        req.pause();
        resolve("tooLarge");
        return;
      }
      chunks.push(c);
    });
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

const isJson = (type: string | undefined) => type !== undefined && /^application\/json\s*(;.*)?$/i.test(type.trim());

/**
 * The request handler of a listener: `routes` keyed `"<METHOD> <path>"` (e.g.
 * `"POST /pair/redeem"`), `policy` the listener's, `nonces` the server's.
 */
export function makeFront(
  routes: Readonly<Record<string, Route>>,
  policy: ListenerPolicy,
  nonces: NonceSource | undefined,
  options: FrontOptions = {},
): (req: IncomingMessage, res: ServerResponse) => void {
  const paths = new Set(Object.keys(routes).map((k) => k.slice(k.indexOf(" ") + 1)));
  return (req, res) => {
    const remote = options.remoteOf?.(req) ?? req.socket.remoteAddress ?? "";
    let url: URL;
    try {
      url = new URL(req.url ?? "/", "http://front.invalid");
    } catch {
      url = new URL("http://front.invalid/");
    }
    const path = url.pathname;
    const kind = credentialKindOf(path);
    const rawOrigin = typeof req.headers.origin === "string" ? req.headers.origin : undefined;
    const send = (a: HttpAnswer, origin?: string) => {
      const text = a.body === undefined ? "" : JSON.stringify(a.body);
      res.writeHead(a.status, {
        // A route's own headers first: the front's below always win.
        ...a.headers,
        "Content-Type": "application/json",
        "Content-Length": String(Buffer.byteLength(text)),
        Connection: "close",
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
        ...(nonces === undefined ? {} : { "DPoP-Nonce": nonces.current() }),
        ...corsHeaders(origin),
      });
      res.end(text);
    };
    const refuse = (e: FinError, reason: string, origin?: string) => {
      if (kind !== undefined) options.onRefused?.({ code: e.code, reason, remote, credentialKind: kind });
      send({ status: statusOf(e.code), body: errorBody(e) }, origin);
    };
    if (!hostAllowed(policy, typeof req.headers.host === "string" ? req.headers.host : undefined)) return refuse(hostRefused(), "host");
    if (!originAllowed(policy, rawOrigin)) return refuse(originRefused(), "origin");
    const origin = rawOrigin === undefined ? undefined : normalOrigin(rawOrigin);
    const method = (req.method ?? "GET").toUpperCase();
    if (method === "OPTIONS") {
      if (!paths.has(path) || origin === undefined) return send({ status: 404 });
      return send({ status: 204, headers: preflightHeaders(origin) });
    }
    if (path === RPC_PATH) return send({ status: 426, body: { protocol: { min: PROTOCOL_VERSION, max: PROTOCOL_VERSION } } }, origin);
    const route = routes[`${method} ${path}`];
    if (route === undefined) return send({ status: 404 }, origin);
    const ctx: HttpCtx = { listener: policy.name, remote, url, host: String(req.headers.host).toLowerCase(), ...(origin === undefined ? {} : { origin }) };
    void (async () => {
      let body: unknown;
      if (method === "POST") {
        if (!isJson(req.headers["content-type"])) {
          if (kind !== undefined) options.onRefused?.({ code: "INVALID_REQUEST", reason: "contentType", remote, credentialKind: kind });
          return send(invalid(415, "contentType", "the body must be JSON (Content-Type: application/json)"), origin);
        }
        const declared = Number(req.headers["content-length"] ?? "0");
        const raw = Number.isFinite(declared) && declared > MAX_BODY_BYTES ? "tooLarge" : await readBody(req);
        if (raw === "tooLarge") {
          if (kind !== undefined) options.onRefused?.({ code: "INVALID_REQUEST", reason: "bodyTooLarge", remote, credentialKind: kind });
          return send(invalid(413, "bodyTooLarge", `the body is at most ${MAX_BODY_BYTES} bytes`), origin);
        }
        try {
          body = JSON.parse(raw.toString("utf8"));
        } catch {
          return send(invalid(400, "notJson", "the body is not JSON"), origin);
        }
      }
      let answer: HttpAnswer;
      try {
        answer = await route(req, body, ctx);
      } catch (e) {
        const err = e instanceof FinError ? e : toFinError(e);
        answer = { status: statusOf(err.code), body: errorBody(err) };
      }
      send(answer, origin);
    })().catch(() => {
      if (!res.headersSent) send({ status: 500 });
      else res.destroy();
    });
  };
}

/**
 * The audit rows of refusals before any proof: the front's (`auth.fail` with the address) and
 * a WebSocket upgrade's on `transport` (never a wrong path, a wrong version or a full pool).
 */
export const refusalAudits = (audit: Audit) => ({
  front: (r: FrontRefusal) =>
    audit.append({
      kind: "auth.fail",
      principal: { kind: "anonymous", id: "" },
      credentialKind: r.credentialKind,
      outcome: "failed",
      code: r.code,
      details: { reason: r.reason, address: r.remote.slice(0, 64) },
    }),
  upgrade:
    (transport: "ws" | "wss") =>
    (r: { readonly reason: string }): void => {
      if (r.reason === "badPath" || r.reason === "badVersion" || r.reason === "tooMany") return;
      audit.append({ kind: "auth.fail", principal: { kind: "anonymous", id: "" }, credentialKind: "ticket", outcome: "failed", code: "UNAUTHENTICATED", details: { reason: r.reason, transport } });
    },
});
