// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { FinRpcs, PROTOCOL_VERSION, type Probe } from "@finisher/contracts";
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
      "project.create": ({ dir, name, rate, idemKey }) => guard(registry.create(dir, name, rate, idemKey)),
      "project.open": ({ dir, relocate }) => guard(registry.open(dir, relocate)),
      "asset.probe": ({ path }) => guard(attemptJson<Probe>(() => probe(path))),
      "tx.commit": ({ projectId, tx, generation }) => guard(registry.commit(projectId, tx, generation)),
      "snapshot.get": ({ projectId, branch }) => guard(registry.snapshot(projectId, branch)),
      // Errors (unknown project, bad range) travel in the stream's error channel.
      "ops.subscribe": ({ projectId, branch, fromRev, generation }) =>
        registry
          .subscribe(projectId, branch, fromRev, generation)
          .pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.events": ({ projectId }) =>
        registry.events(projectId).pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.status": ({ projectId }) => guard(registry.status(projectId)),
      "project.takeover": ({ projectId, confirm }) => guard(registry.takeover(projectId, confirm)),
      "project.verify": ({ projectId }) => guard(registry.verify(projectId)),
      "project.rebuild": ({ projectId }) => guard(registry.rebuild(projectId)),
      "project.compact": ({ projectId }) => guard(registry.compact(projectId)),
      "project.recover": (payload) => guard(registry.history(payload.projectId, "recover", payload)),
      "branch.create": (payload) => guard(registry.history(payload.projectId, "branchCreate", payload)),
      "branch.list": ({ projectId, all }) => guard(registry.branches(projectId, all === true)),
      "branch.set": (payload) => guard(registry.history(payload.projectId, "branchSet", payload)),
      "history.undo": (payload) => guard(registry.history(payload.projectId, "undo", payload)),
      "history.redo": (payload) => guard(registry.history(payload.projectId, "redo", payload)),
      "history.checkpoint": (payload) => guard(registry.history(payload.projectId, "checkpoint", payload)),
      "history.merge": (payload) => guard(registry.history(payload.projectId, "merge", payload)),
      "history.cherryPick": (payload) => guard(registry.history(payload.projectId, "cherryPick", payload)),
      "history.revert": (payload) => guard(registry.history(payload.projectId, "revert", payload)),
      "project.acknowledgeRewind": ({ projectId, branch, generation }) =>
        guard(Effect.map(registry.acknowledgeRewind(projectId, branch, generation), (acknowledged) => ({ acknowledged }))),
      "project.acknowledgeSalvage": ({ projectId, copy }) =>
        guard(Effect.map(registry.acknowledgeSalvage(projectId, copy), (acknowledged) => ({ acknowledged }))),
      "tx.get": ({ projectId, txId }) => guard(registry.txGet(projectId, txId)),
    };
  }),
);
