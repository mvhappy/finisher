// `<server-data>/finisherd.lock`: one finisherd per server-data. A kernel-held exclusive lock
// (fin-node `lockExclusive`: flock / LockFileEx) kept for the whole process lifetime; the OS
// releases it if the process dies, so a leftover file never blocks startup. The file itself
// is never deleted: unlinking a flock'd file someone else is about to lock is racy.
import { join } from "node:path";
import { lockExclusive, type ServerLock } from "@finisher/fin-node";
import { Effect, type Scope } from "effect";
import { type ServerStartError, startFailure } from "./config.ts";

export function lockPath(serverData: string): string {
  return join(serverData, "finisherd.lock");
}

/** Holds the server lock for the enclosing scope; `SERVER_RUNNING` if another server has it. */
export const withServerLock = (serverData: string): Effect.Effect<ServerLock, ServerStartError, Scope.Scope> =>
  Effect.acquireRelease(
    Effect.try({ try: () => lockExclusive(lockPath(serverData)), catch: startFailure }),
    (lock) => Effect.sync(() => lock.release()),
  );
