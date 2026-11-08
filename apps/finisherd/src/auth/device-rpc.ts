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
import type { DevicesWatchItem } from "@finisher/contracts";
import { PubSub } from "effect";
import type { Transport } from "../connections.ts";
import type { DeviceAccess } from "../net/remote-wiring.ts";
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

/** `auth.devices.watch`: the snapshot first, then each change as the store announces it. */
function watchDevices(access: DeviceAccess): Stream.Stream<DevicesWatchItem> {
  return Stream.unwrap(
    Effect.gen(function* () {
      const bus = yield* Effect.acquireRelease(PubSub.sliding<DevicesWatchItem>(256), (b) => PubSub.shutdown(b));
      yield* Effect.acquireRelease(
        Effect.sync(() => access.devices.onEvent((e) => Effect.runSync(PubSub.publish(bus, e as DevicesWatchItem)))),
        (stop) => Effect.sync(stop),
      );
      const live = yield* PubSub.subscribe(bus);
      const snapshot: DevicesWatchItem = { kind: "snapshot", devices: [...access.devices.list()] };
      return Stream.concat(Stream.make(snapshot), Stream.fromSubscription(live));
    }),
  );
}

/** The transport's connections, to answer a device that revokes itself before its connection closes. */
interface CallerLinks {
  readonly transport: Transport;
  link(clientId: number): { closeAfterReply(): void } | undefined;
}

export function deviceHandlers({ guard, pairing, access, connections }: { readonly guard: Guard; readonly pairing: Pairing; readonly access: DeviceAccess; readonly connections: CallerLinks }) {
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
    // Devices (Task 9): the owner's list, names and revocations; the watch stream.
    "auth.device.list": ({ all }: { all?: boolean | undefined }) => run("auth.device.list", () => ({ devices: [...access.devices.list(all === true)] })),
    "auth.device.rename": ({ id, name }: { id: string; name: string }, options: object) =>
      run("auth.device.rename", () => ({ device: access.devices.rename(id, name, principalOf(options)) })),
    "auth.device.revoke": ({ id }: { id: string }, options: object) =>
      run("auth.device.revoke", () => {
        const by = principalOf(options);
        const self = by.kind === "device" && by.id === id;
        const clientId = (options as { client: { id: number } }).client.id;
        const r = access.revoke(id, {
          reason: self ? "self" : "owner",
          by: { kind: by.kind, id: by.id },
          // A device revoking itself gets its answer; its connection closes after it.
          ...(self ? { except: { transport: connections.transport, clientId } } : {}),
        });
        if (self) connections.link(clientId)?.closeAfterReply();
        return { revoked: r.revoked };
      }),
    "auth.devices.watch": () => watchDevices(access),
    "settings.write": unary(guard, "settings.write"),
  };
}
