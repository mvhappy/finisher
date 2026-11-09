// The `env.*` RPCs (5B Decision 30), spread into `handlers.ts`. Every write is the owner's
// (`auth.admin`, human-only; the console types its code, a device needs none: Task 4).
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import { toFinError } from "../errors.ts";
import type { Environment } from "./descriptor.ts";

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;

/** Who called, as the audit names them (the chain's principal). */
const byOf = (options: object) => {
  const p = chainOf(options)?.principal;
  return { kind: p?.kind ?? "anonymous", id: p?.id ?? "" };
};

export function envHandlers({ guard, env }: { readonly guard: Guard; readonly env: Environment }) {
  const run = <A>(tag: string, f: () => A) => guard(tag, Effect.try({ try: f, catch: toFinError }));
  return {
    "env.describe": () => run("env.describe", () => env.describe()),
    "env.name": ({ name }: { name: string }) => run("env.name", () => env.name(name)),
    "env.adopt": (_: unknown, options: object) => run("env.adopt", () => env.adopt(byOf(options))),
    "env.reset": (_: unknown, options: object) => run("env.reset", () => env.reset(byOf(options))),
    "env.rotateIdentity": (_: unknown, options: object) => run("env.rotateIdentity", () => env.rotateIdentity("owner", byOf(options))),
  };
}
