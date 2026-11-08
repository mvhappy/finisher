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
import { layerFonts, type FontsOptions } from "./fonts.ts";
import { Diagnostics, type DiagnosticsOptions, makeDiagnostics } from "./diagnostics.ts";
import { withServerLock } from "./lock.ts";
import { layerPlugins } from "./plugins/registries.ts";
import { layerProjectRegistry, type RegistryOptions } from "./projects.ts";
import { MAX_FRAME } from "./protocol.ts";
import { claimSocketPath, makeLocalServer, serveRpc } from "./transport.ts";
import { makeTicketedWebSocketServer, makeUpgradeCheck } from "./upgrade.ts";
import { ServerControlTag } from "./server-admin.ts";
import { isServeCommand, runConsole } from "./console/main.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  readonly fonts?: FontsOptions;
  readonly diagnostics?: DiagnosticsOptions;
  /** Agent and MCP credentials (tests): the clock expiry reads and the sweep's period (60 s). */
  readonly credentials?: { readonly clock?: () => number; readonly sweepMs?: number };
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
}

/**
 * Runs the server until interrupted or `server.stop`; finalizers stop the transports, close
 * engines and remove `finisherd.json`.
 */
export const serve = ({ serverData, onReady, registry, fonts, diagnostics: diagnosticsOptions, credentials: credentialOptions }: ServeOptions) =>
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
              layerProjectRegistry(serverData, registry),
              fontsLayer,
              layerPlugins(serverData).pipe(Layer.provide(Layer.succeedContext(control)), Layer.provide(fontsLayer)),
              Layer.succeed(Diagnostics, diagnostics),
              Layer.succeedContext(control),
              Layer.succeed(KeysTag, keys),
              Layer.succeed(HandshakeTag, handshake),
              Layer.succeed(CredentialsTag, credentials),
              Layer.succeed(TicketsTag, tickets),
              Layer.succeed(ConnectionRegistryTag, everyConnection),
              Layer.succeed(ServerControlTag, serverControl),
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
