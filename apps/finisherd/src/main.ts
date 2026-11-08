#!/usr/bin/env node
// finisherd: serves `FinRpcs` over the local socket/pipe and a ticketed WebSocket.
// Usage: node apps/finisherd/src/main.ts [--server-data <dir>]
// Logs go to stderr only; stdout stays empty.
import { pathToFileURL } from "node:url";
import { runMain } from "@effect/platform-node/NodeRuntime";
import { Cause, Context, Effect, Exit, Layer, Logger } from "effect";
import {
  ensureServerData,
  parseArgs,
  readOrigins,
  removeServerInfo,
  type ServerInfo,
  startFailure,
  socketPath,
  writeServerInfo,
} from "./config.ts";
import { serverId } from "@finisher/fin-node";
import { canonicalEndpoint } from "./auth/crypto.ts";
import { Handshake, HandshakeTag } from "./auth/handshake.ts";
import { KeysTag, rotateKeys } from "./auth/keys.ts";
import { Connections } from "./connections.ts";
import { AuditTag, layerAudit } from "./control/audit.ts";
import { makeTickets, TicketsTag } from "./auth/tickets.ts";
import { ControlStoreTag, layerControlStore } from "./control/db.ts";
import { layerFonts, type FontsOptions } from "./fonts.ts";
import { Diagnostics, type DiagnosticsOptions, makeDiagnostics } from "./diagnostics.ts";
import { withServerLock } from "./lock.ts";
import { layerProjectRegistry, type RegistryOptions } from "./projects.ts";
import { MAX_FRAME } from "./protocol.ts";
import { claimSocketPath, makeLocalSocketServer, serveRpc } from "./transport.ts";
import { makeTicketedWebSocketServer, makeUpgradeCheck } from "./upgrade.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  readonly fonts?: FontsOptions;
  readonly diagnostics?: DiagnosticsOptions;
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
}

/** Runs the server until interrupted; finalizers stop the transports, close engines and remove `finisherd.json`. */
export const serve = ({ serverData, onReady, registry, fonts, diagnostics: diagnosticsOptions }: ServeOptions) =>
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
      const socket = socketPath(serverData);
      yield* claimSocketPath(socket);
      const tickets = yield* makeTickets();
      // The journal opens under the settings already read, before any transport listens.
      const diagnostics = yield* makeDiagnostics(serverData, diagnosticsOptions);

      const local = yield* makeLocalSocketServer(socket);
      const audit = Context.get(control, AuditTag);
      // The WebSocket (Decision 24): a ticket in `Sec-WebSocket-Protocol`, Host and Origin checked first.
      let wsPort = 0;
      const check = makeUpgradeCheck({ port: () => wsPort, tickets, origins: () => readOrigins(serverData) });
      const { server: ws, port } = yield* Effect.mapError(
        makeTicketedWebSocketServer({
          check,
          // A message over the frame cap and its slack is dropped by ws (close 1009); one just over
          // the cap reaches `protocol.ts`, which answers `FRAME_TOO_LARGE` first.
          maxPayload: MAX_FRAME + 64 * 1024,
          onRefused: (r) => {
            if (r.reason === "badPath" || r.reason === "badVersion") return;
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
      const connections = {
        local: new Connections(process.platform === "win32" ? "pipe" : "socket", () => localEndpoint),
        // Each WebSocket carries its ticket's principal from its first frame (`upgrade.ts`).
        ws: new Connections("ws", () => wsEndpoint),
      };
      const handshake = new Handshake({ keys, envId, audit });
      yield* Layer.build(
        Layer.mergeAll(serveRpc(local, "fin", connections.local), serveRpc(ws, "ui", connections.ws)).pipe(
          Layer.provide(
            Layer.mergeAll(
              layerProjectRegistry(serverData, registry),
              layerFonts(serverData, fonts),
              Layer.succeed(Diagnostics, diagnostics),
              Layer.succeedContext(control),
              Layer.succeed(KeysTag, keys),
              Layer.succeed(HandshakeTag, handshake),
              Layer.succeed(TicketsTag, tickets),
            ),
          ),
        ),
      );

      const publish = (enabled: boolean) =>
        writeServerInfo(serverData, { pid: process.pid, wsPort, socket, envId, diagnostics: { enabled } });
      // Set when `finisherd.json` is removed: a late gate change must not bring it back.
      let stopped = false;
      const info = yield* Effect.acquireRelease(
        Effect.try({ try: () => publish(diagnostics.enabled()), catch: startFailure }),
        () =>
          Effect.sync(() => {
            stopped = true;
            removeServerInfo(serverData);
          }),
      );
      // `fin` reads `diagnostics.enabled` to decide whether to send anything (Decision 23).
      diagnostics.onEnabledChange((enabled) => {
        if (stopped) return;
        try {
          publish(enabled);
        } catch (e) {
          process.stderr.write(`finisherd: warning: cannot update finisherd.json: ${String(e)}\n`);
        }
      });
      yield* Effect.logInfo(`finisherd ready: socket=${socket} wsPort=${info.wsPort} serverData=${serverData}`);
      onReady?.(info);
      yield* Effect.addFinalizer(() => Effect.logInfo("finisherd shutting down"));
      return yield* Effect.never;
    }),
  );

/** Logs to stderr only (stdout is reserved). */
export const StderrLogger = Logger.layer([Logger.withConsoleError(Logger.formatSimple)]);

function main(argv: ReadonlyArray<string>): void {
  let serverData: string;
  try {
    ({ serverData } = parseArgs(argv));
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
