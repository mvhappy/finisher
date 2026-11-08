// Job directories and mapped files (spec §6.4, 5C-1 plan Decision 38). `<server-data>/jobs/`
// is private (`ensurePrivateDir`); each job gets `<jobId>/` (0700; on Windows the parent's
// DACL, inherited) with `tmp/`. 5C creates a kind's files and directories, empties `write`
// directories and recreates `write` files before each attempt, checks the files `prepare`
// wrote, and never reads any of them. Every call is asynchronous (libuv's pool), so no file
// operation blocks the event loop. A directory a worker has already used is checked before it
// is prepared again (`checkNoLinks`): a link the worker planted is never followed (9a review B1).
import { constants } from "node:fs";
import { lstat, mkdir, open, readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import type { FinError, MappedFile } from "@finisher/contracts";
import { ensurePrivateDir } from "../config.ts";
import { finError } from "../errors.ts";

export const JOBS_DIR = "jobs";
const REMOVE_TRIES = 20;
/** Written once `prepare` succeeded (Decision 40; not a mapped name: `~` is not allowed there). */
export const PREPARED_MARKER = ".prepared~";
const REMOVE_WAIT_MS = 25;

/** `<server-data>/jobs`, made private (`SERVER_DATA_UNSAFE` when it is not). */
export function ensureJobsRoot(serverData: string): string {
  const root = join(serverData, JOBS_DIR);
  ensurePrivateDir(root);
  return root;
}

export interface JobDirs {
  readonly job: string;
  readonly tmp: string;
}

export function jobDirs(root: string, id: string): JobDirs {
  const job = join(root, id);
  return { job, tmp: join(job, "tmp") };
}

/** A mapped file's absolute path; a directory's keeps no trailing separator. */
export function mappedPath(dir: string, name: string): string {
  return join(dir, ...(name.endsWith("/") ? name.slice(0, -1) : name).split("/"));
}

const isDirName = (name: string): boolean => name.endsWith("/");
const writable = (f: MappedFile): boolean => f.mode !== "read";

function ioError(e: unknown, path: string, step: string): FinError {
  const code = (e as { code?: unknown }).code;
  return finError("IO_ERROR", `the job directory could not be prepared (${step}): ${(e as Error).message ?? String(e)}`, {
    path,
    step,
    ...(typeof code === "string" ? { cause: code } : {}),
  });
}

async function exists(path: string): Promise<boolean> {
  try {
    await lstat(path);
    return true;
  } catch (e) {
    if ((e as { code?: unknown }).code === "ENOENT") return false;
    throw e;
  }
}

/**
 * Makes the job directory and its `tmp/` (0700). Resolves `true` when the directory is new
 * (nothing in it can have been planted by a worker yet).
 */
export async function createJobDir(dirs: JobDirs): Promise<boolean> {
  try {
    const created = !(await exists(dirs.job));
    await mkdir(dirs.job, { recursive: true, mode: 0o700 });
    await mkdir(dirs.tmp, { recursive: true, mode: 0o700 });
    return created;
  } catch (e) {
    throw ioError(e, dirs.job, "jobDir");
  }
}

/** Whether the kind's `prepare` already succeeded for this directory (9a review M3). */
export async function isPrepared(dirs: JobDirs): Promise<boolean> {
  try {
    return (await lstat(join(dirs.job, PREPARED_MARKER))).isFile();
  } catch {
    return false;
  }
}

/** Records that `prepare` succeeded: it does not run again for this directory. */
export async function markPrepared(dirs: JobDirs): Promise<void> {
  const fh = await open(join(dirs.job, PREPARED_MARKER), constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC | (constants.O_NOFOLLOW ?? 0), 0o600);
  await fh.close();
}

/**
 * A directory a worker has used may hold links it planted (a symlinked `out/`, a junction for
 * `in/`): every existing segment from the job directory down to each mapped path, `tmp/` and
 * each helper directory, is `lstat`ed, and a symlink or junction (Node reports both as
 * symbolic links) fails the job, never retried (9a review B1). Other reparse points are not
 * visible to `lstat`; sub-project 7's sandbox keeps the worker from making them.
 */
export async function checkNoLinks(dir: string, names: ReadonlyArray<string>): Promise<void> {
  const seen = new Set<string>();
  const check = async (path: string, name: string): Promise<boolean> => {
    if (seen.has(path)) return true;
    seen.add(path);
    let st;
    try {
      st = await lstat(path);
    } catch (e) {
      if ((e as { code?: unknown }).code === "ENOENT") return false;
      throw ioError(e, path, name);
    }
    if (st.isSymbolicLink()) {
      throw finError("JOB_FAILED", `the job directory holds a link at ${name}, which a worker planted`, { path, file: name, problem: "link" }, "the worker must not replace its files with links");
    }
    return true;
  };
  if (!(await check(dir, "."))) return;
  for (const name of names) {
    const segments = (name.endsWith("/") ? name.slice(0, -1) : name).split("/");
    let path = dir;
    for (const seg of segments) {
      path = join(path, seg);
      if (!(await check(path, name))) break;
    }
  }
}

/** Empties a writable directory by replacing it: `rm` removes a link itself, never its target. */
async function emptyDir(path: string): Promise<void> {
  await rm(path, { recursive: true, force: true });
  await mkdir(path, { recursive: true, mode: 0o700 });
}

/**
 * Creates a file (0600) of `bytes`, zero-filled. An old one is unlinked first and the new one
 * made with `O_EXCL | O_NOFOLLOW`, so neither a link at the name nor a hard link to a file
 * elsewhere is ever opened or truncated (9a review B1).
 */
async function sizedFile(path: string, bytes: number): Promise<void> {
  await rm(path, { force: true });
  const flags = constants.O_RDWR | constants.O_CREAT | constants.O_EXCL | (constants.O_NOFOLLOW ?? 0);
  const fh = await open(path, flags, 0o600);
  try {
    await fh.truncate(bytes);
  } finally {
    await fh.close();
  }
}

/**
 * Before each attempt: every directory exists (0700) and a `write`/`readWrite` one is empty;
 * every file with `bytes` exists at that size, recreated when it is writable or new. Files
 * without `bytes` are `prepare`'s. A failure is `IO_ERROR`.
 */
export async function createFiles(dir: string, files: ReadonlyArray<MappedFile>): Promise<void> {
  for (const f of files) {
    const path = mappedPath(dir, f.name);
    try {
      if (isDirName(f.name)) {
        if (writable(f)) await emptyDir(path);
        else await mkdir(path, { recursive: true, mode: 0o700 });
        continue;
      }
      const parent = mappedPath(dir, f.name.split("/").slice(0, -1).join("/") || ".");
      if (parent !== dir) await mkdir(parent, { recursive: true, mode: 0o700 });
      if (f.bytes !== undefined && (writable(f) || !(await exists(path)))) await sizedFile(path, f.bytes);
    } catch (e) {
      throw ioError(e, path, f.name);
    }
  }
}

/**
 * The files `prepare` writes (no `bytes`, not a directory) exist as regular files with one
 * link: no symlink, no reparse point, no hard link to a file elsewhere. Else `IO_ERROR`.
 */
export async function checkPrepared(dir: string, files: ReadonlyArray<MappedFile>): Promise<void> {
  for (const f of files) {
    if (isDirName(f.name) || f.bytes !== undefined) continue;
    const path = mappedPath(dir, f.name);
    let problem: string | undefined;
    try {
      const st = await lstat(path);
      if (st.isSymbolicLink()) problem = "a symlink";
      else if (!st.isFile()) problem = "not a regular file";
      else if (st.nlink !== 1) problem = "a file with other links";
    } catch (e) {
      problem = (e as { code?: unknown }).code === "ENOENT" ? "missing" : String((e as Error).message ?? e);
    }
    if (problem !== undefined) {
      throw finError("IO_ERROR", `the prepared file ${f.name} is ${problem}`, { path, file: f.name, problem }, "the kind's prepare hook must write it");
    }
  }
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Deletes a job directory; on Windows a sharing violation is retried (20 × 25 ms). */
export async function removeJobDir(dir: string, platform: NodeJS.Platform = process.platform): Promise<void> {
  for (let i = 0; ; i++) {
    try {
      await rm(dir, { recursive: true, force: true });
      return;
    } catch (e) {
      const code = (e as { code?: unknown }).code;
      if (platform !== "win32" || i >= REMOVE_TRIES - 1 || (code !== "EBUSY" && code !== "EPERM" && code !== "ENOTEMPTY")) throw e;
      await sleep(REMOVE_WAIT_MS);
    }
  }
}

/** Deletes every entry under the jobs root except `keep` (job ids). Resolves with the names removed. */
export async function sweep(root: string, keep: ReadonlySet<string>): Promise<string[]> {
  const removed: string[] = [];
  let entries: string[];
  try {
    entries = await readdir(root);
  } catch (e) {
    if ((e as { code?: unknown }).code === "ENOENT") return removed;
    throw e;
  }
  for (const name of entries) {
    if (keep.has(name)) continue;
    await removeJobDir(join(root, name));
    removed.push(name);
  }
  return removed;
}
