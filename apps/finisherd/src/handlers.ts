// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { type FinError, FinRpcs, PROTOCOL_VERSION, type Probe } from "@finisher/contracts";
import { probe } from "@finisher/fin-node";
import { Effect, Stream } from "effect";
import { SERVER_VERSION } from "./config.ts";
import { callMeta, Diagnostics, RpcSource } from "./diagnostics.ts";
import { attemptJson, finError, toFinError } from "./errors.ts";
import { Fonts } from "./fonts.ts";
import { ProjectRegistry } from "./projects.ts";

/** Advertised in `server.hello`: the RPC tags this server implements. */
export const CAPABILITIES: ReadonlyArray<string> = [...FinRpcs.requests.keys()].sort();

/** Defects (thrown exceptions, bugs) become `INTERNAL` instead of an RPC defect frame. */
const defectsAsErrors = <A, R>(self: Effect.Effect<A, FinError, R>) =>
  Effect.catchDefect(self, (defect) => Effect.fail(toFinError(defect)));

/**
 * The handlers of one transport: built once per transport (`Layer.fresh`), with that transport's
 * `RpcSource` (local socket `fin`, WebSocket `ui`). Every non-stream call is timed into
 * `rpc.stats` (`guard`); the stream handlers do not use it.
 */
export const HandlersLive = FinRpcs.toLayer(
  Effect.gen(function* () {
    const registry = yield* ProjectRegistry;
    const fonts = yield* Fonts;
    const diagnostics = yield* Diagnostics;
    const source = yield* RpcSource;
    const guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => diagnostics.timed(tag, defectsAsErrors(self));
    return {
      "server.hello": () =>
        guard(
          "server.hello",
          Effect.succeed({
            protocolVersion: PROTOCOL_VERSION,
            serverVersion: SERVER_VERSION,
            capabilities: CAPABILITIES,
          }),
        ),
      "project.create": ({ dir, name, rate, idemKey }) => guard("project.create", registry.create(dir, name, rate, idemKey)),
      "project.open": ({ dir, relocate }) => guard("project.open", registry.open(dir, relocate)),
      "asset.probe": ({ path }) => guard("asset.probe", attemptJson<Probe>(() => probe(path))),
      "tx.commit": ({ projectId, tx, generation }, { headers }) =>
        guard("tx.commit", registry.commit(projectId, tx, generation, callMeta(headers))),
      "snapshot.get": ({ projectId, branch }) => guard("snapshot.get", registry.snapshot(projectId, branch)),
      // Errors (unknown project, bad range) travel in the stream's error channel.
      "ops.subscribe": ({ projectId, branch, fromRev, generation }) =>
        registry
          .subscribe(projectId, branch, fromRev, generation)
          .pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.events": ({ projectId }) =>
        registry.events(projectId).pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.status": ({ projectId }) => guard("project.status", registry.status(projectId)),
      "project.takeover": ({ projectId, confirm }) => guard("project.takeover", registry.takeover(projectId, confirm)),
      "project.verify": ({ projectId }) => guard("project.verify", registry.verify(projectId)),
      "project.rebuild": ({ projectId }) => guard("project.rebuild", registry.rebuild(projectId)),
      "project.compact": ({ projectId }) => guard("project.compact", registry.compact(projectId)),
      "project.recover": (payload, { headers }) =>
        guard("project.recover", registry.history(payload.projectId, "recover", payload, callMeta(headers))),
      "branch.create": (payload, { headers }) =>
        guard("branch.create", registry.history(payload.projectId, "branchCreate", payload, callMeta(headers))),
      "branch.list": ({ projectId, all }) => guard("branch.list", registry.branches(projectId, all === true)),
      "branch.set": (payload, { headers }) =>
        guard("branch.set", registry.history(payload.projectId, "branchSet", payload, callMeta(headers))),
      "history.undo": (payload, { headers }) =>
        guard("history.undo", registry.history(payload.projectId, "undo", payload, callMeta(headers))),
      "history.redo": (payload, { headers }) =>
        guard("history.redo", registry.history(payload.projectId, "redo", payload, callMeta(headers))),
      "history.checkpoint": (payload, { headers }) =>
        guard("history.checkpoint", registry.history(payload.projectId, "checkpoint", payload, callMeta(headers))),
      "history.merge": (payload, { headers }) =>
        guard("history.merge", registry.history(payload.projectId, "merge", payload, callMeta(headers))),
      "history.cherryPick": (payload, { headers }) =>
        guard("history.cherryPick", registry.history(payload.projectId, "cherryPick", payload, callMeta(headers))),
      "history.revert": (payload, { headers }) =>
        guard("history.revert", registry.history(payload.projectId, "revert", payload, callMeta(headers))),
      "project.acknowledgeRewind": ({ projectId, branch, generation }) =>
        guard("project.acknowledgeRewind", Effect.map(registry.acknowledgeRewind(projectId, branch, generation), (acknowledged) => ({ acknowledged }))),
      "project.acknowledgeSalvage": ({ projectId, copy }) =>
        guard("project.acknowledgeSalvage", Effect.map(registry.acknowledgeSalvage(projectId, copy), (acknowledged) => ({ acknowledged }))),
      "tx.get": ({ projectId, txId }) => guard("tx.get", registry.txGet(projectId, txId)),
      "eval.frame": ({ projectId, branch, timeline, at }) => guard("eval.frame", registry.evalFrame(projectId, branch ?? "main", timeline, at)),
      "eval.audio": ({ projectId, branch, timeline, from, to }) =>
        guard("eval.audio", registry.evalAudio(projectId, branch ?? "main", timeline, from, to)),
      "fonts.catalog": () => guard("fonts.catalog", fonts.catalog()),
      "fonts.status": () => guard("fonts.status", fonts.status()),
      // `pinned` needs the project open (its pins are the engine's): opened on demand.
      "fonts.fetch": (req) =>
        guard("fonts.fetch", Effect.andThen(req.pinned === undefined ? Effect.void : registry.ensure(req.pinned), fonts.fetch(req))),
      "fonts.plan": ({ projectId, branch, ops }) => guard("fonts.plan", registry.fontsPlan(projectId, branch, ops)),
      "diagnostics.status": () => guard("diagnostics.status", diagnostics.status()),
      "diagnostics.export": (req) => guard("diagnostics.export", diagnostics.export(req)),
      "diagnostics.purge": ({ selector }) => guard("diagnostics.purge", diagnostics.purge(selector)),
      "diagnostics.project": ({ projectId, collect }) =>
        guard(
          "diagnostics.project",
          Effect.andThen(registry.ensure(projectId), () => {
            const engine = registry.engineOf(projectId);
            if (engine === undefined) return Effect.fail(finError("PROJECT_NOT_FOUND", `project ${projectId} is not open`, { projectId }));
            return attemptJson<{ project: string; collect: "default" | "never" }>(() => engine.setCollect(collect));
          }),
        ),
      "diagnostics.consent": ({ purpose, value }) => guard("diagnostics.consent", diagnostics.consent(source, purpose, value)),
      "diagnostics.record": (req, { client, headers }) =>
        guard("diagnostics.record", diagnostics.record(source, client.id, req, headers)),
    };
  }),
);
