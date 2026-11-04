// `<server-data>/finisherd.lock`: one finisherd per server-data, held for the process lifetime.
import { closeSync, linkSync, openSync, readFileSync, renameSync, rmSync, writeSync } from "node:fs";
import { join } from "node:path";
import { Effect } from "effect";
import { ServerStartError } from "./config.ts";

export function lockPath(serverData: string): string {
  return join(serverData, "finisherd.lock");
}

/** `kill(pid, 0)`: alive unless the process does not exist (`EPERM` means alive, not ours). */
export function isPidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as NodeJS.ErrnoException).code === "EPERM";
  }
}

function readPid(file: string): number | undefined {
  try {
    const pid = Number.parseInt(readFileSync(file, "utf8").trim(), 10);
    return Number.isSafeInteger(pid) && pid > 0 ? pid : undefined;
  } catch {
    return undefined;
  }
}

function running(file: string, pid: number | undefined): ServerStartError {
  return new ServerStartError(
    "SERVER_RUNNING",
    `another finisherd (pid ${pid ?? "unknown"}) holds ${file}; one server per server-data`,
  );
}

/**
 * Creates the lock with `open(..., "wx")` and writes this pid. If it exists and its pid is
 * dead, the lock is stolen: renamed aside (only one contender can win that rename), checked
 * to still be the dead owner's, removed, and created again. A live or unreadable owner
 * (a lock being written right now) means `SERVER_RUNNING`.
 */
export function acquireLock(serverData: string, pid: number = process.pid): string {
  const file = lockPath(serverData);
  for (let attempt = 0; attempt < 5; attempt++) {
    try {
      const fd = openSync(file, "wx", 0o600);
      try {
        writeSync(fd, `${pid}\n`);
      } finally {
        closeSync(fd);
      }
      return file;
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "EEXIST") throw e;
    }
    const owner = readPid(file);
    if (owner === undefined || (owner !== pid && isPidAlive(owner))) throw running(file, owner);
    const aside = `${file}.${pid}.stale`;
    try {
      renameSync(file, aside);
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") continue; // another contender moved it
      throw e;
    }
    const moved = readPid(aside);
    if (moved !== owner) {
      // We moved a fresh lock that replaced the stale one in the meantime: put it back.
      try {
        linkSync(aside, file);
      } catch {
        // someone else holds a newer lock already
      }
      rmSync(aside, { force: true });
      throw running(file, moved);
    }
    rmSync(aside, { force: true });
  }
  throw running(file, readPid(file));
}

/** Removes the lock if it is still ours. */
export function releaseLock(serverData: string, pid: number = process.pid): void {
  const file = lockPath(serverData);
  if (readPid(file) === pid) rmSync(file, { force: true });
}

/** Holds the lock for the enclosing scope. */
export const withServerLock = (serverData: string) =>
  Effect.acquireRelease(
    Effect.try({ try: () => acquireLock(serverData), catch: (e) => e as ServerStartError }),
    () => Effect.sync(() => releaseLock(serverData)),
  );
