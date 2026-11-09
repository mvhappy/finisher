// CORS for the HTTP front (5B Decision 11): answered for allowed origins only (`policy.ts`), so
// a page elsewhere gets no `Access-Control-Allow-Origin` and its script cannot read the answer.
// Never `Access-Control-Allow-Credentials`: the front sets no cookie and reads none.

export const PREFLIGHT_MAX_AGE_S = 600;

/** The headers of an answer to an allowed `origin` (none without one). */
export function corsHeaders(origin: string | undefined): Record<string, string> {
  if (origin === undefined) return {};
  return { "Access-Control-Allow-Origin": origin, Vary: "Origin", "Access-Control-Expose-Headers": "DPoP-Nonce" };
}

/** A preflight's headers for an allowed `origin`. */
export function preflightHeaders(origin: string): Record<string, string> {
  return {
    ...corsHeaders(origin),
    "Access-Control-Allow-Methods": "POST",
    "Access-Control-Allow-Headers": "authorization, dpop, content-type",
    "Access-Control-Max-Age": String(PREFLIGHT_MAX_AGE_S),
  };
}
