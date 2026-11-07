#!/usr/bin/env node
// finisherd: serves `FinRpcs` over the local socket/pipe and a bearer-token WebSocket.
// Usage: node apps/finisherd/src/main.ts [--server-data <dir>]
// Logs go to stderr only; stdout stays empty.
import { pathToFileURL } from "node:url";
import { runMain } from "@effect/platform-node/NodeRuntime";
import { Cause, Context, Effect, Exit, Layer, Logger } from "effect";
import {
  ensureServerData,
  newToken,
  parseArgs,
  removeServerInfo,
  type ServerInfo,
  startFailure,
  socketPath,
  writeServerInfo,
} from "./config.ts";
import { serverId } from "@finisher/fin-node";
import { KeysTag, rotateKeys } from "./auth/keys.ts";
import { layerAudit } from "./control/audit.ts";
import { ControlStoreTag, layerControlStore } from "./control/db.ts";
import { layerFonts, type FontsOptions } from "./fonts.ts";
import { HandlersLive } from "./handlers.ts";
import { withServerLock } from "./lock.ts";
import { layerProjectRegistry, type RegistryOptions } from "./projects.ts";
import { claimSocketPath, makeLocalSocketServer, makeWebSocketServer, portOf, serveRpc } from "./transport.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  readonly fonts?: FontsOptions;
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
}

/** Runs the server until interrupted; finalizers stop the transports, close engines and remove `finisherd.json`. */
export const serve = ({ serverData, onReady, registry, fonts }: ServeOptions) =>
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
      const token = newToken();

      const local = yield* makeLocalSocketServer(socket);
      const ws = yield* Effect.mapError(makeWebSocketServer(token), (e) => startFailure(e.reason.cause ?? e));
      yield* Layer.build(
        Layer.mergeAll(serveRpc(local), serveRpc(ws)).pipe(
          Layer.provide(HandlersLive),
          Layer.provide(Layer.mergeAll(layerProjectRegistry(serverData, registry), layerFonts(serverData, fonts))),
          Layer.provide(Layer.succeedContext(control)),
          Layer.provide(Layer.succeed(KeysTag, keys)),
        ),
      );

      const info = yield* Effect.acquireRelease(
        Effect.try({
          try: () => writeServerInfo(serverData, { pid: process.pid, wsPort: portOf(ws), socket, token, envId }),
          catch: startFailure,
        }),
        () => Effect.sync(() => removeServerInfo(serverData)),
      );
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
