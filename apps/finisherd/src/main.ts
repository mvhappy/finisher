#!/usr/bin/env node
// finisherd: serves `FinRpcs` over the local socket/pipe and a ticketed WebSocket.
// Usage: node apps/finisherd/src/main.ts [serve] [--server-data <dir>]
//        node apps/finisherd/src/main.ts <command> …   (the owner's console, `console/main.ts`)
// Serving logs to stderr only; stdout stays empty. Console commands answer on stdout.
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { runMain } from "@effect/platform-node/NodeRuntime";
import { Cause, Context, Deferred, Effect, Exit, Layer, Logger } from "effect";
import {
  ensureServerData,
  parseArgs,
  cachedOrigins,
  removeServerInfo,
  type ServerInfo,
  startFailure,
  socketPath,
  writeServerInfo,
} from "./config.ts";
import { serverId } from "@finisher/fin-node";
import { canonicalEndpoint } from "./auth/crypto.ts";
import { Handshake, HandshakeTag } from "./auth/handshake.ts";
import { Credentials, CredentialsTag } from "./auth/credentials.ts";
import { sweepExpired } from "./auth/revoke.ts";
import { authDir, excludeFromBackups, KEY_FILES, KeysTag, rotateKeys, writeKeyFile } from "./auth/keys.ts";
import { ConnectionRegistry, ConnectionRegistryTag, Connections } from "./connections.ts";
import { AuditTag, layerAudit } from "./control/audit.ts";
import { makeTickets, TicketsTag } from "./auth/tickets.ts";
import { ControlStoreTag, layerControlStore } from "./control/db.ts";
// Registers the `agent` control chain before the store opens (7A-2 Task 3b; 6b wires the runtime).
import { AgentStore } from "./agent/store.ts";
import { systemClock, systemIds } from "./agent/clock.ts";
import { type AgentCore, AgentCoreTag } from "./agent/core.ts";
import { journalVerdicts, OutcomeTracker } from "./agent/outcomes.ts";
import { AgentJournal } from "./agent/journal.ts";
import { SessionCredentials } from "./agent/credentials.ts";
import { BranchLineage } from "./agent/reach.ts";
import { layerFonts, type FontsOptions } from "./fonts.ts";
import { Diagnostics, type DiagnosticsOptions, makeDiagnostics } from "./diagnostics.ts";
import { withServerLock } from "./lock.ts";
// Registers the `jobs` migration chain: imported before the control store opens.
import "./jobs/store.ts";
import { JobEventSinkTag } from "./jobs/events.ts";
import { journalSink } from "./jobs/journal-sink.ts";
import { JobEngineTag, type JobEngineOptions, layerJobEngine } from "./jobs/layer.ts";
import { LibrariesTag } from "./library/api.ts";
import { CatalogsTag, layerCatalogs } from "./library/catalog/client.ts";
import { LibraryServiceTag, librariesHolding, libraryWiring } from "./library/layer.ts";
import { layerPlugins } from "./plugins/registries.ts";
import { layerProjectRegistry, ProjectRegistry, type RegistryOptions } from "./projects.ts";
import { registerMediaKinds } from "./jobs/kinds/media.look.ts";
import { MAX_FRAME } from "./protocol.ts";
import { claimSocketPath, makeLocalServer, serveRpc } from "./transport.ts";
import { makeTicketedWebSocketServer, makeUpgradeCheck } from "./upgrade.ts";
import { ServerControlTag } from "./server-admin.ts";
import { isServeCommand, runConsole } from "./console/main.ts";
import { currentSandboxStatus, reporterFor, sandboxWarnings } from "./sandbox/status.ts";
import { type AgentRuntimeParts, AgentRuntimeTag, type AgentServeOptions, buildAgentRuntime } from "./agent/wiring.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  readonly fonts?: FontsOptions;
  readonly diagnostics?: DiagnosticsOptions;
  /** Agent and MCP credentials (tests): the clock expiry reads and the sweep's period (60 s). */
  readonly credentials?: { readonly clock?: () => number; readonly sweepMs?: number };
  /** The agent runtime's shared pieces, handed to tests once built (7A-2). */
  readonly onAgentCore?: (core: AgentCore) => void;
  /** The agent runtime's options (tests: the fake harness's scripts) and its parts once booted. */
  readonly agents?: AgentServeOptions;
  readonly onAgentRuntime?: (parts: AgentRuntimeParts) => void;
  /** The job engine's options (tests: the test kinds). */
  readonly jobs?: JobEngineOptions;
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
}

/**
 * Runs the server until interrupted or `server.stop`; finalizers stop the transports, close
 * engines and remove `finisherd.json`.
 */
export const serve = ({
  serverData,
  onReady,
  onAgentCore,
  agents: agentOptions,
  onAgentRuntime,
  registry,
  fonts,
  diagnostics: diagnosticsOptions,
  credentials: credentialOptions,
  jobs: jobOptions,
}: ServeOptions) =>
  Effect.scoped(
    Effect.gen(function* () {
      yield* Effect.try({ try: () => ensureServerData(serverData), catch: startFailure });
      // Held until shutdown, before touching the socket: claim + listen is then race-free.
      yield* withServerLock(serverData);
      // The control store (5A Task 1) before any listener: the environment id, the audit log.
      const id = yield* Effect.try({ try: () => serverId(serverData), catch: startFailure });
      const control = yield* Layer.build(Layer.provideMerge(layerAudit, layerControlStore(serverData, id)));
      const envId = Context.get(control, ControlStoreTag).envId;
      // New `local.key` and `console.key` (5A Task 2) before the socket is claimed: a key read
      // while this server listens is always this server's. `auth/` and `run/` are private.
      const keys = yield* Effect.try({ try: () => rotateKeys(serverData), catch: startFailure });
      // `auth/` stays out of Time Machine (review of Tasks 8–9, M3): a backup of
      // `credential.master` would outlive every rotation. Best effort: a warning if it fails.
      excludeFromBackups(authDir(serverData));
      const socket = socketPath(serverData);
      yield* claimSocketPath(socket);
      const tickets = yield* makeTickets();
      // The journal opens under the settings already read, before any transport listens.
      const diagnostics = yield* makeDiagnostics(serverData, diagnosticsOptions);
      const runLog = Effect.runForkWith(yield* Effect.context<never>()); // the server's (stderr) logger
      const logSink = (level: "debug" | "warning", message: string) => void runLog(level === "debug" ? Effect.logDebug(message) : Effect.logWarning(message));
      // 5D's catalog threads before the job engine (library jobs' hooks call the indexer until the
      // engine stops them), the library service after it, so it closes first.
      const jobsRoot = join(serverData, "jobs");
      const catalogs = Context.get(
        yield* Layer.build(layerCatalogs({ serverData, jobsRoot, onFatal: (e) => logSink("warning", `library catalogs: a thread died (${e.code})`) })),
        CatalogsTag,
      );
      const libraries = libraryWiring({
        serverData, jobsRoot, catalogs, control: Context.get(control, ControlStoreTag), audit: Context.get(control, AuditTag), journal: diagnostics.journal, log: logSink,
      });
      // The job engine (5C-1) after the control store and the journal, so it closes before them
      // (its `close` stops every worker); `<server-data>/jobs/` is made private. Its events go
      // to the journal (D34).
      // The project registry first: the `media.look` kind reads from it (its engine writes a
      // job's inputs), and it closes after the job engine. An open project holds the libraries
      // its assets live in (5D): the library service starts after the engine, so the hook is
      // bound once it has; a project opened before that holds none.
      let libraryHold: ((paths: ReadonlyArray<string>) => { release(): void }) | undefined;
      const registryContext = yield* Layer.build(
        layerProjectRegistry(serverData, { ...registry, libraryUse: (paths) => libraryHold?.(paths) ?? { release: () => {} } }),
      );
      const projects = Context.get(registryContext, ProjectRegistry);
      const register = (r: Parameters<typeof libraries.register>[0]) => {
        registerMediaKinds(r, { serverData, projects });
        jobOptions?.register?.(r);
        libraries.register(r);
      };
      const jobsContext = yield* Layer.build(
        layerJobEngine(serverData, { diagnostics: () => diagnostics.enabled(), ...jobOptions, register }).pipe(
          Layer.provide(Layer.mergeAll(Layer.succeedContext(control), Layer.succeed(JobEventSinkTag, journalSink(diagnostics.journal, logSink)))),
        ),
      );
      const engine = Context.get(jobsContext, JobEngineTag);
      const caps = engine.workers().caps;
      yield* Effect.logInfo(
        `finisherd job engine: killTree=${caps.killTree} watchdog=${caps.watchdog} cgroupMemory=${caps.cgroupMemory} leafLimits=${caps.leafLimits} sandbox=${caps.sandbox} sandboxed=${caps.sandboxed}`,
      );

      const library = yield* Effect.acquireRelease(
        Effect.tryPromise({ try: () => libraries.start(engine), catch: startFailure }),
        (s) => Effect.sync(() => s.close()),
      );
      libraryHold = (paths) => library.use(librariesHolding(library.registry, paths));

      const audit = Context.get(control, AuditTag);
      // Windows: the native pipe listener (Task 11); a client of another user is refused
      // before its first byte and audited.
      const local = yield* makeLocalServer(socket, {
        onRefused: (r) =>
          audit.append({
            kind: "auth.fail",
            principal: { kind: "anonymous", id: "" },
            credentialKind: "pipe",
            outcome: "failed",
            code: "UNAUTHENTICATED",
            details: { reason: r.reason, transport: "pipe", peer: { pid: r.pid, ...(r.sid == null ? {} : { sid: r.sid }) } },
          }),
      });
      // The WebSocket (Decision 24): a ticket in `Sec-WebSocket-Protocol`, Host and Origin checked first.
      let wsPort = 0;
      const check = makeUpgradeCheck({ port: () => wsPort, tickets, origins: cachedOrigins(serverData) });
      const { server: ws, port } = yield* Effect.mapError(
        makeTicketedWebSocketServer({
          check,
          // A message over the frame cap and its slack is dropped by ws (close 1009); one just over
          // the cap reaches `protocol.ts`, which answers `FRAME_TOO_LARGE` first.
          maxPayload: MAX_FRAME + 64 * 1024,
          onRefused: (r) => {
            if (r.reason === "badPath" || r.reason === "badVersion" || r.reason === "tooMany") return;
            audit.append({
              kind: "auth.fail",
              principal: { kind: "anonymous", id: "" },
              credentialKind: "ticket",
              outcome: "failed",
              code: "UNAUTHENTICATED",
              details: { reason: r.reason, transport: "ws" },
            });
          },
        }),
        (e) => startFailure(e.reason.cause ?? e),
      );
      wsPort = port;
      // The endpoints both MACs bind (Decision 7): the socket's real path or the pipe's name.
      const localEndpoint = canonicalEndpoint(socket);
      const wsEndpoint = `ws://127.0.0.1:${wsPort}`;
      const everyConnection = new ConnectionRegistry();
      const connections = {
        local: everyConnection.add(new Connections(process.platform === "win32" ? "pipe" : "socket", () => localEndpoint)),
        // Each WebSocket carries its ticket's principal from its first frame (`upgrade.ts`).
        ws: everyConnection.add(new Connections("ws", () => wsEndpoint)),
      };
      // Agent and MCP credentials (5A Task 9): proved through the handshake while live; every
      // request re-checks; expired ones are ended by the sweep, revoked ones at once.
      const credentials = new Credentials(Context.get(control, ControlStoreTag), keys.credentialMaster, audit, credentialOptions?.clock, (key) =>
        writeKeyFile(join(authDir(serverData), KEY_FILES.credentialMaster), key),
      );
      everyConnection.useLiveness((p) => credentials.isLive(p));
      // One last write of ended credentials before the control store closes (it closes after).
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          credentials.persist();
          credentials.dispose();
        }),
      );
      const handshake = new Handshake({
        keys,
        envId,
        audit,
        credentials: (ref) => credentials.forHello(ref),
        revoke: (ref) => credentials.markEnded(ref, "self"),
      });
      const revokeDeps = {
        audit,
        tickets,
        connections: everyConnection,
        revoke: (ref: { kind: "agent" | "mcp"; id: string }) => credentials.markEnded(ref, "expired"),
        persist: () => credentials.persist(),
      };
      // A busy store fails one sweep, never the timer: the next one tries again.
      const sweep = () => {
        try {
          sweepExpired(revokeDeps, () => credentials.expired());
        } catch (e) {
          process.stderr.write(`finisherd: warning: credential expiry sweep failed: ${String(e)}\n`);
        }
      };
      sweep();
      yield* Effect.acquireRelease(
        Effect.sync(() => {
          const timer = setInterval(sweep, credentialOptions?.sweepMs ?? 60_000);
          timer.unref();
          return timer;
        }),
        (timer) => Effect.sync(() => clearInterval(timer)),
      );
      // The agent runtime's shared pieces (7A-2): the session store, each session's credential
      // (minted, refreshed and revoked through the one revoke path) and the branch lineage the
      // chains read for branch reach. Credential effects a crash left in the outbox are done now.
      const agentClock = credentialOptions?.clock === undefined ? systemClock : { now: credentialOptions.clock };
      const agentIds = systemIds(agentClock);
      const agentStore = new AgentStore(Context.get(control, ControlStoreTag), serverData, agentClock, agentIds);
      const sessionCredentials = new SessionCredentials({
        store: agentStore,
        credentials,
        revoke: { ...revokeDeps, revoke: (ref) => credentials.markEnded(ref, "owner") },
      });
      sessionCredentials.drain();
      // A revocation lost before it was written (a busy store, then a crash) is done now.
      sessionCredentials.reconcile();
      // Owner decision F2 (plan 7C Decision 23): a profile not enforced on this host runs,
      // warned; `sandbox.unconfined` is recorded once per profile for this start.
      const sandbox = currentSandboxStatus();
      reporterFor(diagnostics.journal, (m) => logSink("warning", m)).report(sandbox);
      for (const w of sandboxWarnings(sandbox)) yield* Effect.logWarning(`finisherd: ${w.code} (${String(w.details.profile)}): ${w.message}`);
      // The outcome tracker (Task 11) emits 5E's `review.verdict` into this journal once the kind exists.
      const lineage = new BranchLineage();
      const outcomes = new OutcomeTracker({ store: agentStore, lineage, clock: agentClock.now, verdicts: journalVerdicts(diagnostics.journal) });
      // Its journal records metadata to the diagnostics journal and content to the trace store (D34).
      const agentJournal = new AgentJournal({
        diagnostics: diagnostics.journal,
        traces: diagnostics.traces,
        store: agentStore,
        ids: agentIds,
        clock: agentClock,
        log: (message, fields) => logSink("warning", `${message} ${JSON.stringify(fields ?? {})}`),
      });
      const agentCore: AgentCore = {
        store: agentStore,
        credentials: sessionCredentials,
        lineage,
        outcomes,
        clock: agentClock,
        ids: agentIds,
        journal: agentJournal,
      };
      onAgentCore?.(agentCore);
      // The agent runtime over the project registry (7A Task 12): recovery, the outbox
      // and the credential reconcile run here, before either transport serves a request.
      const projectRegistry = projects; // built above, before the job engine
      const agentRuntime = yield* buildAgentRuntime({
        serverData,
        core: agentCore,
        registry: projectRegistry,
        db: Context.get(control, ControlStoreTag).db,
        audit,
        endpoint: socket,
        log: (message) => logSink("warning", `finisherd: agent runtime: ${message}`),
        ...(agentOptions === undefined ? {} : { options: agentOptions }),
      });
      onAgentRuntime?.(agentRuntime);
      // One font layer for the handlers and the plugins, which feed it the packs' catalogs.
      const fontsLayer = layerFonts(serverData, fonts);
      // `server.stop` (5A Task 8): `serve` returns, and its scope's finalizers stop everything.
      const stopped = Deferred.makeUnsafe<void>();
      const listeners = [
        { transport: connections.local.transport, endpoint: localEndpoint },
        { transport: connections.ws.transport, endpoint: wsEndpoint },
      ] as const;
      const serverControl = { envId, listeners: () => listeners, stop: () => void Deferred.doneUnsafe(stopped, Exit.void) };
      yield* Layer.build(
        Layer.mergeAll(serveRpc(local, connections.local), serveRpc(ws, connections.ws)).pipe(
          Layer.provide(
            Layer.mergeAll(
              Layer.succeedContext(registryContext),
              fontsLayer,
              layerPlugins(serverData).pipe(Layer.provide(Layer.succeedContext(control)), Layer.provide(fontsLayer)),
              Layer.succeed(Diagnostics, diagnostics),
              Layer.succeedContext(control),
              Layer.succeedContext(jobsContext),
              Layer.succeed(LibrariesTag, library),
              Layer.succeed(LibraryServiceTag, library),
              Layer.succeed(KeysTag, keys),
              Layer.succeed(HandshakeTag, handshake),
              Layer.succeed(CredentialsTag, credentials),
              Layer.succeed(TicketsTag, tickets),
              Layer.succeed(ConnectionRegistryTag, everyConnection),
              Layer.succeed(ServerControlTag, serverControl),
              Layer.succeed(AgentCoreTag, agentCore),
              Layer.succeed(AgentRuntimeTag, agentRuntime),
            ),
          ),
        ),
      );

      const publish = (enabled: boolean) =>
        writeServerInfo(serverData, { pid: process.pid, wsPort, socket, envId, diagnostics: { enabled } });
      // Set when `finisherd.json` is removed: a late gate change must not bring it back.
      let removed = false;
      const info = yield* Effect.acquireRelease(
        Effect.try({ try: () => publish(diagnostics.enabled()), catch: startFailure }),
        () =>
          Effect.sync(() => {
            removed = true;
            removeServerInfo(serverData);
          }),
      );
      // `fin` reads `diagnostics.enabled` to decide whether to send anything (Decision 23).
      diagnostics.onEnabledChange((enabled) => {
        if (removed) return;
        try {
          publish(enabled);
        } catch (e) {
          process.stderr.write(`finisherd: warning: cannot update finisherd.json: ${String(e)}\n`);
        }
      });
      yield* Effect.logInfo(`finisherd ready: socket=${socket} wsPort=${info.wsPort} serverData=${serverData}`);
      onReady?.(info);
      yield* Effect.addFinalizer(() => Effect.logInfo("finisherd shutting down"));
      return yield* Deferred.await(stopped);
    }),
  );

/** Logs to stderr only (stdout is reserved). */
export const StderrLogger = Logger.layer([Logger.withConsoleError(Logger.formatSimple)]);

function main(argv: ReadonlyArray<string>): void {
  // Decision 29: no argument, `serve` or only flags serve; any other first word is a console command.
  if (!isServeCommand(argv)) {
    void runConsole(argv).then((code) => {
      process.exitCode = code;
    });
    return;
  }
  let serverData: string;
  try {
    ({ serverData } = parseArgs(argv[0] === "serve" ? argv.slice(1) : argv));
  } catch (e) {
    process.stderr.write(`finisherd: ${(e as Error).message}\n`);
    process.exitCode = 2;
    return;
  }
  const program = serve({ serverData }).pipe(
    Effect.catchCause((cause) =>
      Effect.sync(() => {
        if (Cause.hasInterruptsOnly(cause)) return;
        const error = startFailure(Cause.squash(cause));
        const details = Object.keys(error.details).length > 0 ? ` ${JSON.stringify(error.details)}` : "";
        process.stderr.write(`finisherd: ${error.code}: ${error.message}${details}\n`);
        if (error.fix !== undefined) process.stderr.write(`finisherd: fix: ${error.fix}\n`);
        process.exitCode = 1;
      }),
    ),
    Effect.provide(StderrLogger),
  );
  runMain(program, {
    disableErrorReporting: true,
    // SIGINT/SIGTERM interrupt the main fiber: a graceful shutdown exits 0.
    teardown: (exit, onExit) =>
      onExit(Exit.isSuccess(exit) || Cause.hasInterruptsOnly(exit.cause) ? Number(process.exitCode ?? 0) : 1),
  });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
