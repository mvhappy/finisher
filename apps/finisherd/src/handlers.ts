// `FinRpcs` handlers. Every failure reaches the client as `FinError { code, message, details, fix }`.
import { randomBytes } from "node:crypto";
import { type FinError, FinRpcs, PROTOCOL_VERSION } from "@finisher/contracts";
import { Effect, Option, Stream } from "effect";
import { SERVER_VERSION } from "./config.ts";
import { HandshakeTag, revokeSelf } from "./auth/handshake.ts";
import { TicketsTag } from "./auth/tickets.ts";
import { CredentialsTag } from "./auth/credentials.ts";
import { KEY_BYTES } from "./auth/crypto.ts";
import { revokeCredential } from "./auth/revoke.ts";
import { deviceHandlers } from "./auth/device-rpc.ts";
import { PairingTag } from "./auth/pairing.ts";
import { DeviceAccessTag } from "./net/remote-wiring.ts";
import { RemoteListenerTag } from "./net/remote.ts";
import { defaultProjectRoots, readServerSettings } from "./net/server-settings.ts";
import { settingsHandlers } from "./net/settings-rpc.ts";
import { EnvironmentTag, envMoved } from "./env/descriptor.ts";
import { envHandlers } from "./env/rpc.ts";
import { chainOf, credentialOf, withChain, writeGuardOf } from "./chain/chain.ts";
import { withoutVia } from "./chain/client-via.ts";
import { AuditTag } from "./control/audit.ts";
import { ConnectionRegistryTag, ConnectionsTag } from "./connections.ts";
import { issueChallenge, mayChallenge, noteMinted } from "./auth/challenges.ts";
import { actorFor, type CallMeta, Diagnostics } from "./diagnostics.ts";
import { attemptJson, finError, toFinError } from "./errors.ts";
import { agentAsk as agentAskPort } from "./agent/approvals.ts";
import { AgentCoreTag } from "./agent/core.ts";
import { withOutcomes } from "./agent/outcomes.ts";
import { agentChainHooks, BranchReach, PathReach } from "./agent/reach.ts";
import { withReviewLink } from "./agent/review-link.ts";
import { agentHandlers } from "./agent/rpc.ts";
import { commandRules } from "./agent/rules.ts";
import { rulesHandlers } from "./agent/rules-rpc.ts";
import { SavedPrompts } from "./agent/saved-prompts.ts";
import { savedPromptHandlers } from "./agent/saved-prompts-rpc.ts";
import { AgentRuntimeTag } from "./agent/wiring.ts";
import { agentBranchLimitsExcept } from "./chain/policy.ts";
import { protectedBranches } from "./chain/protected.ts";
import { Fonts } from "./fonts.ts";
import { JobEngineTag } from "./jobs/layer.ts";
import { chainJobs, jobHandlers } from "./jobs/rpc.ts";
import { MediaCacheTag } from "./media/cache.ts";
import { mediaCapabilities } from "./media/caps.ts";
import { look as mediaLook } from "./media/look.ts";
import { probeDeep } from "./media/probe-deep.ts";
import { MediaProxyTag, type ProxyService } from "./media/proxy.ts";
import { reprobe } from "./media/reprobe.ts";
import { requireAbsolute, requireAbsoluteOpt } from "./paths.ts";
import { dropPayload, metaJson } from "./history.ts";
import { LibraryServiceTag } from "./library/layer.ts";
import { cataloguedAt } from "./library/reach.ts";
import { libraryHandlers } from "./library/rpc.ts";
import { pluginHandlers } from "./plugins/rpc.ts";
import { commandHandlers, PluginCommandsTag } from "./plugins/host/command-rpc.ts";
import { txToCommit } from "./plugins/host/commit-channel.ts";
import type { PluginCommandRunRequest } from "@finisher/core-types";
import { PluginsTag } from "./plugins/registries.ts";
import { probeFor } from "./probe.ts";
import { ProjectRegistry } from "./projects.ts";
import { type AdminDeps, auditList, auditPurge, ServerControlTag, serverStatus, serverStop } from "./server-admin.ts";
import { reporterFor } from "./sandbox/status.ts";
import { observingArchive, observingMerge } from "./reviews/conflicts.ts";
import { principalKey } from "./reviews/workflow.ts";
import { reviewHandlers } from "./reviews/rpc.ts";
import { ReviewsTag } from "./reviews/service.ts";

/** Advertised in `server.hello`: the RPC tags this server implements. */
export const CAPABILITIES: ReadonlyArray<string> = [...FinRpcs.requests.keys()].sort();

/** The audit's word for a purge selector (never the project id or the date). */
const purgeReason = (selector: unknown): string =>
  selector === "all" ? "all" : selector !== null && typeof selector === "object" && "before" in selector ? "before" : "project";

/** Defects (thrown exceptions, bugs) become `INTERNAL` instead of an RPC defect frame. */
const defectsAsErrors = <A, R>(self: Effect.Effect<A, FinError, R>) =>
  Effect.catchDefect(self, (defect) => Effect.fail(toFinError(defect)));

/**
 * The handlers of one transport: built once per transport (`Layer.fresh`), with that transport's
 * `Connections`. The journal's source and a call's actor come from the call's principal
 * (`sourceOf`, `actorFor`; 5A Decision 27), never from the transport. Every non-stream call is
 * timed into `rpc.stats` (`guard`); the stream handlers do not use it. Every RPC passes the
 * middleware chain first (`withChain`, 5A Task 5): authentication, resolvers, scopes, reach and
 * policy; every write takes the chain's guard to the store (`meta`).
 */
export const HandlersLive = FinRpcs.toLayer(
  Effect.gen(function* () {
    const registry = yield* ProjectRegistry;
    const fonts = yield* Fonts;
    const diagnostics = yield* Diagnostics;
    const connections = yield* ConnectionsTag;
    const handshake = yield* HandshakeTag;
    const audit = yield* AuditTag;
    const plugins = yield* PluginsTag;
    const engine = yield* JobEngineTag;
    const tickets = yield* TicketsTag;
    const everyConnection = yield* ConnectionRegistryTag;
    const credentials = yield* CredentialsTag;
    const library = yield* LibraryServiceTag;
    const jobEngine = yield* JobEngineTag;
    const agents = yield* AgentCoreTag;
    const agentRuntime = yield* AgentRuntimeTag;
    const reviews = yield* ReviewsTag;
    const environment = yield* EnvironmentTag;
    const pairing = yield* PairingTag;
    const deviceAccess = yield* DeviceAccessTag;
    const remoteListener = yield* RemoteListenerTag;
    const mediaCache = yield* MediaCacheTag;
    // Plugin commands (sp8b Task 9): served only where the plugin host runs.
    const pluginCommands = Option.getOrUndefined(yield* Effect.serviceOption(PluginCommandsTag));
    const admin: AdminDeps = {
      control: yield* ServerControlTag,
      audit,
      connections: everyConnection,
      journalEnabled: () => diagnostics.enabled(),
      jobCaps: () => jobEngine.workers().caps,
      sandboxReporter: reporterFor(diagnostics.journal),
      ...(pluginCommands?.enabledCode === undefined ? {} : { enabledCodePlugins: pluginCommands.enabledCode }),
    };
    /** The principal the chain bound to this call (`anonymous` outside it: fail closed). */
    const principalOf = (options: object) => chainOf(options)?.principal ?? connections.principalOf((options as { client: { id: number } }).client.id);
    const guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => diagnostics.timed(tag, defectsAsErrors(self));
    /**
     * `f` with a write's `metaJson`: its actor and the chain's write guard. A write that did not
     * pass the chain, or a branch-limited principal's write without an allow-list, is a defect
     * (`INTERNAL`), never an open guard (review of Task 5, H1 and L3).
     */
    const withMeta = <A>(options: { readonly headers: unknown }, f: (meta: CallMeta) => Effect.Effect<A, FinError>) =>
      Effect.flatMap(writeGuardOf(options), (guard) => f({ ...actorFor(principalOf(options), options.headers), guard }));
    /**
     * `f` with a `metaJson` whose guard names only the caller's credential, for the changes made
     * outside `Store::write` (review of 5A Tasks 8–9, L1): refused once the credential is ended.
     * A call that did not pass the chain is a defect, as a write's is.
     */
    const withCredential = <A>(options: { readonly headers: unknown }, f: (meta: CallMeta) => Effect.Effect<A, FinError>) =>
      Effect.suspend(() => {
        const c = chainOf(options);
        if (c === undefined) return Effect.die(new Error("a credential-checked call reached its handler without the chain"));
        const credential = credentialOf(c.principal);
        return f({ ...actorFor(c.principal, options.headers), guard: { denyBranches: [], ...(credential === undefined ? {} : { credential }) } });
      });
    /** An `ok` audit row of `kind` for the call's principal (consent changes, purges). */
    const auditAs = (options: object, kind: "consent.change" | "diagnostics.purge" | "history.drop", details: Record<string, string | number>) => {
      const p = principalOf(options);
      audit.append({ kind, principal: { kind: p.kind, id: p.id }, outcome: "ok", details: { ...details, transport: connections.transport } });
    };
    /** The owner's revocations (`auth.credential.revoke`, `rotateMaster`): the one revoke path. */
    const ownerRevoke = {
      audit,
      tickets,
      connections: everyConnection,
      revoke: (ref: { kind: "agent" | "mcp"; id: string }) => credentials.markEnded(ref, "owner"),
      persist: () => credentials.persist(),
    };
    // `media.look` and `media.capabilities` (6A Task 13): jobs on the engine's `media` family.
    const mediaDeps = { registry, engine };
    // 6B-1 Task 14: the server's one capability cache and its proxy service, when it has them.
    const mediaProxy = Option.getOrUndefined(yield* Effect.serviceOption(MediaProxyTag));
    const capabilitiesOf = mediaProxy?.capabilities ?? mediaCapabilities(mediaDeps);
    const proxiesOf = <A>(f: (p: ProxyService) => Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
      mediaProxy === undefined ? Effect.fail(finError("SERVER_UNAVAILABLE", "proxies are not served here", { feature: "mediaProxy" })) : f(mediaProxy.proxies);
    const ownerOf = (options: object) => {
      const p = principalOf(options);
      return { kind: p.kind, id: p.id };
    };
    const projects = {
      isOpen: registry.isOpen,
      dirOf: registry.dirOf,
      indexedDirOf: registry.indexedDirOf,
      resolveBranch: registry.resolveBranch,
      branches: (projectId: string) => Effect.map(registry.branches(projectId, true), (r) => r.branches),
      // 5D Decision 47: reach to a library file is a catalogued row, never a path prefix.
      libraryFile: (path: string) => cataloguedAt(library, path),
      // 5B Decision 32: a device's roots, read live (`settings.write` changes them).
      allowedRoots: () =>
        Effect.sync(() => [
          ...(admin.control.serverData === undefined ? defaultProjectRoots() : readServerSettings(admin.control.serverData).projectRoots),
          ...library.registry.list().map((l) => l.root),
          ...registry.openDirs(),
        ]),
    };
    // Agent sessions (7A Task 7): branch reach over their root's subtree, path reach over their
    // project folder and its libraries, the server's author, `session_txs` after every write.
    const reach = new BranchReach(agents.store, agents.lineage, projects);
    // Then the outcome tracker (Task 11), once the session's own rows are written.
    // Then the review link (7B Task 11): a session's `review.request` is recorded and `branchReady` emitted.
    const tracked = withOutcomes(agentChainHooks({ reach, paths: new PathReach(agents.store, projects), sessions: agents.store, clock: agents.clock.now }), agents.outcomes, projects);
    const hooks = withReviewLink(tracked, agents.reviewLink, reach);
    // Then command rules (7B Task 2), for agent principals only; a person's allow is a one-time grant (Task 5).
    // Then protected branches (5E Task 6), for every principal without `branch.protected`.
    const policies = [reach.policy, agentBranchLimitsExcept((p) => reach.covers(p)), protectedBranches, commandRules(agentRuntime.rules, agents.store, { grants: agentRuntime.grants })];
    // A running session's rule ask becomes a durable card (7B Task 4): `APPROVAL_PENDING`.
    const agentAsk = agentAskPort(agentRuntime.approvals, agents.store, agentRuntime.approvalKey);
    /** An agent session's default branch, for `auth.whoami` (Decision 14a); none for any other principal. */
    const defaultBranchOf = (p: { readonly kind: string; readonly id: string }) => {
      const s = p.kind === "agent" ? agents.store.get(p.id) : undefined;
      return s === undefined ? {} : { defaultBranch: agents.store.state(s.id).defaultBranch };
    };
    /** The chained handlers, for a plugin command's commit as its invoker (set below). */
    let chained: Record<string, (payload: unknown, options: object) => Effect.Effect<unknown, FinError>> = {};
    const commands = commandHandlers({
      service: pluginCommands,
      projects: registry,
      chained: () => chained,
      sessionRootOf: (p) => (p.kind === "agent" ? agents.store.get(p.id)?.rootBranch : undefined),
      authorOf: (p) => hooks.authorOf?.(p),
      guard,
    });
    const served = withChain(FinRpcs.of({
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
          Effect.try({ try: () => {
            const principal = principalOf(options);
            // `TICKET_LIMIT` past the principal's mint rate or the global cap.
            const minted = tickets.mint(principal, ttlS, `${connections.transport}:${options.client.id}`);
            audit.append({
              kind: "auth.ticket",
              principal: { kind: principal.kind, id: principal.id },
              outcome: "ok",
              details: { transport: connections.transport },
            });
            return minted;
          }, catch: toFinError }),
        ),
      "auth.whoami": (_, options) =>
        guard(
          "auth.whoami",
          Effect.sync(() => {
            const principal = principalOf(options);
            return { principal, ...defaultBranchOf(principal) };
          }),
        ),
      "auth.revokeSelf": (_, options) => {
        const self = principalOf(options);
        // A device revokes itself through the one device path (5B Decision 24); its own
        // connection answers, then closes.
        if (self.kind === "device") {
          return guard(
            "auth.revokeSelf",
            Effect.sync(() => {
              deviceAccess.revoke(self.id, {
                reason: "self",
                by: { kind: self.kind, id: self.id },
                except: { transport: connections.transport, clientId: options.client.id },
              });
              connections.link(options.client.id)?.closeAfterReply();
              return {};
            }),
          );
        }
        return guard(
          "auth.revokeSelf",
          revokeSelf(
            {
              audit,
              tickets,
              connections: everyConnection,
              ...(handshake.deps.revoke === undefined ? {} : { revoke: handshake.deps.revoke }),
              persist: () => credentials.persist(),
            },
            principalOf(options),
            {
              transport: connections.transport,
              clientId: options.client.id,
              closeAfterReply: () => connections.link(options.client.id)?.closeAfterReply(),
            },
          ),
        );
      },
      // The console's confirmations (Decision 30): a code for this connection only, which the
      // console sends itself once the person has confirmed.
      "console.challenge": (_, options) =>
        guard(
          "console.challenge",
          Effect.suspend(() => {
            const c = connections.get(options.client.id);
            // Every human principal (5B Decision 22): the console and paired devices.
            if (!mayChallenge(principalOf(options)) || c === undefined) {
              return Effect.fail(finError("INVALID_REQUEST", "console.challenge is for the server's console and paired devices", { tag: "console.challenge" }));
            }
            return Effect.succeed(issueChallenge(c));
          }),
        ),
      // The server itself and the owner's audit log (5A Task 8, `server-admin.ts`).
      "server.status": () => guard("server.status", serverStatus(admin)),
      "server.stop": (_, options) => guard("server.stop", serverStop(admin, connections.link(options.client.id)?.afterReply)),
      "auth.audit.list": (req) => guard("auth.audit.list", auditList(admin, req)),
      "auth.audit.purge": (req, options) => guard("auth.audit.purge", auditPurge(admin, req, principalOf(options), connections.transport)),
      // The owner's agent and MCP credentials (5A Task 9, `auth/credentials.ts`): revoking goes
      // the one revoke path (the store, tickets, connections and streams, writes in flight).
      "auth.credential.mint": (req, options) =>
        guard(
          "auth.credential.mint",
          Effect.try({
            try: () => {
              const minted = credentials.mint(principalOf(options), req);
              noteMinted(connections.get(options.client.id), minted.credential.id);
              return minted;
            },
            catch: toFinError,
          }),
        ),
      "auth.credential.list": ({ all }) => guard("auth.credential.list", Effect.sync(() => ({ credentials: credentials.list(all === true) }))),
      "auth.credential.revoke": ({ id }) =>
        guard(
          "auth.credential.revoke",
          Effect.sync(() => {
            const kind = credentials.kindOf(id);
            if (kind === undefined) return { revoked: false };
            return { revoked: revokeCredential(ownerRevoke, { kind, id }, { rpc: "auth.credential.revoke", reason: "owner" }).revoked };
          }),
        ),
      // A new `credential.master` (review of Tasks 8–9, M3): every credential ends with it.
      "auth.credential.rotateMaster": (_, options) =>
        guard(
          "auth.credential.rotateMaster",
          Effect.try({
            try: () => {
              const by = principalOf(options);
              let ended = 0;
              for (const ref of credentials.rotateMaster(randomBytes(KEY_BYTES))) {
                try {
                  if (revokeCredential(ownerRevoke, ref, { rpc: "auth.credential.rotateMaster", reason: "owner" }).revoked) ended++;
                } catch (e) {
                  // The new master already stops it from proving; its connections end at their next request.
                  process.stderr.write(`finisherd: warning: cannot end credential ${ref.id}: ${String(e)}\n`);
                }
              }
              audit.append({
                kind: "credential.rotateMaster",
                principal: { kind: by.kind, id: by.id },
                outcome: "ok",
                details: { count: ended, transport: connections.transport },
              });
              return { ended };
            },
            catch: toFinError,
          }),
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
          }).pipe(
            // A project open means looks are coming: a media worker is started now (a family or
            // a pool that cannot, or an engine that is closing, is nothing the caller needs).
            Effect.tap(() => Effect.ignore(engine.warm("media"))),
          ),
        ),
      // Protocol 2: the probe is for `projectId` (opened on demand, `PROJECT_NOT_FOUND` otherwise).
      // For a principal with roots the file is opened beneath its project's folder, through no
      // symlink: one swapped in after the chain's check, or a dangling one, is refused (`probe.ts`).
      "asset.probe": ({ projectId, path }, options) =>
        guard(
          "asset.probe",
          Effect.andThen(
            registry.ensure(projectId),
            Effect.flatMap(requireAbsolute("path", path), (p) => probeFor(chainOf(options)?.principal, projects, projectId, p)),
          ),
        ),
      // The chain already strips a client's `via`; this is the backstop for a chainless caller.
      // The plugin host's own commit (sp8b Task 9, `commit-channel.ts`) adds its `via`, or checks
      // everything and commits nothing for a dry run.
      "tx.commit": ({ projectId, tx, generation }, options) => {
        const sent = txToCommit(tx, options, withoutVia);
        return guard("tx.commit", withMeta(options, (m) => ("dryRun" in sent ? Effect.succeed(sent as never) : registry.commit(projectId, sent.tx, generation, m))));
      },
      "snapshot.get": ({ projectId, branch }) => guard("snapshot.get", registry.snapshot(projectId, branch)),
      // Errors (unknown project, bad range) travel in the stream's error channel.
      "ops.subscribe": ({ projectId, branch, fromRev, generation }) =>
        registry
          .subscribe(projectId, branch, fromRev, generation)
          .pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.events": ({ projectId }) =>
        registry.events(projectId).pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
      "project.status": ({ projectId }) => guard("project.status", registry.status(projectId)),
      "project.close": ({ projectId }) => guard("project.close", registry.close(projectId)),
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
        guard("branch.set", withMeta(options, (m) => registry.history(payload.projectId, "branchSet", payload, m).pipe(observingArchive(reviews.workflow, principalKey(principalOf(options)), payload)))),
      "history.undo": (payload, options) =>
        guard("history.undo", withMeta(options, (m) => registry.history(payload.projectId, "undo", payload, m))),
      "history.redo": (payload, options) =>
        guard("history.redo", withMeta(options, (m) => registry.history(payload.projectId, "redo", payload, m))),
      "history.checkpoint": (payload, options) =>
        guard("history.checkpoint", withMeta(options, (m) => registry.history(payload.projectId, "checkpoint", payload, m))),
      "history.merge": (payload, options) =>
        guard("history.merge", withMeta(options, (m) => registry.history(payload.projectId, "merge", payload, m).pipe(observingMerge(reviews.workflow, principalKey(principalOf(options)), payload)))),
      "history.cherryPick": (payload, options) =>
        guard("history.cherryPick", withMeta(options, (m) => registry.history(payload.projectId, "cherryPick", payload, m))),
      "history.revert": (payload, options) =>
        guard("history.revert", withMeta(options, (m) => registry.history(payload.projectId, "revert", payload, m))),
      // 5E: a rejection that outlives the review. Audited with a count only: no review text, no txIds.
      "history.drop": (payload, options) =>
        guard(
          "history.drop",
          Effect.tap(
            withMeta(options, (m) => registry.history(payload.projectId, "drop", dropPayload(payload), m)),
            () => Effect.sync(() => (payload.dryRun === true ? undefined : auditAs(options, "history.drop", { count: (payload.drops?.length ?? 0) + (payload.undrops?.length ?? 0) }))),
          ),
        ),
      // Not records, but changes a revoked credential must not make either (L1): the guard's credential.
      "project.acknowledgeRewind": ({ projectId, branch, generation }, options) =>
        guard(
          "project.acknowledgeRewind",
          withCredential(options, (meta) => Effect.map(registry.acknowledgeRewind(projectId, branch, generation, meta), (acknowledged) => ({ acknowledged }))),
        ),
      "project.acknowledgeSalvage": ({ projectId, copy }, options) =>
        guard("project.acknowledgeSalvage", withCredential(options, (meta) => Effect.map(registry.acknowledgeSalvage(projectId, copy, meta), (acknowledged) => ({ acknowledged })))),
      "tx.get": ({ projectId, txId }) => guard("tx.get", registry.txGet(projectId, txId)),
      "history.log": ({ projectId, branch, before, limit }) => guard("history.log", registry.historyLog(projectId, branch, before, limit)),
      "eval.frame": ({ projectId, branch, timeline, at, rev }) =>
        guard("eval.frame", registry.evalFrame(projectId, branch ?? "main", timeline, at, rev)),
      "eval.audio": ({ projectId, branch, timeline, from, to, rev }) =>
        guard("eval.audio", registry.evalAudio(projectId, branch ?? "main", timeline, from, to, rev)),
      "media.look": (req, options) => guard("media.look", mediaLook(mediaDeps, ownerOf(options), req)),
      "media.capabilities": (_, options) => guard("media.capabilities", capabilitiesOf(ownerOf(options))),
      // The media cache (6B-1 Task 13): its state, and an eviction down to the cap or `toGiB`.
      "media.cache.status": () => guard("media.cache.status", mediaCache.status()),
      "media.cache.gc": ({ toGiB }) => guard("media.cache.gc", mediaCache.gc(toGiB)),
      "media.probeDeep": (req, options) => guard("media.probeDeep", probeDeep({ ...mediaDeps, capabilities: capabilitiesOf }, ownerOf(options), req)),
      // 6B-1 Task 14: a proxy of a project asset, and the state of its tiers.
      "media.proxy": (req, options) => guard("media.proxy", proxiesOf((p) => p.request(ownerOf(options), req))),
      "media.proxy.status": (req, options) => guard("media.proxy.status", proxiesOf((p) => p.status(ownerOf(options), req))),
      "asset.reprobe": (req, options) =>
        guard(
          "asset.reprobe",
          withMeta(options, (m) => reprobe({ ...mediaDeps, registry, capabilities: capabilitiesOf, projects }, chainOf(options)?.principal, ownerOf(options), m, req)),
        ),
      "fonts.catalog": () => guard("fonts.catalog", fonts.catalog()),
      "fonts.status": () => guard("fonts.status", fonts.status()),
      // `pinned` needs the project open (its pins are the engine's): opened on demand.
      "fonts.fetch": (req) =>
        guard("fonts.fetch", Effect.andThen(req.pinned === undefined ? Effect.void : registry.ensure(req.pinned), fonts.fetch(req))),
      "fonts.plan": ({ projectId, branch, ops }) => guard("fonts.plan", registry.fontsPlan(projectId, branch, ops)),
      "diagnostics.status": () => guard("diagnostics.status", diagnostics.status()),
      // Content only on explicit request (`content: true`, human-only); otherwise, and for a caller
      // that is not a person, it is removed (Decision 22; owner decision, review of Task 7 L4).
      "diagnostics.export": (req, options) => guard("diagnostics.export", diagnostics.export(req, !principalOf(options).human || req.content !== true)),
      "diagnostics.purge": ({ selector }, options) =>
        guard(
          "diagnostics.purge",
          Effect.tap(diagnostics.purge(selector), () =>
            Effect.sync(() => auditAs(options, "diagnostics.purge", { reason: purgeReason(selector) })),
          ),
        ),
      "diagnostics.project": ({ projectId, collect }, options) =>
        guard(
          "diagnostics.project",
          Effect.andThen(registry.ensure(projectId), () => {
            const engine = registry.engineOf(projectId);
            if (engine === undefined) return Effect.fail(finError("PROJECT_NOT_FOUND", `project ${projectId} is not open`, { projectId }));
            return withCredential(options, (meta) => attemptJson<{ project: string; collect: "default" | "never" }>(() => engine.setCollect(collect, metaJson(meta))));
          }),
        ),
      "diagnostics.consent": ({ purpose, value }, options) =>
        guard(
          "diagnostics.consent",
          Effect.tap(diagnostics.consent(principalOf(options), purpose, value), () => Effect.sync(() => auditAs(options, "consent.change", { purpose, value }))),
        ),
      "diagnostics.record": (req, options) =>
        guard(
          "diagnostics.record",
          diagnostics.record({ principal: principalOf(options), connection: `${connections.transport}:${options.client.id}` }, req, options.headers),
        ),
      ...pluginHandlers({ plugins, audit, guard }),
      "plugin.commands": (payload, options) => commands["plugin.commands"](payload, options),
      "plugin.command.run": (payload, options) => commands["plugin.command.run"](payload as PluginCommandRunRequest, options),
      ...libraryHandlers({ service: library, engine: jobEngine, guard }),
      // The job engine (5C-2): ownership and reach per job in `jobs/rpc.ts`.
      ...jobHandlers({ engine: jobEngine, guard }),
      ...agentHandlers({ guard, store: agents.store, reach, agents: agentRuntime, principalOf, protect: { gate: reviews.gate, protectedBranches: reviews.chain.protectedBranches } }),
      ...rulesHandlers({ guard, store: agents.store, rules: agentRuntime.rules, ruleStore: agentRuntime.ruleStore, audit, clock: agents.clock, roles: agentRuntime.roles, dirOf: (id) => registry.dirOf(id) ?? registry.indexedDirOf(id), principalOf }),
      ...savedPromptHandlers({ guard, runtime: agentRuntime.runtime, principalOf, prompts: new SavedPrompts({ serverData: agentRuntime.rules.serverData, roles: agentRuntime.roles, dirOf: (id) => registry.dirOf(id) ?? registry.indexedDirOf(id), trustOf: (id) => agentRuntime.ruleStore.trustOf(id) }) }),
      // 5B: pairing, devices, settings writes and the environment.
      ...reviewHandlers({ guard, reviews, projects, audit, transport: connections.transport, roles: () => new Set(agentRuntime.roles.keys()) }),
      ...deviceHandlers({ guard, pairing, access: deviceAccess, connections }),
      ...settingsHandlers({ guard, serverData: admin.control.serverData ?? "", remote: remoteListener, audit }),
      ...envHandlers({ guard, env: environment }),
    }), {
      connections,
      projects,
      jobs: chainJobs(jobEngine),
      reviews: reviews.chain,
      gate: reviews.gate,
      audit,
      timed: guard,
      touch: (p) => credentials.touch(p.id),
      hooks,
      policies,
      agentAsk,
      // Copied or moved server data refuses devices until its owner decides (5B Decision 31).
      refuse: (p) => (p.kind === "device" && environment.state() === "moved" ? envMoved() : undefined),
      pluginCommands: { writes: (p, name) => (pluginCommands === undefined ? Effect.succeed(undefined) : pluginCommands.registry.writes(p, name)) },
    });
    chained = served as unknown as typeof chained;
    return served;
  }),
);
