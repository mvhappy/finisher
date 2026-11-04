// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { FinRpcs, PROTOCOL_VERSION } from "@finisher/contracts";
import { probe } from "@finisher/fin-node";
import { Effect, Stream } from "effect";
import { SERVER_VERSION } from "./config.ts";
import { attemptJson, toFinError } from "./errors.ts";
import { ProjectRegistry } from "./projects.ts";

/** Advertised in `server.hello`: the RPC tags this server implements. */
export const CAPABILITIES: ReadonlyArray<string> = [...FinRpcs.requests.keys()].sort();

/** Defects (thrown exceptions, bugs) become `INTERNAL` instead of an RPC defect frame. */
const guard = <A, E, R>(self: Effect.Effect<A, E, R>) =>
  Effect.catchDefect(self, (defect) => Effect.fail(toFinError(defect)));

export const HandlersLive = FinRpcs.toLayer(
  Effect.gen(function* () {
    const registry = yield* ProjectRegistry;
    return {
      "server.hello": () =>
        Effect.succeed({
          protocolVersion: PROTOCOL_VERSION,
          serverVersion: SERVER_VERSION,
          capabilities: CAPABILITIES,
        }),
      "project.create": ({ dir, name, rate }) => guard(registry.create(dir, name, rate)),
      "project.open": ({ dir }) => guard(registry.open(dir)),
      "asset.probe": ({ path }) => guard(attemptJson<unknown>(() => probe(path))),
      "tx.commit": ({ projectId, tx }) => guard(registry.commit(projectId, tx)),
      "snapshot.get": ({ projectId, branch }) => guard(registry.snapshot(projectId, branch)),
      // Errors (unknown project, bad range) travel in the stream's error channel.
      "ops.subscribe": ({ projectId, branch, fromRev }) =>
        registry
          .subscribe(projectId, branch, fromRev)
          .pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
    };
  }),
);
