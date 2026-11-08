// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { type FinError, FinRpcs, PROTOCOL_VERSION, type Probe } from "@finisher/contracts";
import { probe } from "@finisher/fin-node";
import { Effect, Stream } from "effect";
import { SERVER_VERSION } from "./config.ts";
import { HandshakeTag, revokeSelf } from "./auth/handshake.ts";
import { TicketsTag } from "./auth/tickets.ts";
import { chainOf, withChain, writeGuardOf } from "./chain/chain.ts";
import { stillCanonical } from "./chain/resolve.ts";
import { AuditTag } from "./control/audit.ts";
import { ConnectionsTag } from "./connections.ts";
import { type CallMeta, callMeta, Diagnostics, RpcSource } from "./diagnostics.ts";
import { attemptJson, finError, toFinError } from "./errors.ts";
import { Fonts } from "./fonts.ts";
import { requireAbsolute, requireAbsoluteOpt } from "./paths.ts";
import { ProjectRegistry } from "./projects.ts";

/** Advertised in `server.hello`: the RPC tags this server implements. */
export const CAPABILITIES: ReadonlyArray<string> = [...FinRpcs.requests.keys()].sort();

/** Defects (thrown exceptions, bugs) become `INTERNAL` instead of an RPC defect frame. */
const defectsAsErrors = <A, R>(self: Effect.Effect<A, FinError, R>) =>
  Effect.catchDefect(self, (defect) => Effect.fail(toFinError(defect)));

/**
 * The handlers of one transport: built once per transport (`Layer.fresh`), with that transport's
 * `RpcSource` (local socket `fin`, WebSocket `ui`) and `Connections`. Every non-stream call is
 * timed into `rpc.stats` (`guard`); the stream handlers do not use it. Every RPC passes the
 * middleware chain first (`withChain`, 5A Task 5): authentication, resolvers, scopes, reach and
 * policy; every write takes the chain's guard to the store (`meta`).
 */
export const HandlersLive = FinRpcs.toLayer(
  Effect.gen(function* () {
    const registry = yield* ProjectRegistry;
    const fonts = yield* Fonts;
    const diagnostics = yield* Diagnostics;
    const source = yield* RpcSource;
    const connections = yield* ConnectionsTag;
    const handshake = yield* HandshakeTag;
    const audit = yield* AuditTag;
    const tickets = yield* TicketsTag;
    /** The principal the chain bound to this call (`anonymous` outside it: fail closed). */
    const principalOf = (options: object) => chainOf(options)?.principal ?? connections.principalOf((options as { client: { id: number } }).client.id);
    const guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => diagnostics.timed(tag, defectsAsErrors(self));
    /**
     * `f` with a write's `metaJson`: its actor and the chain's write guard. A write that did not
     * pass the chain, or a branch-limited principal's write without an allow-list, is a defect
     * (`INTERNAL`), never an open guard (review of Task 5, H1 and L3).
     */
    const withMeta = <A>(options: { readonly headers: unknown }, f: (meta: CallMeta) => Effect.Effect<A, FinError>) =>
      Effect.flatMap(writeGuardOf(options), (guard) => f({ ...callMeta(options.headers, source), guard }));
    const projects = {
      isOpen: registry.isOpen,
      dirOf: registry.dirOf,
      resolveBranch: registry.resolveBranch,
      branches: (projectId: string) => Effect.map(registry.branches(projectId, true), (r) => r.branches),
    };
    return withChain(FinRpcs.of({
      // Public: the handshake (5A Decisions 8 and 9).
      "server.hello": (req, { client }) =>
        guard(
          "server.hello",
          Effect.map(handshake.hello(connections.get(client.id), connections.endpoint(), req), (proof) => ({
            protocolVersion: PROTOCOL_VERSION,
            serverVersion: SERVER_VERSION,
            capabilities: CAPABILITIES,
            ...proof,
          })),
        ),
      "auth.prove": (req, { client }) => guard("auth.prove", handshake.prove(connections, client.id, req)),
      // One-time WebSocket tickets (5A Decision 23), bound to the caller's principal.
      "auth.ticket": ({ ttlS }, options) =>
        guard(
          "auth.ticket",
          Effect.sync(() => {
            const principal = principalOf(options);
            const minted = tickets.mint(principal, ttlS);
            audit.append({
              kind: "auth.ticket",
              principal: { kind: principal.kind, id: principal.id },
              outcome: "ok",
              details: { transport: connections.transport },
            });
            return minted;
          }),
        ),
      "auth.whoami": (_, options) => guard("auth.whoami", Effect.sync(() => ({ principal: principalOf(options) }))),
      "auth.revokeSelf": (_, options) =>
        guard(
          "auth.revokeSelf",
          revokeSelf(
            handshake.deps,
            principalOf(options),
            () => connections.link(options.client.id)?.closeAfterReply(),
            (pred) => tickets.dropWhere(pred),
          ),
        ),
      // Paths are absolute and normalised, or `INVALID_PATH` (5A Decision 35), counted in rpc.stats.
      "project.create": ({ dir, name, rate, idemKey }) =>
        guard("project.create", Effect.flatMap(requireAbsolute("dir", dir), (d) => registry.create(d, name, rate, idemKey))),
      // The chain resolved `dir` to its real path and read the project it names: the registry
      // opens only that project, and only attaches when loading needed a scope not checked (H2).
      "project.open": ({ dir, relocate }, options) =>
        guard(
          "project.open",
          Effect.flatMap(requireAbsolute("dir", dir), (d) => {
            const c = chainOf(options);
            return registry.open(d, relocate, { projectId: c?.openedProject, attachOnly: c?.facts.attached === true });
          }),
        ),
      // Protocol 2: the probe is for `projectId` (opened on demand, `PROJECT_NOT_FOUND` otherwise).
      // For a principal with roots the checked path is re-checked around the probe: a symlink
      // swapped in after the chain's check is refused (review of Task 5, M2).
      "asset.probe": ({ projectId, path }, options) =>
        guard(
          "asset.probe",
          Effect.andThen(
            registry.ensure(projectId),
            Effect.flatMap(requireAbsolute("path", path), (p) => {
              const roots = chainOf(options)?.principal.projects !== "*";
              const check = roots ? stillCanonical("asset.probe", p) : Effect.void;
              return Effect.andThen(check, Effect.tap(attemptJson<Probe>(() => probe(p)), () => check));
            }),
          ),
        ),
      "tx.commit": ({ projectId, tx, generation }, options) =>
        guard("tx.commit", withMeta(options, (m) => registry.commit(projectId, tx, generation, m))),
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
      "project.recover": (payload, options) =>
        guard(
          "project.recover",
          Effect.andThen(
            requireAbsoluteOpt("importDir", payload.importDir),
            withMeta(options, (m) => registry.history(payload.projectId, "recover", payload, m)),
          ),
        ),
      "branch.create": (payload, options) =>
        guard("branch.create", withMeta(options, (m) => registry.history(payload.projectId, "branchCreate", payload, m))),
      "branch.list": ({ projectId, all }) => guard("branch.list", registry.branches(projectId, all === true)),
      "branch.set": (payload, options) =>
        guard("branch.set", withMeta(options, (m) => registry.history(payload.projectId, "branchSet", payload, m))),
      "history.undo": (payload, options) =>
        guard("history.undo", withMeta(options, (m) => registry.history(payload.projectId, "undo", payload, m))),
      "history.redo": (payload, options) =>
        guard("history.redo", withMeta(options, (m) => registry.history(payload.projectId, "redo", payload, m))),
      "history.checkpoint": (payload, options) =>
        guard("history.checkpoint", withMeta(options, (m) => registry.history(payload.projectId, "checkpoint", payload, m))),
      "history.merge": (payload, options) =>
        guard("history.merge", withMeta(options, (m) => registry.history(payload.projectId, "merge", payload, m))),
      "history.cherryPick": (payload, options) =>
        guard("history.cherryPick", withMeta(options, (m) => registry.history(payload.projectId, "cherryPick", payload, m))),
      "history.revert": (payload, options) =>
        guard("history.revert", withMeta(options, (m) => registry.history(payload.projectId, "revert", payload, m))),
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
    }), { connections, projects, audit, timed: guard });
  }),
);
