// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { FinRpcs, PROTOCOL_VERSION, type Probe } from "@finisher/contracts";
import { probe } from "@finisher/fin-node";
import { Effect, Stream } from "effect";
import { SERVER_VERSION } from "./config.ts";
import { attemptJson, toFinError } from "./errors.ts";
import { Fonts } from "./fonts.ts";
import { requireAbsolute, requireAbsoluteOpt } from "./paths.ts";
import { ProjectRegistry } from "./projects.ts";

/** Advertised in `server.hello`: the RPC tags this server implements. */
export const CAPABILITIES: ReadonlyArray<string> = [...FinRpcs.requests.keys()].sort();

/** Defects (thrown exceptions, bugs) become `INTERNAL` instead of an RPC defect frame. */
const guard = <A, E, R>(self: Effect.Effect<A, E, R>) =>
  Effect.catchDefect(self, (defect) => Effect.fail(toFinError(defect)));

export const HandlersLive = FinRpcs.toLayer(
  Effect.gen(function* () {
    const registry = yield* ProjectRegistry;
    const fonts = yield* Fonts;
    return {
      "server.hello": () =>
        Effect.succeed({
          protocolVersion: PROTOCOL_VERSION,
          serverVersion: SERVER_VERSION,
          capabilities: CAPABILITIES,
        }),
      // Paths are absolute and normalised, or `INVALID_PATH` (5A Decision 35).
      "project.create": ({ dir, name, rate, idemKey }) =>
        guard(Effect.flatMap(requireAbsolute("dir", dir), (d) => registry.create(d, name, rate, idemKey))),
      "project.open": ({ dir, relocate }) => guard(Effect.flatMap(requireAbsolute("dir", dir), (d) => registry.open(d, relocate))),
      "asset.probe": ({ path }) =>
        guard(Effect.flatMap(requireAbsolute("path", path), (p) => attemptJson<Probe>(() => probe(p)))),
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
      "project.recover": (payload) =>
        guard(Effect.andThen(requireAbsoluteOpt("importDir", payload.importDir), registry.history(payload.projectId, "recover", payload))),
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
      "eval.frame": ({ projectId, branch, timeline, at }) => guard(registry.evalFrame(projectId, branch ?? "main", timeline, at)),
      "eval.audio": ({ projectId, branch, timeline, from, to }) =>
        guard(registry.evalAudio(projectId, branch ?? "main", timeline, from, to)),
      "fonts.catalog": () => guard(fonts.catalog()),
      "fonts.status": () => guard(fonts.status()),
      // `pinned` needs the project open (its pins are the engine's): opened on demand.
      "fonts.fetch": (req) =>
        guard(Effect.andThen(req.pinned === undefined ? Effect.void : registry.ensure(req.pinned), fonts.fetch(req))),
      "fonts.plan": ({ projectId, branch, ops }) => guard(registry.fontsPlan(projectId, branch, ops)),
    };
  }),
);
