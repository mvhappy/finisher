// The pairing endpoints of the HTTP front (5B Decisions 13, 18, 20): `POST /pair/redeem` (no
// proof: the offer's secret is the credential; rate limited per address and per server) and
// `POST /pair/status` (a DPoP proof by the pending request's key). Every refusal before a known
// device is audited `auth.fail {credentialKind: pairing|dpop, reason, address}` (aggregated).
import { FinError, type RouteKind } from "@finisher/contracts";
import type { Audit } from "../control/audit.ts";
import type { Environment } from "../env/descriptor.ts";
import { finError } from "../errors.ts";
import type { HttpCtx, Route } from "../http/front.ts";
import { Buckets, PAIR_PER_ADDRESS, PAIR_PER_SERVER } from "../http/limits.ts";
import { type DpopVerifier, dpopRefusal } from "./dpop.ts";
import type { Pairing } from "./pairing.ts";

export interface PairHttpDeps {
  readonly pairing: Pairing;
  readonly verifier: DpopVerifier;
  readonly env: Environment;
  readonly audit: Audit;
  /** The origins a proof's `htu` may name on the listener the request came in on. */
  readonly allowedOrigins: (ctx: HttpCtx) => ReadonlyArray<string>;
  /** The route kind of the listener and host a request came over (never its address). */
  readonly routeOf: (ctx: HttpCtx) => RouteKind;
  readonly clock?: () => number;
}

const SERVER_KEY = "server";

export function pairRoutes(d: PairHttpDeps): Record<string, Route> {
  const perAddress = new Buckets(PAIR_PER_ADDRESS, undefined, d.clock);
  const perServer = new Buckets(PAIR_PER_SERVER, undefined, d.clock);
  const fail = (ctx: HttpCtx, credentialKind: "pairing" | "dpop", code: string, reason: string) =>
    d.audit.append({
      kind: "auth.fail",
      principal: { kind: "anonymous", id: "" },
      credentialKind,
      outcome: "failed",
      code,
      details: { reason, address: ctx.remote.slice(0, 64) },
    });
  return {
    "POST /pair/redeem": async (_req, body, ctx) => {
      // The address's bucket first, the server's only when it passed: one host cannot drain the
      // server's for everyone, and each new address still takes from the server's.
      const address = perAddress.take(ctx.remote);
      const server = address && perServer.take(SERVER_KEY);
      if (!server || !address) {
        fail(ctx, "pairing", "PAIRING_RATE_LIMITED", address ? "server" : "address");
        const retry = address ? perServer.retryAfterS(SERVER_KEY) : perAddress.retryAfterS(ctx.remote);
        const e = finError("PAIRING_RATE_LIMITED", "too many pairing attempts", { retryAfterS: retry }, "Wait a minute, then try the pairing link again.");
        return { status: 429, headers: { "Retry-After": String(retry) }, body: { error: { code: e.code, message: e.message, details: e.details, fix: e.fix } } };
      }
      try {
        const answer = await d.pairing.redeem(body, { remote: ctx.remote, listener: ctx.listener, route: d.routeOf(ctx) });
        return { status: 200, body: answer };
      } catch (e) {
        if (e instanceof FinError) fail(ctx, "pairing", e.code, "redeem");
        throw e;
      }
    },
    "POST /pair/status": async (req, body, ctx) => {
      const pendingId = (body as { pendingId?: unknown } | null)?.pendingId;
      if (typeof pendingId !== "string") throw finError("INVALID_REQUEST", "the request names no pending pairing", { field: "pendingId" });
      const header = req.headers.dpop;
      const proof = await d.verifier.verify(typeof header === "string" ? header : undefined, {
        method: "POST",
        url: ctx.url,
        allowedOrigins: d.allowedOrigins(ctx),
        knownKey: (jkt) => d.pairing.isPendingKey(jkt),
      });
      if (!proof.ok) {
        if (proof.code !== "DPOP_NONCE") fail(ctx, "dpop", proof.code, proof.reason);
        return dpopRefusal(proof);
      }
      try {
        const s = d.pairing.status(pendingId, proof.jkt);
        return { status: 200, body: s.state === "approved" ? { ...s, environment: d.env.describe() } : s };
      } catch (e) {
        if (e instanceof FinError) fail(ctx, "pairing", e.code, "status");
        throw e;
      }
    },
  };
}
