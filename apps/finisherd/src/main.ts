#!/usr/bin/env node
// finisherd: serves `FinRpcs` over the local socket/pipe and a bearer-token WebSocket.
// Usage: node apps/finisherd/src/main.ts [--server-data <dir>]
// Logs go to stderr only; stdout stays empty.
import { pathToFileURL } from "node:url";
import { runMain } from "@effect/platform-node/NodeRuntime";
import { Cause, Effect, Exit, Layer, Logger } from "effect";
import {
  ensureServerData,
  newToken,
  parseArgs,
  removeServerInfo,
  type ServerInfo,
  type ServerStartError,
  socketPath,
  writeServerInfo,
} from "./config.ts";
import { HandlersLive } from "./handlers.ts";
import { withServerLock } from "./lock.ts";
import { layerProjectRegistry, type RegistryOptions } from "./projects.ts";
import { claimSocketPath, makeLocalSocketServer, makeWebSocketServer, portOf, serveRpc } from "./transport.ts";

export interface ServeOptions {
  readonly serverData: string;
  readonly registry?: RegistryOptions;
  /** Called once both transports listen and `finisherd.json` is written. */
  readonly onReady?: (info: ServerInfo) => void;
}

/** Runs the server until interrupted; finalizers stop the transports, close engines and remove `finisherd.json`. */
export const serve = ({ serverData, onReady, registry }: ServeOptions) =>
  Effect.scoped(
    Effect.gen(function* () {
      yield* Effect.try({ try: () => ensureServerData(serverData), catch: (e) => e as ServerStartError });
      // Held until shutdown, before touching the socket: claim + listen is then race-free.
      yield* withServerLock(serverData);
      const socket = socketPath(serverData);
      yield* claimSocketPath(socket);
      const token = newToken();

      const local = yield* makeLocalSocketServer(socket);
      const ws = yield* makeWebSocketServer(token);
      yield* Layer.build(
        Layer.mergeAll(serveRpc(local), serveRpc(ws)).pipe(
          Layer.provide(HandlersLive),
          Layer.provide(layerProjectRegistry(serverData, registry)),
        ),
      );

      const info = yield* Effect.acquireRelease(
        Effect.sync(() => writeServerInfo(serverData, { pid: process.pid, wsPort: portOf(ws), socket, token })),
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
        const error = Cause.squash(cause) as { code?: string; message?: string };
        process.stderr.write(`finisherd: ${error.code ? `${error.code}: ` : ""}${error.message ?? String(error)}\n`);
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
