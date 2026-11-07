// Scan batch files (plan 5D Decision 26), read and validated in the **indexer thread** only:
// the scan worker writes NDJSON into its job's `out/` and calls `library.batch {file, seq}`;
// the family's `onCall` checks the name and hands the indexer the path, never the bytes. Each
// line of the file is one generated `ScanBatch` (fin-library's `types.rs`), lines in order;
// together they hold ≤ 4,096 entries and the file is ≤ 8 MiB.
//
// The file is opened without following links (`O_NOFOLLOW` where the OS has it, and the
// `lstat`/`fstat` identity everywhere), must be a regular file within the size cap, must be
// valid UTF-8, and every line must match the generated schema exactly (no unknown keys, as
// serde's `deny_unknown_fields`). Anything else is `LIBRARY_BATCH_INVALID` before a row is
// written. `SPECS` lists the checked fields; `batch-file.test.ts` compares them with the
// generated JSON schema, so the validator cannot drift from the Rust types.
import { closeSync, constants, fstatSync, lstatSync, openSync, readSync } from "node:fs";
import { basename, isAbsolute, join, relative } from "node:path";
import type { DirDone, Placeholder, ScanBatch, ScanEntry } from "@finisher/core-types";
import { CatalogFailure } from "./protocol.ts";

export const BATCH_MAX_BYTES = 8 * 1024 * 1024;
export const BATCH_MAX_ENTRIES = 4096;

/** Where a batch file is: the job's `out/` directory and a plain file name inside it. */
export interface BatchFileRef {
  readonly outDir: string;
  readonly file: string;
}

const PLAIN_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

function invalid(reason: string, details: Record<string, unknown> = {}): CatalogFailure {
  return new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason, ...details });
}

/**
 * Checks where the file is: `outDir` absolute, named `out`, inside `jobsRoot` when one is
 * configured, and not itself a link; `file` a plain name. Answers the full path.
 */
export function batchPath(ref: BatchFileRef, jobsRoot?: string): string {
  if (typeof ref.outDir !== "string" || !isAbsolute(ref.outDir) || basename(ref.outDir) !== "out") throw invalid("outDir");
  if (jobsRoot !== undefined) {
    const rel = relative(jobsRoot, ref.outDir);
    if (rel === "" || rel.startsWith("..") || isAbsolute(rel)) throw invalid("outsideJobs");
  }
  if (typeof ref.file !== "string" || !PLAIN_NAME.test(ref.file) || ref.file.includes("..")) throw invalid("fileName");
  let dir;
  try {
    dir = lstatSync(ref.outDir);
  } catch {
    throw invalid("outDir");
  }
  if (!dir.isDirectory()) throw invalid("outDir");
  return join(ref.outDir, ref.file);
}

/** Reads a batch file safely (see the header) and answers its validated lines. */
export function readBatchFile(ref: BatchFileRef, jobsRoot?: string): ScanBatch[] {
  const path = batchPath(ref, jobsRoot);
  let st;
  try {
    st = lstatSync(path);
  } catch {
    throw invalid("missing");
  }
  if (st.isSymbolicLink() || !st.isFile()) throw invalid("notRegular");
  if (st.size > BATCH_MAX_BYTES) throw invalid("tooLarge", { bytes: st.size });
  const nofollow = (constants as { O_NOFOLLOW?: number }).O_NOFOLLOW ?? 0;
  let fd;
  try {
    fd = openSync(path, constants.O_RDONLY | nofollow);
  } catch {
    throw invalid("open");
  }
  let bytes: Buffer;
  try {
    const fst = fstatSync(fd);
    if (!fst.isFile() || fst.ino !== st.ino || fst.dev !== st.dev) throw invalid("replaced");
    if (fst.size > BATCH_MAX_BYTES) throw invalid("tooLarge", { bytes: fst.size });
    // One byte more than the cap: a file that grew past it is refused, not truncated.
    bytes = Buffer.alloc(Math.min(fst.size + 1, BATCH_MAX_BYTES + 1));
    let n = 0;
    for (;;) {
      const r = readSync(fd, bytes, n, bytes.length - n, null);
      if (r === 0) break;
      n += r;
      if (n === bytes.length) break;
    }
    if (n > BATCH_MAX_BYTES) throw invalid("tooLarge", { bytes: n });
    bytes = bytes.subarray(0, n);
  } finally {
    closeSync(fd);
  }
  return parseBatchText(bytes);
}

/** Parses and validates NDJSON bytes (exported for tests). */
export function parseBatchText(bytes: Uint8Array): ScanBatch[] {
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw invalid("utf8");
  }
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (lines.length === 0) throw invalid("empty");
  const out: ScanBatch[] = [];
  let entries = 0;
  lines.forEach((line, i) => {
    let value: unknown;
    try {
      value = JSON.parse(line);
    } catch {
      throw invalid("json", { line: i + 1 });
    }
    const problem = checkObject(value, SPECS.ScanBatch, "");
    if (problem !== undefined) throw invalid("schema", { line: i + 1, at: problem });
    const batch = value as ScanBatch;
    entries += batch.entries.length;
    if (entries > BATCH_MAX_ENTRIES) throw invalid("tooManyEntries", { line: i + 1 });
    out.push(batch);
  });
  return out;
}

type Check = (v: unknown, at: string) => string | undefined;
interface Field {
  readonly required: boolean;
  readonly check: Check;
}
type Spec = Readonly<Record<string, Field>>;

const B64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const I64 = /^-?(?:0|[1-9][0-9]{0,18})$/;
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const ENTRY_TYPES = new Set(["file", "dir", "symlink", "other"]);
const PLACEHOLDERS = new Set<Placeholder>(["dataless", "icloudStub", "nextcloud", "owncloud", "recallOnOpen", "recallOnData", "offline", "unpinned"]);

const ok = undefined;
const bool: Check = (v, at) => (typeof v === "boolean" ? ok : at);
const str: Check = (v, at) => (typeof v === "string" ? ok : at);
const b64: Check = (v, at) => (typeof v === "string" && B64.test(v) ? ok : at);
const nonEmptyB64: Check = (v, at) => (typeof v === "string" && v.length > 0 && B64.test(v) ? ok : at);
const i64: Check = (v, at) => {
  if (typeof v !== "string" || !I64.test(v)) return at;
  const n = BigInt(v);
  return n >= I64_MIN && n <= I64_MAX ? ok : at;
};
const uint = (max: number): Check => (v, at) => (typeof v === "number" && Number.isSafeInteger(v) && v >= 0 && v <= max ? ok : at);
const nullOr = (c: Check): Check => (v, at) => (v === null ? ok : c(v, at));
const oneOf = (set: ReadonlySet<string>): Check => (v, at) => (typeof v === "string" && set.has(v) ? ok : at);
const arrayOf = (c: Check): Check => (v, at) => {
  if (!Array.isArray(v)) return at;
  for (let i = 0; i < v.length; i++) {
    const p = c(v[i], `${at}[${i}]`);
    if (p !== undefined) return p;
  }
  return ok;
};
const object = (spec: Spec): Check => (v, at) => checkObject(v, spec, at);

function checkObject(v: unknown, spec: Spec, at: string): string | undefined {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return at || "$";
  const o = v as Record<string, unknown>;
  for (const key of Object.keys(o)) if (!(key in spec)) return `${at}.${key}`;
  for (const [key, field] of Object.entries(spec)) {
    if (!(key in o)) {
      if (field.required) return `${at}.${key}`;
      continue;
    }
    const p = field.check(o[key], `${at}.${key}`);
    if (p !== undefined) return p;
  }
  return ok;
}

const req = (check: Check): Field => ({ required: true, check });
const opt = (check: Check): Field => ({ required: false, check });

const RawRelSpec: Spec = { components: req(arrayOf(nonEmptyB64)) };
const ScanEntrySpec: Spec = {
  automount: req(bool),
  dev: opt(nullOr(str)),
  dir: req(object(RawRelSpec)),
  entryType: req(oneOf(ENTRY_TYPES)),
  excluded: opt(nullOr(str)),
  fileId: opt(nullOr(b64)),
  mountRoot: req(bool),
  mtimeNs: req(i64),
  name: req(nonEmptyB64),
  nameLossy: req(bool),
  nameNfc: req(str),
  placeholder: opt(nullOr(oneOf(PLACEHOLDERS))),
  relKey: req(str),
  reparseTag: opt(nullOr(uint(0xffff_ffff))),
  size: req(uint(Number.MAX_SAFE_INTEGER)),
} satisfies Record<keyof ScanEntry, Field>;
const DirDoneSpec: Spec = {
  code: opt(nullOr(str)),
  complete: req(bool),
  dir: req(object(RawRelSpec)),
  entries: req(uint(0xffff_ffff)),
  mtimeNs: opt(nullOr(i64)),
} satisfies Record<keyof DirDone, Field>;
const ScanBatchSpec: Spec = {
  dirsDone: req(arrayOf(object(DirDoneSpec))),
  entries: req(arrayOf(object(ScanEntrySpec))),
  seq: req(uint(Number.MAX_SAFE_INTEGER)),
} satisfies Record<keyof ScanBatch, Field>;

/** The validated shapes, by their generated names (the drift test reads them). */
export const SPECS = { RawRel: RawRelSpec, ScanEntry: ScanEntrySpec, DirDone: DirDoneSpec, ScanBatch: ScanBatchSpec } as const;
