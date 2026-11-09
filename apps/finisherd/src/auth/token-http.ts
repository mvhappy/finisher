// Device tokens and tickets on the HTTP front (5B Decision 16):
// - `POST /auth/token`: a DPoP proof by a paired device's key → `{accessToken, tokenType:
//   "DPoP", expiresIn: 900}`; a revoked device's key is `DEVICE_REVOKED`;
// - `POST /auth/ws-ticket`: `Authorization: DPoP <token>` and a proof by the token's key that
//   carries `ath` → a one-time WebSocket ticket for the device's principal (5A's tickets, owned
//   by `device:<id>`; `TICKET_LIMIT` applies) → `{ticket, expiresIn: 30}`.
// Both: 30 a minute per device and 120 per server (`TOKEN_RATE_LIMITED`); `ENV_MOVED` while the
// server data waits for its owner; every refusal before a known device audited `auth.fail
// {credentialKind: dpop, reason, address}`. A use notes the device seen over the route's kind.
import { DEVICE_TICKET_TTL_S, FinError, type RouteKind } from "@finisher/contracts";
import type { Audit } from "../control/audit.ts";
import type { Environment } from "../env/descriptor.ts";
import { envMoved } from "../env/descriptor.ts";
import { finError, toFinError } from "../errors.ts";
import type { HttpAnswer, HttpCtx, Route } from "../http/front.ts";
import { Buckets, TOKEN_PER_DEVICE, TOKEN_PER_SERVER } from "../http/limits.ts";
import type { DeviceTokens } from "./device-tokens.ts";
import type { DeviceStore } from "./devices.ts";
import { type DpopVerifier, dpopRefusal } from "./dpop.ts";
import { deviceRevoked } from "./revoke.ts";
import type { Tickets } from "./tickets.ts";

export interface TokenHttpDeps {
  readonly devices: DeviceStore;
  readonly tokens: DeviceTokens;
  readonly tickets: Pick<Tickets, "mint">;
  readonly verifier: DpopVerifier;
  readonly env: Pick<Environment, "state">;
  readonly audit: Audit;
  readonly allowedOrigins: (ctx: HttpCtx) => ReadonlyArray<string>;
  readonly routeOf: (ctx: HttpCtx) => RouteKind;
  readonly clock?: () => number;
}

const SERVER_KEY = "server";

const errorAnswer = (status: number, e: FinError, headers?: Record<string, string>): HttpAnswer => ({
  status,
  ...(headers === undefined ? {} : { headers }),
  body: { error: { code: e.code, message: e.message, details: e.details ?? {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } },
});

export function tokenRoutes(d: TokenHttpDeps): Record<string, Route> {
  const perDevice = new Buckets(TOKEN_PER_DEVICE, undefined, d.clock);
  const perServer = new Buckets(TOKEN_PER_SERVER, undefined, d.clock);
  const fail = (ctx: HttpCtx, code: string, reason: string) =>
    d.audit.append({
      kind: "auth.fail",
      principal: { kind: "anonymous", id: "" },
      credentialKind: "dpop",
      outcome: "failed",
      code,
      details: { reason, address: ctx.remote.slice(0, 64) },
    });

  /** The proof, the live device it names, the rate limits: the device or the refusal. */
  const prove = async (req: Parameters<Route>[0], ctx: HttpCtx, accessToken?: string) => {
    if (d.env.state() === "moved") {
      fail(ctx, "ENV_MOVED", "moved");
      return { answer: errorAnswer(403, envMoved()) };
    }
    const header = req.headers.dpop;
    // A revoked device's key and a device past its rate limit are refused before the proof's
    // `jti` is remembered, so neither fills the replay cache (review of Task 9).
    const proof = await d.verifier.verify(typeof header === "string" ? header : undefined, {
      method: "POST",
      url: ctx.url,
      allowedOrigins: d.allowedOrigins(ctx),
      ...(accessToken === undefined ? {} : { accessToken }),
      knownKey: (jkt) => d.devices.isKnownJkt(jkt),
      admit: (jkt) => {
        const device = d.devices.byJkt(jkt);
        if (device === undefined) return { code: "DEVICE_REVOKED", reason: "revoked" };
        if (!perDevice.take(device.id)) return { code: "TOKEN_RATE_LIMITED", reason: "device" };
        if (!perServer.take(SERVER_KEY)) return { code: "TOKEN_RATE_LIMITED", reason: "server" };
        return undefined;
      },
    });
    if (!proof.ok) {
      if (proof.code === "ADMIT") {
        if (proof.refusal.code === "DEVICE_REVOKED") {
          fail(ctx, "DEVICE_REVOKED", "revoked");
          return { answer: errorAnswer(401, deviceRevoked()) };
        }
        const device = d.devices.byJkt(proof.jkt);
        const retry = proof.refusal.reason === "server" || device === undefined ? perServer.retryAfterS(SERVER_KEY) : perDevice.retryAfterS(device.id);
        const e = finError("TOKEN_RATE_LIMITED", "too many token requests", { retryAfterS: retry }, "Wait a minute, then try again.");
        return { answer: errorAnswer(429, e, { "Retry-After": String(retry) }) };
      }
      if (proof.code !== "DPOP_NONCE") fail(ctx, proof.code, proof.reason);
      return { answer: dpopRefusal(proof) };
    }
    const device = d.devices.byJkt(proof.jkt)!;
    return { device, jkt: proof.jkt };
  };

  return {
    "POST /auth/token": async (req, _body, ctx) => {
      const r = await prove(req, ctx);
      if ("answer" in r) return r.answer;
      d.devices.seen(r.device.id, d.routeOf(ctx));
      return { status: 200, body: { ...d.tokens.issue({ id: r.device.id, jkt: r.jkt }), tokenType: "DPoP" } };
    },
    "POST /auth/ws-ticket": async (req, _body, ctx) => {
      const auth = req.headers.authorization;
      const m = typeof auth === "string" ? /^DPoP ([A-Za-z0-9_-]{16,128})$/.exec(auth) : null;
      if (m === null) {
        fail(ctx, "DPOP_INVALID", "noToken");
        return dpopRefusal({ ok: false, code: "DPOP_INVALID", reason: "missing" });
      }
      const token = m[1]!;
      const r = await prove(req, ctx, token);
      if ("answer" in r) return r.answer;
      const held = d.tokens.check(token, r.jkt);
      if ("refused" in held) {
        fail(ctx, held.refused, "token");
        const e =
          held.refused === "DEVICE_REVOKED"
            ? deviceRevoked()
            : held.refused === "TOKEN_EXPIRED"
              ? finError("TOKEN_EXPIRED", "this access token expired", {}, "Ask the server for a new token with a fresh proof.")
              : finError("DPOP_INVALID", "the proof's key is not the token's", { reason: "jkt" }, "Make the proof with the key the token was given to.");
        return errorAnswer(401, e);
      }
      d.devices.seen(r.device.id, d.routeOf(ctx));
      try {
        const minted = d.tickets.mint(d.devices.principalOf(r.device), DEVICE_TICKET_TTL_S);
        return { status: 200, body: { ticket: minted.ticket, expiresIn: DEVICE_TICKET_TTL_S } };
      } catch (e) {
        const err = toFinError(e);
        return errorAnswer(err.code === "TICKET_LIMIT" ? 429 : 500, err);
      }
    },
  };
}
