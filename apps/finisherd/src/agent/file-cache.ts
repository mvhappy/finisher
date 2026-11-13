// Configuration files read at every decision (7B's rule files, 7F's hook files): each is
// opened and `fstat`ed, and what was parsed from it is kept by `(path, ino, size, mtime,
// ctime)`; any change re-reads it, hashing and parsing the one buffer read (what is hashed is
// what applies). A file changed within `RACY_MS` of being read is not kept (a second change in
// the same timestamp tick could keep its key). A FIFO is never opened blocking; anything but a
// regular file, or one larger than `CONFIG_FILE_MAX`, is an error.
import { createHash } from "node:crypto";
import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";

/** The largest configuration file read; a larger one is unreadable. */
export const CONFIG_FILE_MAX = 256 * 1024;
/** A file changed this recently (ms) is read again at every decision (see the module docs). */
export const RACY_MS = 2000;
/** Files whose parsed value is kept at most (past it the cache starts again). */
const CACHE_MAX = 1024;

/** A file's bytes as trust records them (`sha256:<hex>`). */
export const hashOf = (bytes: Buffer) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

/** A file's state: absent, unreadable, or read (its hash, and its value or why it does not parse). */
export type Read<T> = { readonly absent: true } | { readonly error: string } | { readonly hash: string; readonly value?: T; readonly invalid?: string };

const cache = new Map<string, { readonly key: string; readonly read: Read<unknown> }>();
/** Not following a FIFO into a blocking open (Windows has no such flag: 0). */
const NONBLOCK = constants.O_NONBLOCK ?? 0;

/** The first line of an error's message, whitespace collapsed. */
export const firstLine = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/\s+/g, " ").trim();

/**
 * `path` read and parsed with `parse` (its JSON in, the value out; a throw is `invalid`), kept
 * per `slot`: a slot is one reader's view of one file, so two readers of the same file (the
 * rules and the hooks of `settings.json`) never share a parsed value.
 */
export function readCached<T>(path: string, slot: string, parse: (json: unknown) => T): Read<T> {
  return readCachedBytes(path, slot, (bytes) => parse(JSON.parse(bytes.toString("utf8")) as unknown));
}

/** As `readCached`, for any file (`parse` takes its bytes), at most `max` bytes. */
export function readCachedBytes<T>(path: string, slot: string, parse: (bytes: Buffer) => T, max = CONFIG_FILE_MAX): Read<T> {
  let fd: number;
  try {
    fd = openSync(path, constants.O_RDONLY | NONBLOCK);
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    return code === "ENOENT" || code === "ENOTDIR" ? { absent: true } : { error: String(code ?? e) };
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) return { error: "not a regular file" };
    if (st.size > max) return { error: `larger than ${max} bytes` };
    const key = `${path}\0${st.ino}\0${st.size}\0${st.mtimeMs}\0${st.ctimeMs}`;
    const hit = cache.get(slot);
    if (hit?.key === key) return hit.read as Read<T>;
    const readAt = Date.now();
    const bytes = readFileSync(fd);
    if (bytes.length > max) return { error: `larger than ${max} bytes` };
    const hash = hashOf(bytes);
    let read: Read<T>;
    try {
      read = { hash, value: parse(bytes) };
    } catch (e) {
      read = { hash, invalid: firstLine(e) };
    }
    if (Math.max(st.mtimeMs, st.ctimeMs) < readAt - RACY_MS) {
      if (cache.size >= CACHE_MAX) cache.clear();
      cache.set(slot, { key, read });
    } else {
      cache.delete(slot);
    }
    return read;
  } catch (e) {
    return { error: String((e as { code?: unknown }).code ?? e) };
  } finally {
    closeSync(fd);
  }
}
