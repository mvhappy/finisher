// File operations in a job directory that follow no link a worker may have planted (9a review
// B1; 9b review B1, H1). Directories are made one segment at a time and `lstat`ed after; a
// kind's hooks open mapped files through `openMapped` (`O_NOFOLLOW`, then `fstat`: regular
// files with one link only), and before `onSucceeded` the directory is walked (`checkTree`):
// no link, no FIFO, socket or device, no hard-linked file. Every call is asynchronous.
import { constants } from "node:fs";
import { type FileHandle, lstat, mkdir, open, readdir } from "node:fs/promises";
import { join } from "node:path";
import type { FinError, MappedFile } from "@finisher/contracts";
import { finError } from "../errors.ts";

const NOFOLLOW = constants.O_NOFOLLOW ?? 0;
/** A FIFO is opened without blocking (and then refused by `fstat`). */
const NONBLOCK = constants.O_NONBLOCK ?? 0;

export type Planted = "link" | "special" | "hardLink" | "notDir";

const PROBLEMS: Record<Planted, string> = {
  link: "a link",
  special: "a FIFO, socket or device",
  hardLink: "a file with other links",
  notDir: "a file where a directory belongs",
};

/** `JOB_FAILED {path, file, problem}`: never retried (a worker broke the directory's rules). */
export function planted(path: string, name: string, problem: Planted): FinError {
  return finError(
    "JOB_FAILED",
    `the job directory holds ${PROBLEMS[problem]} at ${name}, which a worker planted`,
    { path, file: name, problem },
    "the worker must not replace its files with links or special files",
  );
}

const code = (e: unknown): unknown => (e as { code?: unknown }).code;

/** `lstat`, or `undefined` when nothing is there. */
async function lstatOrNone(path: string) {
  try {
    return await lstat(path);
  } catch (e) {
    if (code(e) === "ENOENT") return undefined;
    throw e;
  }
}

const segmentsOf = (name: string): string[] => (name.endsWith("/") ? name.slice(0, -1) : name).split("/").filter((s) => s !== "");

/**
 * Makes `base/<segments…>` one directory at a time (0700), each `lstat`ed after its `mkdir`:
 * an existing link or file at any segment is refused, never followed. Resolves the path.
 */
export async function mkdirSafe(base: string, rel: string, name = rel): Promise<string> {
  let path = base;
  for (const seg of segmentsOf(rel)) {
    path = join(path, seg);
    try {
      await mkdir(path, { mode: 0o700 });
    } catch (e) {
      if (code(e) !== "EEXIST") throw e;
    }
    const st = await lstat(path);
    if (st.isSymbolicLink()) throw planted(path, name, "link");
    if (!st.isDirectory()) throw planted(path, name, "notDir");
  }
  return path;
}

/** Every existing parent of `base/<name>` is a directory, not a link. */
async function checkParents(base: string, name: string): Promise<void> {
  const segs = segmentsOf(name);
  let path = base;
  for (const seg of segs.slice(0, -1)) {
    path = join(path, seg);
    const st = await lstatOrNone(path);
    if (st === undefined) return;
    if (st.isSymbolicLink()) throw planted(path, name, "link");
    if (!st.isDirectory()) throw planted(path, name, "notDir");
  }
}

/** Whether `name` is a mapped file of `files`, or lies under a mapped directory. */
export function isMapped(files: ReadonlyArray<MappedFile>, name: string): boolean {
  const segs = name.split("/");
  if (name === "" || name.endsWith("/") || segs.some((s) => s === "" || s === "." || s === ".." || s.includes("\\"))) return false;
  return files.some((f) => (f.name.endsWith("/") ? name.startsWith(f.name) : f.name === name));
}

export type OpenHow = "r" | "w" | "rw";

/**
 * Opens a mapped file for a kind's hook: `r` reads, `rw` reads and writes an existing file,
 * `w` creates it (0600, its parents made with `mkdirSafe`) or truncates it. The name must be a
 * mapped file or lie under a mapped directory (`INVALID_REQUEST`); a link at any segment, a
 * FIFO or other special file, or a hard-linked file is refused (`JOB_FAILED`) before any byte
 * is read or written.
 */
export async function openMapped(dir: string, files: ReadonlyArray<MappedFile>, name: string, how: OpenHow = "r"): Promise<FileHandle> {
  if (!isMapped(files, name)) throw finError("INVALID_REQUEST", `${name} is not one of the job's mapped files`, { file: name });
  const path = join(dir, ...segmentsOf(name));
  if (how === "w") await mkdirSafe(dir, segmentsOf(name).slice(0, -1).join("/"), name);
  else await checkParents(dir, name);
  // Without `O_NOFOLLOW` (Windows), the name itself is checked first.
  if (NOFOLLOW === 0 && (await lstatOrNone(path))?.isSymbolicLink() === true) throw planted(path, name, "link");
  const flags =
    (how === "r" ? constants.O_RDONLY : how === "rw" ? constants.O_RDWR : constants.O_WRONLY | constants.O_CREAT) | NOFOLLOW | NONBLOCK;
  let fh: FileHandle;
  try {
    fh = await open(path, flags, 0o600);
  } catch (e) {
    if (code(e) === "ELOOP" || code(e) === "EMLINK") throw planted(path, name, "link");
    if (code(e) === "ENXIO") throw planted(path, name, "special");
    throw e;
  }
  try {
    const st = await fh.stat();
    if (!st.isFile()) throw planted(path, name, "special");
    if (st.nlink !== 1) throw planted(path, name, "hardLink");
    if (how === "w") await fh.truncate(0);
    return fh;
  } catch (e) {
    await fh.close();
    throw e;
  }
}

/** The entry at `path` is a directory (walked) or a regular file with one link. */
async function walk(path: string, name: string): Promise<void> {
  const st = await lstatOrNone(path);
  if (st === undefined) return;
  if (st.isSymbolicLink()) throw planted(path, name, "link");
  if (st.isDirectory()) {
    for (const entry of await readdir(path)) await walk(join(path, entry), `${name}${name.endsWith("/") ? "" : "/"}${entry}`);
    return;
  }
  if (!st.isFile()) throw planted(path, name, "special");
  if (st.nlink !== 1) throw planted(path, name, "hardLink");
}

/**
 * Before a kind's `onSucceeded` (9b review B1), once every process of the job is gone: every
 * mapped file and everything under each mapped directory is a directory or a regular file
 * with one link, and no parent on the way is a link. `JOB_FAILED` otherwise.
 */
export async function checkTree(dir: string, files: ReadonlyArray<MappedFile>): Promise<void> {
  for (const f of files) {
    await checkParents(dir, f.name);
    await walk(join(dir, ...segmentsOf(f.name)), f.name);
  }
}
