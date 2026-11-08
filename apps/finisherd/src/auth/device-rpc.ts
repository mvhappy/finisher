// 5B's pairing, device and settings handlers, spread into `handlers.ts` (the `env.*` ones are
// `env/rpc.ts`). Task 1 lands the contracts and
// the `devices` store only: until each RPC's task, its handler answers `INTERNAL {reason:
// notYet}` so the handler map stays complete. Importing this module registers the `devices`
// control chain (`devices.ts`).
import type { FinError } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import type { Principal } from "@finisher/contracts";
import { chainOf } from "../chain/chain.ts";
import { finError, toFinError } from "../errors.ts";
import type { Pairing } from "./pairing.ts";
import "./devices.ts";

const notYet = (tag: string): FinError =>
  finError("INTERNAL", `${tag} is not available on this server yet`, { tag, reason: "notYet" });

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;

const unary = (guard: Guard, tag: string) => () => guard(tag, Effect.fail(notYet(tag)));
const stream = (tag: string) => () => Stream.fail(notYet(tag));

/** The principal the chain bound to this call. */
const principalOf = (options: object): Principal => {
  const p = chainOf(options)?.principal;
  if (p === undefined) throw new Error("a pairing call reached its handler without the chain");
  return p;
};

export function deviceHandlers({ guard, pairing }: { readonly guard: Guard; readonly pairing: Pairing }) {
  const run = <A>(tag: string, f: () => A) => guard(tag, Effect.try({ try: f, catch: toFinError }));
  return {
    // Pairing (Task 8): the owner's offers and answers; the chain checked scopes and codes.
    "auth.pair.offer": (req: { label?: string | undefined; ttlS?: number | undefined; self?: boolean | undefined }, options: object) =>
      run("auth.pair.offer", () => pairing.offer(principalOf(options), req)),
    "auth.pair.approve": ({ pendingId }: { pendingId: string }, options: object) =>
      run("auth.pair.approve", () => ({ device: pairing.approve(principalOf(options), pendingId) })),
    "auth.pair.reject": ({ pendingId }: { pendingId: string }, options: object) =>
      run("auth.pair.reject", () => {
        pairing.reject(principalOf(options), pendingId);
        return { rejected: true as const };
      }),
    "auth.pair.pending": () => pairing.pending(),
    "auth.device.list": unary(guard, "auth.device.list"),
    "auth.device.rename": unary(guard, "auth.device.rename"),
    "auth.device.revoke": unary(guard, "auth.device.revoke"),
    "auth.devices.watch": stream("auth.devices.watch"),
    "settings.write": unary(guard, "settings.write"),
  };
}
