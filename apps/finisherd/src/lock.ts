// `<server-data>/finisherd.lock`: one finisherd per server-data, held for the process lifetime.
import { closeSync, linkSync, openSync, readFileSync, renameSync, rmSync, statSync, writeFileSync, writeSync } from "node:fs";
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

/** An unreadable or empty lock older than this is treated as stale. */
export const UNPARSABLE_LOCK_STALE_MS = 10_000;

interface LockState {
  readonly raw: string;
  readonly mtimeMs: number;
  readonly pid: number | undefined;
}

function parsePid(raw: string): number | undefined {
  const text = raw.trim();
  if (!/^[0-9]+$/.test(text)) return undefined;
  const pid = Number(text);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : undefined;
}

function readLock(file: string): LockState | undefined {
  try {
    const raw = readFileSync(file, "utf8");
    return { raw, mtimeMs: statSync(file).mtimeMs, pid: parsePid(raw) };
  } catch {
    return undefined;
  }
}

function readPid(file: string): number | undefined {
  return readLock(file)?.pid;
}

function running(file: string, pid: number | undefined): ServerStartError {
  return new ServerStartError(
    "SERVER_RUNNING",
    `another finisherd (pid ${pid ?? "unknown"}) holds ${file}; one server per server-data`,
  );
}

/** Errors meaning "this filesystem cannot hard-link", where we fall back to `wx`. */
const NO_LINK = new Set(["EPERM", "ENOTSUP", "EOPNOTSUPP", "ENOSYS", "EXDEV"]);

/**
 * Publishes a lock that always contains a pid: `<lock>.<pid>.tmp` is written first, then
 * hard-linked to the lock name (`link` fails with `EEXIST` if a lock exists — exclusive and
 * atomic). Where hard links are unsupported (some Windows filesystems) it falls back to
 * `open(..., "wx")` + write; the mtime rule in `isStale` covers a crash in that window.
 * Returns false if a lock already exists.
 */
function publish(file: string, pid: number): boolean {
  const tmp = `${file}.${pid}.tmp`;
  writeFileSync(tmp, `${pid}\n`, { mode: 0o600 });
  try {
    linkSync(tmp, file);
    return true;
  } catch (e) {
    const code = (e as NodeJS.ErrnoException).code ?? "";
    if (code === "EEXIST") return false;
    if (!NO_LINK.has(code)) throw e;
    try {
      const fd = openSync(file, "wx", 0o600);
      try {
        writeSync(fd, `${pid}\n`);
      } finally {
        closeSync(fd);
      }
      return true;
    } catch (e2) {
      if ((e2 as NodeJS.ErrnoException).code === "EEXIST") return false;
      throw e2;
    }
  } finally {
    rmSync(tmp, { force: true });
  }
}

/** A lock is stale if its pid is dead (or ours), or it has no parsable pid and is >10 s old. */
function isStale(state: LockState, pid: number, now: number): boolean {
  if (state.pid === undefined) return now - state.mtimeMs > UNPARSABLE_LOCK_STALE_MS;
  return state.pid === pid || !isPidAlive(state.pid);
}

/**
 * Takes `<server-data>/finisherd.lock` for this process (see `publish`). An existing stale
 * lock is stolen: renamed aside (only one contender wins that rename), checked to still be
 * the same stale file, removed, and published again. A live owner means `SERVER_RUNNING`.
 */
export function acquireLock(serverData: string, pid: number = process.pid, now: () => number = Date.now): string {
  const file = lockPath(serverData);
  for (let attempt = 0; attempt < 5; attempt++) {
    if (publish(file, pid)) return file;
    const state = readLock(file);
    if (state === undefined) continue; // released in the meantime
    if (!isStale(state, pid, now())) throw running(file, state.pid);
    const aside = `${file}.${pid}.stale`;
    try {
      renameSync(file, aside);
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") continue; // another contender moved it
      throw e;
    }
    const moved = readLock(aside);
    if (moved === undefined || moved.raw !== state.raw || moved.mtimeMs !== state.mtimeMs) {
      // We moved a fresh lock that replaced the stale one in the meantime: put it back.
      try {
        linkSync(aside, file);
      } catch {
        // someone else holds a newer lock already
      }
      rmSync(aside, { force: true });
      throw running(file, moved?.pid);
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
