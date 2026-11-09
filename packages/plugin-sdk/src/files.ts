// A cold kind's job files (8C onward; plan 8B Decision 5, spec §3.5): 5C maps `in/` and `out/`
// files of the job directory into `start.files`. A Node plugin reads inputs only by position
// (`fs.readSync` at an offset: no mapping, no rings) and writes outputs only inside the size
// 5C created them at, never growing or truncating them; the host validates a copy of what it
// reads back. Files are opened without following a final symlink where the OS can refuse one,
// and must be regular files of their declared size.
import { closeSync, constants, fstatSync, openSync, readSync, writeSync } from "node:fs";
import type { FileGrant } from "@finisher/core-types";
import { PluginError } from "./errors.ts";

const NOFOLLOW = constants.O_NOFOLLOW ?? 0;

/** An output's problem is the plugin's (`PLUGIN_OUTPUT_INVALID`); an input's the host's (`PLUGIN_PROTOCOL`). */
type Side = "input" | "output";
const fileError = (side: Side, message: string, details: { [k: string]: unknown }): PluginError =>
  new PluginError(side === "output" ? "PLUGIN_OUTPUT_INVALID" : "PLUGIN_PROTOCOL", message, details);

function openChecked(grant: FileGrant, flags: number, side: Side): { readonly fd: number; readonly size: number } {
  const fd = openSync(grant.path, flags | NOFOLLOW);
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) throw fileError(side, `${grant.name} is not a regular file`, { name: grant.name, reason: "notRegular" });
    if (grant.bytes !== undefined && st.size !== grant.bytes) throw fileError(side, `${grant.name} is not its declared size`, { name: grant.name, reason: "size", declared: grant.bytes, got: st.size });
    return { fd, size: st.size };
  } catch (e) {
    closeSync(fd);
    throw e;
  }
}

/** An input file, read by position only. */
export class InputFile {
  readonly name: string;
  readonly size: number;
  #fd: number | undefined;

  constructor(grant: FileGrant) {
    if (grant.mode === "write") throw fileError("input", `${grant.name} is not readable`, { name: grant.name, reason: "mode" });
    const { fd, size } = openChecked(grant, constants.O_RDONLY, "input");
    this.name = grant.name;
    this.size = size;
    this.#fd = fd;
  }

  /** Up to `length` bytes at `offset` (fewer only at the end of the file). */
  read(offset: number, length: number): Uint8Array {
    const fd = this.#open();
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(length) || offset < 0 || length < 0) throw new RangeError("offset and length are non-negative integers");
    const n = Math.max(0, Math.min(length, this.size - offset));
    const buf = new Uint8Array(n);
    let done = 0;
    while (done < n) {
      const got = readSync(fd, buf, done, n - done, offset + done);
      if (got === 0) break;
      done += got;
    }
    return done === n ? buf : buf.subarray(0, done);
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
    if (grant.mode === "read") throw fileError("output", `${grant.name} is not writable`, { name: grant.name, reason: "mode" });
    if (grant.bytes === undefined) throw fileError("output", `${grant.name} has no declared size`, { name: grant.name, reason: "size" });
    const { fd, size } = openChecked(grant, constants.O_RDWR, "output");
    this.name = grant.name;
    this.size = size;
    this.#fd = fd;
  }

  /** Writes all of `bytes` at `offset`; a write past the declared size is refused, nothing written. */
  write(offset: number, bytes: Uint8Array): void {
    if (this.#fd === undefined) throw new Error(`${this.name} is closed`);
    if (!Number.isSafeInteger(offset) || offset < 0 || offset + bytes.length > this.size) {
      throw fileError("output", `a write to ${this.name} passes its declared size`, { name: this.name, reason: "size", declared: this.size, offset, length: bytes.length });
    }
    let done = 0;
    while (done < bytes.length) done += writeSync(this.#fd, bytes, done, bytes.length - done, offset + done);
  }

  close(): void {
    if (this.#fd !== undefined) closeSync(this.#fd);
    this.#fd = undefined;
  }
}

/** The grant named `name` in a start's `files`, opened for its mode. */
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "input"): InputFile;
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "output"): OutputFile;
export function openGrant(files: ReadonlyArray<FileGrant> | undefined, name: string, as: "input" | "output"): InputFile | OutputFile {
  const g = files?.find((f) => f.name === name);
  if (g === undefined) throw new PluginError("PLUGIN_PROTOCOL", `the job has no file ${JSON.stringify(name)}`, { reason: "noFile", name });
  return as === "input" ? new InputFile(g) : new OutputFile(g);
}
