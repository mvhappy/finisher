// Authentication in front of every handler (spec §4.4 stage 2): a request on a connection that
// has not proved a credential reaches only the public RPCs (`server.hello`, `auth.prove`); any
// other answers `UNAUTHENTICATED` without its handler running. Task 5's chain takes this stage
// over. Until `ENFORCE.proof` (Task 4), an unproved connection runs as `local` and is audited
// once as `auth.legacy`.
import { type FinError, PUBLIC_RPCS, STREAM_RPCS } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import type { Audit } from "../control/audit.ts";
import type { Connection, Connections } from "../connections.ts";
import { unauthenticated } from "./handshake.ts";

type Handler = (payload: unknown, options: { readonly client: { readonly id: number } }) => unknown;

/**
 * `handlers` with the authentication stage in front of each non-public one. `timed` counts a
 * unary refusal in `rpc.stats` (spec §3.4: denials are counted there, not journalled).
 */
export function withAuthentication<H extends object>(
  handlers: H,
  connections: Connections,
  audit: Audit,
  timed: <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError> = (_, self) => self,
): H {
  const onLegacy = (c: Connection) =>
    audit.append({ kind: "auth.legacy", principal: { kind: "local", id: "local" }, outcome: "ok", details: { transport: c.transport } });
  const streams = new Set<string>(STREAM_RPCS);
  const out: Record<string, Handler> = {};
  for (const [tag, handler] of Object.entries(handlers) as Array<[string, Handler]>) {
    if (PUBLIC_RPCS.has(tag)) {
      out[tag] = handler;
      continue;
    }
    const refuse = streams.has(tag) ? () => Stream.fail(unauthenticated("unproved")) : () => timed(tag, Effect.fail(unauthenticated("unproved")));
    out[tag] = (payload, options) =>
      connections.principalOf(options.client.id, onLegacy).kind === "anonymous" ? refuse() : handler(payload, options);
  }
  return out as H;
}
