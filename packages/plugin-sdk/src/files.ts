// A cold kind's job files (8C onward; plan 8B Decision 5, spec §3.5): 5C maps `in/` and `out/`
// files of the job directory into `start.files`. A Node plugin reads inputs only by position
// (`fs.readSync` at an offset: no mapping, no rings) and writes outputs only inside the size
// 5C created them at, never growing or truncating them; the host validates a copy of what it
// reads back. Files are opened without following a final symlink where the OS can refuse one,
// and must be regular files of their declared size. An output opens write-only, so a Node
// grant of `--allow-fs-write` alone suffices.
//
// Errors (the cross-SDK contract): a grant of the wrong mode is `PLUGIN_PERMISSION_DENIED
// {perm: "fs", access}`; an output of the wrong size, truncated, a link or not a regular file
// is `PLUGIN_OUTPUT_INVALID {reason}` (the plugin's fault), the same of an input
// `PLUGIN_PROTOCOL {reason}` (the host's); an I/O failure is `PLUGIN_COMMAND_FAILED {reason:
// io, nodeCode}`. Details name the grant (`file`), never its path.
import { closeSync, constants, fstatSync, lstatSync, openSync, readSync, writeSync } from "node:fs";
import type { FileGrant } from "@finisher/core-types";
import { codes, PluginError, toPluginError } from "./errors.ts";

/** `O_NOFOLLOW` where the OS has it; Windows has none (`refuseLink` stands in there). */
const NOFOLLOW: number | undefined = constants.O_NOFOLLOW;

/** An output's problem is the plugin's (`PLUGIN_OUTPUT_INVALID`); an input's the host's (`PLUGIN_PROTOCOL`). */
type Side = "input" | "output";
const fileError = (side: Side, file: string, reason: string, message: string, more: { [k: string]: unknown } = {}): PluginError =>
  new PluginError(side === "output" ? codes.PLUGIN_OUTPUT_INVALID : codes.PLUGIN_PROTOCOL, message, { file, reason, ...more });

const modeError = (file: string, access: "read" | "write"): PluginError =>
  new PluginError(codes.PLUGIN_PERMISSION_DENIED, `${file} is not granted for ${access === "read" ? "reading" : "writing"}`, { perm: "fs", access, file });

/** A failure of `fs` on a grant: a link (`ELOOP`), Node's permission model, or I/O. */
function fsError(e: unknown, side: Side, file: string): PluginError {
  if (e instanceof PluginError) return e;
  const code = typeof e === "object" && e !== null ? (e as { code?: unknown }).code : undefined;
  // `O_NOFOLLOW` on a final symlink: ELOOP (Linux, macOS), EMLINK (FreeBSD).
  if (code === "ELOOP" || code === "EMLINK") return fileError(side, file, "link", `${file} is a link`);
  if (code === "ERR_ACCESS_DENIED") return toPluginError(e);
  const details: { [k: string]: unknown } = { file, reason: "io" };
  if (typeof code === "string") details["nodeCode"] = code;
  return new PluginError(codes.PLUGIN_COMMAND_FAILED, `${file} could not be ${side === "input" ? "read" : "written"}${typeof code === "string" ? ` (${code})` : ""}`, details);
}

/**
 * Refuses `path` when it is a link (Windows reports a junction as one too). Only where the OS
 * has no `O_NOFOLLOW`: it is a check before the open, so a link swapped in between is not
 * seen; the host's validation of its own copy of an output is the real guard.
 */
export function refuseLink(path: string, side: Side, file: string): void {
  let isLink: boolean;
  try {
    isLink = lstatSync(path).isSymbolicLink();
  } catch (e) {
    throw fsError(e, side, file);
  }
  if (isLink) throw fileError(side, file, "link", `${file} is a link`);
}

function openChecked(grant: FileGrant, flags: number, side: Side): { readonly fd: number; readonly size: number } {
  if (NOFOLLOW === undefined) refuseLink(grant.path, side, grant.name);
  let fd: number;
  try {
    fd = openSync(grant.path, flags | (NOFOLLOW ?? 0));
  } catch (e) {
    throw fsError(e, side, grant.name);
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) throw fileError(side, grant.name, "notRegular", `${grant.name} is not a regular file`);
    if (grant.bytes !== undefined && st.size !== grant.bytes) throw fileError(side, grant.name, "size", `${grant.name} is not its declared size`, { declared: grant.bytes, got: st.size });
    return { fd, size: st.size };
  } catch (e) {
    closeSync(fd);
    throw fsError(e, side, grant.name);
  }
}

/** An input file, read by position only. */
export class InputFile {
  readonly name: string;
  readonly size: number;
  #fd: number | undefined;

  constructor(grant: FileGrant) {
    if (grant.mode === "write") throw modeError(grant.name, "read");
    const { fd, size } = openChecked(grant, constants.O_RDONLY, "input");
    this.name = grant.name;
    this.size = size;
    this.#fd = fd;
  }

  /** Up to `length` bytes at `offset` (fewer only at the end of the file); a file that shrank is `truncated`. */
  read(offset: number, length: number): Uint8Array {
    const fd = this.#open();
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length < 0) throw new RangeError("offset and length are non-negative integers");
    const n = Math.max(0, Math.min(length, this.size - offset));
    const buf = new Uint8Array(n);
    let done = 0;
    while (done < n) {
      let got: number;
      try {
        got = readSync(fd, buf, done, n - done, offset + done);
      } catch (e) {
        throw fsError(e, "input", this.name);
      }
      if (got === 0) throw fileError("input", this.name, "truncated", `${this.name} is shorter than when it was opened`);
      done += got;
    }
    return buf;
  }

  close(): void {
    if (this.#fd !== undefined) closeSync(this.#fd);
    this.#fd = undefined;
  }

  #open(): number {
    if (this.#fd === undefined) throw new Error(`${this.name} is closed`);
    return this.#fd;
  }
}

/** An output file of a declared size: writes land inside it, never past its end. */
export class OutputFile {
  readonly name: string;
  readonly size: number;
  #fd: number | undefined;

  constructor(grant: FileGrant) {
    if (grant.mode === "read") throw modeError(grant.name, "write");
    if (grant.bytes === undefined) throw fileError("output", grant.name, "size", `${grant.name} has no declared size`);
    const { fd, size } = openChecked(grant, constants.O_WRONLY, "output");
    this.name = grant.name;
    this.size = size;
    this.#fd = fd;
  }

  /** Writes all of `bytes` at `offset`; a write past the declared size is refused, nothing written. */
  write(offset: number, bytes: Uint8Array): void {
    if (this.#fd === undefined) throw new Error(`${this.name} is closed`);
    if (!Number.isSafeInteger(offset) || offset < 0 || offset + bytes.length > this.size) {
      throw fileError("output", this.name, "size", `a write to ${this.name} passes its declared size`, { declared: this.size, offset, length: bytes.length });
    }
    let done = 0;
    try {
      while (done < bytes.length) done += writeSync(this.#fd, bytes, done, bytes.length - done, offset + done);
    } catch (e) {
      throw fsError(e, "output", this.name);
    }
  }

  /** Closes the file; one that is no longer its declared size is `truncated`. */
  close(): void {
    const fd = this.#fd;
    if (fd === undefined) return;
    this.#fd = undefined;
    let size: number;
    try {
      size = fstatSync(fd).size;
    } catch (e) {
      throw fsError(e, "output", this.name);
    } finally {
      closeSync(fd);
    }
    if (size !== this.size) throw fileError("output", this.name, "truncated", `${this.name} is no longer its declared size`, { declared: this.size, got: size });
  }
}

/** The grant named `name` in a start's `files`, opened for its mode. */
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "input"): InputFile;
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "output"): OutputFile;
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "input" | "output"): InputFile | OutputFile {
  const g = files?.find((f) => f.name === name);
  if (g === undefined) throw new PluginError(codes.PLUGIN_PROTOCOL, `the job has no file ${JSON.stringify(name)}`, { reason: "noFile", file: name });
  return as === "input" ? new InputFile(g) : new OutputFile(g);
}
