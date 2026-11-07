// Applying scan batches to a catalog (plan 5D Task 7a; Task 7b adds the diff on top). Runs in
// the indexer thread only, inside the transactions `indexer-thread.ts` opens (≤ 2,000 rows).
//
// What 7a does with a validated batch, entry by entry, in order:
// - a `dir` entry upserts its row under its parent, keyed `(parent, raw name)`; its `mtimeNs`
//   is **not** taken from the parent's listing: only the directory's own complete listing
//   (`DirDone`) records it, so a poll compares against what was last listed;
// - a `file` entry inserts or refreshes its row, keyed `(dirId, raw name)`; a different size,
//   mtime or file id is a change (fingerprint, probe and every value derived from them cleared,
//   `lastChange` set); a placeholder is `cloudOnly`; excluded files, symlinks and other
//   entries are not catalogued;
// - a `DirDone` records the listing's `mtimeNs`, `listedGen` (complete listings only) and
//   `code`; an incomplete listing of a present directory makes it `unreadable`, a complete
//   one makes an unreadable directory present again.
// Marking rows missing, renames, moves, hard links, collisions and empty-listing suspicion
// are 7b's diff (`diff.ts`), which runs at the same points.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import type { DirDone, RawRel, ScanBatch, ScanEntry } from "@finisher/core-types";
import { type FtsStatements, ftsAdd, prepareFts } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";

/** The root directory's row (created by `core` 0001). */
export const ROOT_DIR = 1;

export interface DirRef {
  readonly id: number;
  readonly relKey: string;
}

/** One catalog's apply state in the indexer: its statements and a cache of directory chains. */
export interface ApplyConn {
  readonly db: DatabaseSync;
  readonly st: ApplyStatements;
  readonly fts: FtsStatements;
  /** Directory chain (base64 components joined by `,`) → row. */
  readonly dirs: Map<string, DirRef>;
}

export interface ApplyStatements {
  readonly dirGet: StatementSync;
  readonly dirInsert: StatementSync;
  readonly dirUpdate: StatementSync;
  readonly dirDone: StatementSync;
  readonly fileGet: StatementSync;
  readonly fileInsert: StatementSync;
  readonly fileSeen: StatementSync;
  readonly fileChanged: StatementSync;
}

export function prepareApply(db: DatabaseSync): ApplyConn {
  const st: ApplyStatements = {
    dirGet: db.prepare("SELECT id, relKey FROM dirs WHERE parent = ? AND name = ?"),
    dirInsert: db.prepare(
      "INSERT INTO dirs (parent, name, nameNfc, relKey, fileId, devId, mount, placeholder, state, code, lastSeen) " +
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    ),
    dirUpdate: db.prepare(
      "UPDATE dirs SET nameNfc = ?, relKey = ?, fileId = ?, devId = ?, mount = ?, placeholder = ?, state = ?, code = ?, lastSeen = ? WHERE id = ?",
    ),
    dirDone: db.prepare(
      "UPDATE dirs SET mtimeNs = coalesce(?, mtimeNs), listedGen = CASE WHEN ? THEN ? ELSE listedGen END, code = ?, " +
        "state = CASE WHEN ? THEN (CASE WHEN state = 'unreadable' THEN 'present' ELSE state END) " +
        "WHEN state = 'present' THEN 'unreadable' ELSE state END, lastSeen = ? WHERE id = ?",
    ),
    fileGet: db.prepare(
      "SELECT id, size, CAST(mtimeNs AS TEXT) AS mtimeNs, fileId, state, code FROM files WHERE dirId = ? AND name = ?",
    ),
    fileInsert: db.prepare(
      "INSERT INTO files (dirId, name, nameNfc, nameLossy, relKey, ext, size, mtimeNs, fileId, placeholder, state, " +
        "firstSeen, lastSeen, lastChange) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    ),
    fileSeen: db.prepare(
      "UPDATE files SET nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, placeholder = ?, state = ?, code = ?, " +
        "lastSeen = ?, missingSince = NULL WHERE id = ?",
    ),
    fileChanged: db.prepare(
      "UPDATE files SET nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, size = ?, mtimeNs = ?, fileId = ?, placeholder = ?, " +
        "state = ?, code = NULL, fp = NULL, fullHash = NULL, kind = NULL, probe = NULL, probeVersion = NULL, durationMs = NULL, " +
        "width = NULL, height = NULL, lastSeen = ?, lastChange = ?, missingSince = NULL WHERE id = ?",
    ),
  };
  return { db, st, fts: prepareFts(db), dirs: new Map([["", { id: ROOT_DIR, relKey: "" }]]) };
}

/** A batch in apply order: each line's entries, then its `dirsDone`. */
export type BatchRow = { readonly entry: ScanEntry } | { readonly done: DirDone };

export function rowsOf(batches: ReadonlyArray<ScanBatch>): BatchRow[] {
  const rows: BatchRow[] = [];
  for (const b of batches) {
    for (const entry of b.entries) rows.push({ entry });
    for (const done of b.dirsDone) rows.push({ done });
  }
  return rows;
}

const chainKey = (rel: RawRel): string => rel.components.join(",");
const raw = (b64: string): Uint8Array => Buffer.from(b64, "base64");

/** Looks a directory chain up in the cache, then the catalog (caching every prefix). */
export function findDir(conn: ApplyConn, rel: RawRel): DirRef | undefined {
  const key = chainKey(rel);
  const hit = conn.dirs.get(key);
  if (hit !== undefined) return hit;
  let ref: DirRef = { id: ROOT_DIR, relKey: "" };
  for (let i = 0; i < rel.components.length; i++) {
    const prefix = rel.components.slice(0, i + 1).join(",");
    const cached = conn.dirs.get(prefix);
    if (cached !== undefined) {
      ref = cached;
      continue;
    }
    const row = conn.st.dirGet.get(ref.id, raw(rel.components[i] as string)) as { id: number; relKey: string } | undefined;
    if (row === undefined) return undefined;
    ref = { id: row.id, relKey: row.relKey };
    conn.dirs.set(prefix, ref);
  }
  return ref;
}

/**
 * Checks, before anything is written, that every entry's and listing's directory is in the
 * catalog or created by a `dir` entry earlier in the batch; else `LIBRARY_BATCH_INVALID`.
 */
export function checkChains(conn: ApplyConn, rows: ReadonlyArray<BatchRow>): void {
  const created = new Set<string>();
  const known = (rel: RawRel): boolean => created.has(chainKey(rel)) || findDir(conn, rel) !== undefined;
  rows.forEach((row, i) => {
    const rel = "entry" in row ? row.entry.dir : row.done.dir;
    if (!known(rel)) {
      throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir", row: i });
    }
    if ("entry" in row && row.entry.entryType === "dir") created.add([...row.entry.dir.components, row.entry.name].join(","));
  });
}

export interface ApplyCounts {
  dirs: number;
  added: number;
  changed: number;
  seen: number;
  skipped: number;
  listings: number;
}

export const emptyCounts = (): ApplyCounts => ({ dirs: 0, added: 0, changed: 0, seen: 0, skipped: 0, listings: 0 });

/** A file's extension for the `kinds` filter: lower-case, ≤ 16 characters, never a dot-file's name. */
export function extOf(nameNfc: string): string | null {
  const dot = nameNfc.lastIndexOf(".");
  if (dot <= 0 || dot === nameNfc.length - 1) return null;
  const ext = nameNfc.slice(dot + 1).toLowerCase();
  return ext.length <= 16 ? ext : null;
}

function dirState(e: ScanEntry): { state: string; code: string | null } {
  if (e.excluded != null) return { state: "excluded", code: e.excluded };
  if (e.placeholder != null) return { state: "cloudOnly", code: null };
  return { state: "present", code: null };
}

function applyDir(conn: ApplyConn, parent: DirRef, e: ScanEntry, at: number, counts: ApplyCounts): void {
  const name = raw(e.name);
  const fileId = e.fileId == null ? null : raw(e.fileId);
  const mount = e.mountRoot ? (e.dev ?? "") : null;
  const { state, code } = dirState(e);
  const known = conn.st.dirGet.get(parent.id, name) as { id: number } | undefined;
  let id: number;
  if (known === undefined) {
    const row = conn.st.dirInsert.get(parent.id, name, e.nameNfc, e.relKey, fileId, e.dev ?? null, mount, e.placeholder ?? null, state, code, at) as {
      id: number;
    };
    id = row.id;
  } else {
    id = known.id;
    conn.st.dirUpdate.run(e.nameNfc, e.relKey, fileId, e.dev ?? null, mount, e.placeholder ?? null, state, code, at, id);
  }
  conn.dirs.set([...e.dir.components, e.name].join(","), { id, relKey: e.relKey });
  counts.dirs++;
}

function sameId(a: Uint8Array | null, b: Uint8Array | null): boolean {
  if (a === null || b === null) return a === b;
  return Buffer.compare(a, b) === 0;
}

function applyFile(conn: ApplyConn, dir: DirRef, e: ScanEntry, at: number, counts: ApplyCounts): void {
  if (e.excluded != null) {
    counts.skipped++;
    return;
  }
  const name = raw(e.name);
  const fileId = e.fileId == null ? null : raw(e.fileId);
  const mtime = BigInt(e.mtimeNs);
  const ext = extOf(e.nameNfc);
  const placeholder = e.placeholder ?? null;
  const lossy = e.nameLossy ? 1 : 0;
  const known = conn.st.fileGet.get(dir.id, name) as
    | { id: number; size: number; mtimeNs: string; fileId: Uint8Array | null; state: string; code: string | null }
    | undefined;
  if (known === undefined) {
    const state = placeholder === null ? "present" : "cloudOnly";
    const row = conn.st.fileInsert.get(dir.id, name, e.nameNfc, lossy, e.relKey, ext, e.size, mtime, fileId, placeholder, state, at, at, at) as {
      id: number;
    };
    ftsAdd(conn.fts, row.id, e.nameNfc, dir.relKey);
    counts.added++;
    return;
  }
  const changed = known.size !== e.size || known.mtimeNs !== mtime.toString() || !sameId(known.fileId, fileId);
  if (changed) {
    const state = placeholder === null ? "present" : "cloudOnly";
    conn.st.fileChanged.run(e.nameNfc, lossy, e.relKey, ext, e.size, mtime, fileId, placeholder, state, at, at, known.id);
    counts.changed++;
    return;
  }
  // Unchanged: a probe's verdict (`probeFailed`, `skipped`) stands; anything else is present
  // again (or still a placeholder).
  const keep = placeholder === null && (known.state === "probeFailed" || known.state === "skipped");
  const state = placeholder !== null ? "cloudOnly" : keep ? known.state : "present";
  conn.st.fileSeen.run(e.nameNfc, lossy, e.relKey, ext, placeholder, state, keep ? known.code : null, at, known.id);
  counts.seen++;
}

function applyDone(conn: ApplyConn, d: DirDone, gen: number, at: number, counts: ApplyCounts): void {
  const dir = findDir(conn, d.dir);
  if (dir === undefined) throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir" });
  const mtime = d.mtimeNs == null ? null : BigInt(d.mtimeNs);
  const complete = d.complete ? 1 : 0;
  conn.st.dirDone.run(d.complete ? mtime : null, complete, gen, d.code ?? null, complete, at, dir.id);
  counts.listings++;
}

/** Applies `rows` (one transaction's worth, ≤ 2,000) in order. */
export function applyRows(conn: ApplyConn, rows: ReadonlyArray<BatchRow>, gen: number, at: number, counts: ApplyCounts): void {
  for (const row of rows) {
    if ("done" in row) {
      applyDone(conn, row.done, gen, at, counts);
      continue;
    }
    const e = row.entry;
    const dir = findDir(conn, e.dir);
    if (dir === undefined) throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir" });
    if (e.entryType === "dir") applyDir(conn, dir, e, at, counts);
    else if (e.entryType === "file") applyFile(conn, dir, e, at, counts);
    else counts.skipped++;
  }
}
