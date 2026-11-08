// 5B's RPC handlers (`DEVICE_RPCS`), spread into `handlers.ts`. Task 1 lands the contracts and
// the `devices` store only: until each RPC's task, its handler answers `INTERNAL {reason:
// notYet}` so the handler map stays complete. Importing this module registers the `devices`
// control chain (`devices.ts`).
import type { FinError } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import { finError } from "../errors.ts";
import "./devices.ts";

const notYet = (tag: string): FinError =>
  finError("INTERNAL", `${tag} is not available on this server yet`, { tag, reason: "notYet" });

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;

const unary = (guard: Guard, tag: string) => () => guard(tag, Effect.fail(notYet(tag)));
const stream = (tag: string) => () => Stream.fail(notYet(tag));

export function deviceHandlers({ guard }: { readonly guard: Guard }) {
  return {
    "auth.pair.offer": unary(guard, "auth.pair.offer"),
    "auth.pair.approve": unary(guard, "auth.pair.approve"),
    "auth.pair.reject": unary(guard, "auth.pair.reject"),
    "auth.pair.pending": stream("auth.pair.pending"),
    "auth.device.list": unary(guard, "auth.device.list"),
    "auth.device.rename": unary(guard, "auth.device.rename"),
    "auth.device.revoke": unary(guard, "auth.device.revoke"),
    "auth.devices.watch": stream("auth.devices.watch"),
    "settings.write": unary(guard, "settings.write"),
  };
}

export function envHandlers({ guard }: { readonly guard: Guard }) {
  return {
    "env.describe": unary(guard, "env.describe"),
    "env.name": unary(guard, "env.name"),
    "env.adopt": unary(guard, "env.adopt"),
    "env.reset": unary(guard, "env.reset"),
    "env.rotateIdentity": unary(guard, "env.rotateIdentity"),
  };
}
