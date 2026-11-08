// Session directories and node blobs (plan 7A Decisions 1, 4; spec §3.3, §8.5).
//
// `<server-data>/agents/` is private (`ensurePrivateDir`); each session has `<sessionId>/` with
// `blobs/`, `tmp/`, `git/` and `provider/` (0700; Task 7 writes `credential.json` there). A node
// payload over 4 KiB is a content-addressed file `blobs/<blake3-hex>`, written temp-then-rename
// **before** the row that names it, so a crash between the two leaves an orphan blob (swept
// here) and never a row naming a missing file. Every call is asynchronous (libuv's pool), so
// no file operation blocks the event loop.
import { randomBytes } from "node:crypto";
import { lstat, mkdir, open, readdir, readFile, rename, rm } from "node:fs/promises";
import { join } from "node:path";
import { blake3Hex } from "@finisher/fin-node";
import { ensurePrivateDir } from "../config.ts";
import { finError } from "../errors.ts";

export const AGENTS_DIR = "agents";
/** The per-session directories besides `blobs/` (spec §8.5). */
export const SESSION_SUBDIRS = ["blobs", "tmp", "git", "provider"] as const;

const SESSION_ID = /^[0-9A-Za-z_-]{1,64}$/;
const BLOB_REF = /^[0-9a-f]{64}$/;
const TMP_SUFFIX = ".tmp";

/** `<server-data>/agents`, made private (`SERVER_DATA_UNSAFE` when it is not). */
export function ensureAgentsRoot(serverData: string): string {
  const root = join(serverData, AGENTS_DIR);
  ensurePrivateDir(root);
  return root;
}

export function isBlobRef(ref: string): boolean {
  return BLOB_REF.test(ref);
}

/** `<root>/<sessionId>`; an id that is not a plain name is a bug, never a path. */
export function sessionDir(root: string, sessionId: string): string {
  if (!SESSION_ID.test(sessionId)) throw finError("INVALID_ID", "a session id is a plain name", { sessionId });
  return join(root, sessionId);
}

function code(e: unknown): unknown {
  return (e as { code?: unknown } | null)?.code;
}

async function mkdirPrivate(path: string): Promise<void> {
  try {
    await mkdir(path, { mode: 0o700 });
  } catch (e) {
    if (code(e) !== "EEXIST") throw e;
  }
  const st = await lstat(path);
  if (st.isSymbolicLink() || !st.isDirectory()) {
    throw finError("SERVER_DATA_UNSAFE", `${path} is not a directory`, { path });
  }
}

/** Creates the session's directory and its subdirectories (0700); existing ones are kept. */
export async function makeSessionDirs(root: string, sessionId: string): Promise<string> {
  const dir = sessionDir(root, sessionId);
  await mkdirPrivate(dir);
  for (const sub of SESSION_SUBDIRS) await mkdirPrivate(join(dir, sub));
  return dir;
}

/**
 * Stores `bytes` as `blobs/<blake3-hex>` (0600, temp then rename) and answers the hex. A blob
 * already there is the same content (content addressing): it is kept.
 */
export async function writeBlob(root: string, sessionId: string, bytes: Uint8Array): Promise<string> {
  const ref = blake3Hex(bytes);
  const dir = join(sessionDir(root, sessionId), "blobs");
  await mkdirPrivate(sessionDir(root, sessionId));
  await mkdirPrivate(dir);
  const path = join(dir, ref);
  const known = await lstat(path).catch(() => undefined);
  if (known?.isFile() === true && known.size === bytes.byteLength) return ref;
  const tmp = join(dir, `${ref}.${randomBytes(6).toString("hex")}${TMP_SUFFIX}`);
  const fh = await open(tmp, "wx", 0o600);
  try {
    await fh.writeFile(bytes);
    await fh.sync();
  } finally {
    await fh.close();
  }
  try {
    await rename(tmp, path);
  } catch (e) {
    await rm(tmp, { force: true });
    throw e;
  }
  return ref;
}

/** A blob's bytes; `NOT_FOUND` when it is not there. */
export async function readBlob(root: string, sessionId: string, ref: string): Promise<Buffer> {
  if (!isBlobRef(ref)) throw finError("INVALID_ID", "a blob reference is a BLAKE3 hex", { ref });
  try {
    return await readFile(join(sessionDir(root, sessionId), "blobs", ref));
  } catch (e) {
    if (code(e) === "ENOENT") throw finError("NOT_FOUND", "the node's blob is gone", { sessionId, ref });
    throw e;
  }
}

/** Removes one blob (no error when it is gone). */
export async function removeBlob(root: string, sessionId: string, ref: string): Promise<void> {
  if (!isBlobRef(ref)) throw finError("INVALID_ID", "a blob reference is a BLAKE3 hex", { ref });
  await rm(join(sessionDir(root, sessionId), "blobs", ref), { force: true });
}

/** Removes the session's directory and everything in it (no error when it is gone). */
export async function removeSessionDir(root: string, sessionId: string): Promise<void> {
  await rm(sessionDir(root, sessionId), { recursive: true, force: true, maxRetries: 5 });
}

/** The names of the session directories under `root` (anything else is ignored). */
export async function listSessionDirs(root: string): Promise<string[]> {
  const entries = await readdir(root, { withFileTypes: true }).catch((e) => {
    if (code(e) === "ENOENT") return [];
    throw e;
  });
  return entries.filter((d) => d.isDirectory() && SESSION_ID.test(d.name)).map((d) => d.name);
}

/** How old an entry is (ms), or undefined when it is gone. */
async function age(path: string, now: number): Promise<number | undefined> {
  const st = await lstat(path).catch(() => undefined);
  return st === undefined ? undefined : now - st.mtimeMs;
}

/**
 * Deletes the session's blobs that no row names (`keep`) and leftover temp files, each only
 * when older than `graceMs` (a blob being written now has no row yet). Answers how many went.
 */
export async function sweepBlobs(root: string, sessionId: string, keep: ReadonlySet<string>, now: number, graceMs: number): Promise<number> {
  const dir = join(sessionDir(root, sessionId), "blobs");
  const names = await readdir(dir).catch((e) => {
    if (code(e) === "ENOENT") return [] as string[];
    throw e;
  });
  let removed = 0;
  for (const name of names) {
    const orphan = name.endsWith(TMP_SUFFIX) || (isBlobRef(name) && !keep.has(name));
    if (!orphan) continue;
    const a = await age(join(dir, name), now);
    if (a === undefined || a < graceMs) continue;
    await rm(join(dir, name), { force: true });
    removed++;
  }
  return removed;
}

/** Whether the session directory was last changed at least `graceMs` ago. */
export async function dirOlderThan(root: string, sessionId: string, now: number, graceMs: number): Promise<boolean> {
  const a = await age(sessionDir(root, sessionId), now);
  return a !== undefined && a >= graceMs;
}
