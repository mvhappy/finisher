// Authentication in front of every handler (spec §4.4 stage 2): a request on a connection that
// has not proved a credential reaches only the public RPCs (`server.hello`, `auth.prove`); any
// other answers `UNAUTHENTICATED` without its handler running. Task 5's chain takes this stage
// over.
import { type FinError, PUBLIC_RPCS, STREAM_RPCS } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import type { Connections } from "../connections.ts";
import { unauthenticated } from "./handshake.ts";

type Handler = (payload: unknown, options: { readonly client: { readonly id: number } }) => unknown;

/**
 * `handlers` with the authentication stage in front of each non-public one. `timed` counts a
 * unary refusal in `rpc.stats` (spec §3.4: denials are counted there, not journalled).
 */
export function withAuthentication<H extends object>(
  handlers: H,
  connections: Connections,
  timed: <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError> = (_, self) => self,
): H {
  const streams = new Set<string>(STREAM_RPCS);
  const out: Record<string, Handler> = {};
  for (const [tag, handler] of Object.entries(handlers) as Array<[string, Handler]>) {
    if (PUBLIC_RPCS.has(tag)) {
      out[tag] = handler;
      continue;
    }
    const refuse = streams.has(tag) ? () => Stream.fail(unauthenticated("unproved")) : () => timed(tag, Effect.fail(unauthenticated("unproved")));
    out[tag] = (payload, options) =>
      connections.principalOf(options.client.id).kind === "anonymous" ? refuse() : handler(payload, options);
  }
  return out as H;
}
