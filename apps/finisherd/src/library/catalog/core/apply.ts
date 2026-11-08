// Applying scan batches to a catalog (plan 5D Tasks 7a, 7b). Runs in the indexer thread only,
// inside the transactions `indexer-thread.ts` opens (≤ 2,000 rows each).
//
// A validated batch, row by row, in order (each line's entries, then its `dirsDone`):
// - a `dir` entry upserts its row under its parent, keyed `(parent, raw name)`; its `mtimeNs`
//   is **not** taken from the parent's listing: only the directory's own complete listing
//   records it, so a poll compares against what was last listed. A placeholder directory's
//   subtree becomes `cloudOnly`; an excluded one's goes missing (`EXCLUDED`) once the
//   listing that reported it closes (`diff.ts`);
// - a `file` entry inserts or refreshes its row, keyed `(dirId, raw name)`:
//   - a missing row that comes back with its size and mtime is present again with its
//     fingerprint (re-probed if it had none); anything else that differs (size, mtime, or two
//     known file ids) is a change: fingerprint, probe and their derived values cleared,
//     `lastChange` set;
//   - a new name may be a rename (`diff.ts`: `dev:fileId` hints);
//   - a placeholder is `cloudOnly`; excluded files, symlinks and others are not catalogued
//     (a row of that name is then missing once the listing closes, an excluded file's with
//     code `EXCLUDED`);
// - every entry counts toward its `(job, dir)` listing, which its `DirDone` closes (`diff.ts`),
//   and clears its directory's empty suspicion;
// - a `DirDone` records `mtimeNs`, `listedGen`, `listedAt` and `code` only for a listing that
//   closed and started no earlier than the one recorded last; an incomplete listing makes a
//   present directory `unreadable`, such a closed one makes an unreadable directory present.
//
// Applying is a generator (`steps.ts`): subtree changes yield between steps of rows.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import type { DirDone, RawRel, ScanBatch, ScanEntry } from "@finisher/core-types";
import { type FtsStatements, ftsAdd, ftsReplace, prepareFts } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";
import {
  type DiffConn, type DiffCtx, cloudSubtree, closeListing, endListing, holds, listingOf, prefetchIds, prepareDiff, renameCandidate, touch,
  unsuspect, usableId,
} from "./diff.ts";
import { checkChain, checkEntry } from "./entry-check.ts";
import { type Budget, type Work, room } from "./steps.ts";

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
  readonly diff: DiffConn;
  /** Directory chain (base64 components joined by `,`) → row. */
  readonly dirs: Map<string, DirRef>;
}

export interface ApplyStatements {
  readonly dirGet: StatementSync;
  readonly dirInsert: StatementSync;
  readonly dirUpdate: StatementSync;
  readonly dirDone: StatementSync;
  readonly dirDoneStale: StatementSync;
  readonly dirIncomplete: StatementSync;
  readonly fileGet: StatementSync;
  readonly fileInsert: StatementSync;
  readonly fileSeen: StatementSync;
  readonly fileChanged: StatementSync;
}

export function prepareApply(db: DatabaseSync): ApplyConn {
  const st: ApplyStatements = {
    dirGet: db.prepare("SELECT id, relKey, state FROM dirs WHERE parent = ? AND name = ?"),
    dirInsert: db.prepare(
      "INSERT INTO dirs (parent, name, nameNfc, relKey, fileId, devId, mount, placeholder, state, code, lastSeen) " +
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    ),
    dirUpdate: db.prepare(
      "UPDATE dirs SET nameNfc = ?, relKey = ?, fileId = ?, devId = ?, mount = ?, placeholder = ?, state = ?, code = ?, lastSeen = ? WHERE id = ?",
    ),
    dirDone: db.prepare(
      "UPDATE dirs SET mtimeNs = coalesce(?, mtimeNs), listedGen = ?, listedAt = ?, code = ?, emptySince = NULL, emptyJob = NULL, " +
        "state = CASE WHEN state = 'unreadable' THEN 'present' ELSE state END, lastSeen = ? WHERE id = ?",
    ),
    // A listing older than the one recorded: it marked what it could, records nothing else.
    dirDoneStale: db.prepare(
      "UPDATE dirs SET emptySince = NULL, emptyJob = NULL, code = CASE WHEN code = 'DIR_EMPTY_SUSPECT' THEN NULL ELSE code END, " +
        "lastSeen = ? WHERE id = ?",
    ),
    dirIncomplete: db.prepare(
      "UPDATE dirs SET code = ?, state = CASE WHEN state = 'present' THEN 'unreadable' ELSE state END, lastSeen = ? WHERE id = ?",
    ),
    fileGet: db.prepare(
      "SELECT id, size, CAST(mtimeNs AS TEXT) AS mtimeNs, fileId, dev, fp, state, code FROM files WHERE dirId = ? AND name = ?",
    ),
    fileInsert: db.prepare(
      "INSERT INTO files (dirId, name, nameNfc, nameLossy, relKey, ext, size, mtimeNs, fileId, dev, placeholder, state, renameOf, " +
        "firstSeen, lastSeen, lastChange) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    ),
    fileSeen: db.prepare(
      "UPDATE files SET nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, fileId = ?, dev = ?, placeholder = ?, state = ?, code = ?, " +
        "lastSeen = ?, missingSince = NULL, missingGen = NULL, probeVersion = CASE WHEN ? AND fp IS NULL THEN NULL ELSE probeVersion END WHERE id = ?",
    ),
    fileChanged: db.prepare(
      "UPDATE files SET nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, size = ?, mtimeNs = ?, fileId = ?, dev = ?, placeholder = ?, " +
        "state = ?, code = NULL, fp = NULL, fullHash = NULL, kind = NULL, probe = NULL, probeVersion = NULL, durationMs = NULL, " +
        "width = NULL, height = NULL, renameOf = NULL, lastSeen = ?, lastChange = ?, missingSince = NULL, missingGen = NULL WHERE id = ?",
    ),
  };
  return { db, st, fts: prepareFts(db), diff: prepareDiff(db), dirs: new Map([["", { id: ROOT_DIR, relKey: "" }]]) };
}

/** Drops cached directory rows and open listings (after a rollback, or at close). */
export function resetApply(conn: ApplyConn): void {
  conn.dirs.clear();
  conn.dirs.set("", { id: ROOT_DIR, relKey: "" });
  conn.diff.listings.clear();
  conn.diff.touched.clear();
  conn.diff.unsuspected.clear();
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
 * catalog or created by a `dir` entry earlier in the batch, and every name and key
 * (`entry-check.ts`); else `LIBRARY_BATCH_INVALID`.
 */
export function checkChains(conn: ApplyConn, rows: ReadonlyArray<BatchRow>): void {
  const created = new Map<string, string>();
  const keyOf = (rel: RawRel): string | undefined => created.get(chainKey(rel)) ?? findDir(conn, rel)?.relKey;
  rows.forEach((row, i) => {
    const rel = "entry" in row ? row.entry.dir : row.done.dir;
    checkChain(rel.components, i);
    const parentKey = keyOf(rel);
    if (parentKey === undefined) {
      throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir", row: i });
    }
    if (!("entry" in row)) return;
    checkEntry(row.entry, parentKey, i);
    if (row.entry.entryType === "dir") created.set([...row.entry.dir.components, row.entry.name].join(","), row.entry.relKey);
  });
}

export interface ApplyCounts {
  dirs: number;
  added: number;
  changed: number;
  seen: number;
  skipped: number;
  listings: number;
  /** Rows a missing row took over by `dev:fileId` (new path, same row). */
  renamed: number;
  /** New rows inserted with `renameOf` a live row. */
  renamePending: number;
  reappeared: number;
  missing: number;
  /** Listings that marked nothing: offline, or their state lost. */
  incomplete: number;
  /** Empty listings of directories with rows (`DIR_EMPTY_SUSPECT`). */
  suspect: number;
  /** Rows with collision bits in the listings this batch closed (`NAME_COLLISION`). */
  collisions: number;
  /** Rows of the `missing` an exclusion took out (code `EXCLUDED`). */
  excluded: number;
}

export const emptyCounts = (): ApplyCounts => ({
  dirs: 0, added: 0, changed: 0, seen: 0, skipped: 0, listings: 0, renamed: 0, renamePending: 0, reappeared: 0, missing: 0,
  incomplete: 0, suspect: 0, collisions: 0, excluded: 0,
});

/** Who applies and when: the job's listings are its own; `at` never goes backwards. */
export type ApplyCtx = DiffCtx;

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

function* applyDir(conn: ApplyConn, parent: DirRef, e: ScanEntry, ctx: ApplyCtx, counts: ApplyCounts, b: Budget): Work {
  const name = raw(e.name);
  const fileId = e.fileId == null ? null : raw(e.fileId);
  const mount = e.mountRoot ? (e.dev ?? "") : null;
  const { state, code } = dirState(e);
  const known = conn.st.dirGet.get(parent.id, name) as { id: number; state: string } | undefined;
  let id: number;
  if (known === undefined) {
    const row = conn.st.dirInsert.get(parent.id, name, e.nameNfc, e.relKey, fileId, e.dev ?? null, mount, e.placeholder ?? null, state, code, ctx.at) as {
      id: number;
    };
    id = row.id;
  } else {
    id = known.id;
    conn.st.dirUpdate.run(e.nameNfc, e.relKey, fileId, e.dev ?? null, mount, e.placeholder ?? null, state, code, ctx.at, id);
    if (state === "excluded") listingOf(conn.diff, ctx, parent.id).excludedDirs.push(id);
    else if (state === "cloudOnly") yield* cloudSubtree(conn.diff, id, b);
  }
  conn.dirs.set([...e.dir.components, e.name].join(","), { id, relKey: e.relKey });
  counts.dirs++;
}

interface Known {
  id: number;
  size: number;
  mtimeNs: string;
  fileId: Uint8Array | null;
  dev: string | null;
  fp: string | null;
  state: string;
  code: string | null;
}

function sameId(a: Uint8Array | null, b: Uint8Array | null): boolean {
  return a === null || b === null || Buffer.compare(a, b) === 0;
}

function applyFile(conn: ApplyConn, dir: DirRef, e: ScanEntry, ctx: ApplyCtx, counts: ApplyCounts): void {
  const { st, diff } = conn;
  const name = raw(e.name);
  if (e.excluded != null) {
    counts.skipped++;
    const row = st.fileGet.get(dir.id, name) as Known | undefined;
    if (row !== undefined && row.state !== "missing") listingOf(diff, ctx, dir.id).excludedFiles.push(row.id);
    return;
  }
  const at = ctx.at;
  const fileId = usableId(e.fileId);
  const dev = e.dev ?? null;
  const mtime = BigInt(e.mtimeNs);
  const ext = extOf(e.nameNfc);
  const placeholder = e.placeholder ?? null;
  const lossy = e.nameLossy ? 1 : 0;
  const fresh = placeholder === null ? "present" : "cloudOnly";
  const known = st.fileGet.get(dir.id, name) as Known | undefined;
  if (known === undefined) {
    const hint = renameCandidate(diff, fileId, dev, e.size, mtime);
    holds(diff, fileId);
    if (hint?.missing === true) {
      diff.st.takeOver.run(dir.id, name, e.nameNfc, lossy, e.relKey, ext, fileId, dev, placeholder, fresh, at, hint.id);
      ftsReplace(conn.fts, hint.id, e.nameNfc, dir.relKey);
      counts.renamed++;
      return;
    }
    const renameOf = hint?.id ?? null;
    // `run()`: its last insert rowid is this row's (RETURNING costs more per insert).
    const newId = Number(st.fileInsert.run(dir.id, name, e.nameNfc, lossy, e.relKey, ext, e.size, mtime, fileId, dev, placeholder, fresh, renameOf, at, at, at).lastInsertRowid);
    ftsAdd(conn.fts, newId, e.nameNfc, dir.relKey);
    if (renameOf === null) counts.added++;
    else counts.renamePending++;
    return;
  }
  holds(diff, fileId);
  const sameSize = known.size === e.size && known.mtimeNs === mtime.toString();
  const wasMissing = known.state === "missing";
  if (!sameSize || (!wasMissing && !sameId(known.fileId, fileId))) {
    st.fileChanged.run(e.nameNfc, lossy, e.relKey, ext, e.size, mtime, fileId, dev, placeholder, fresh, at, at, known.id);
    touch(diff, known.fileId);
    touch(diff, fileId);
    counts.changed++;
    return;
  }
  // Unchanged (or back with its size and mtime): a probe's verdict (`probeFailed`, `skipped`)
  // stands while the row stays live; anything else is present again (or still a placeholder).
  const keep = !wasMissing && placeholder === null && (known.state === "probeFailed" || known.state === "skipped");
  const state = keep ? known.state : fresh;
  st.fileSeen.run(e.nameNfc, lossy, e.relKey, ext, fileId, dev, placeholder, state, keep ? known.code : null, at, wasMissing ? 1 : 0, known.id);
  if (wasMissing || !sameId(known.fileId, fileId) || (known.fileId === null) !== (fileId === null) || known.dev !== dev) {
    touch(diff, known.fileId);
    touch(diff, fileId);
  }
  if (wasMissing) counts.reappeared++;
  else counts.seen++;
}

function* applyDone(conn: ApplyConn, d: DirDone, ctx: ApplyCtx, counts: ApplyCounts, b: Budget): Work {
  const dir = findDir(conn, d.dir);
  if (dir === undefined) throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir" });
  counts.listings++;
  if (!d.complete) {
    endListing(conn.diff, ctx.job, dir.id);
    if (!conn.diff.offline) conn.st.dirIncomplete.run(d.code ?? null, ctx.at, dir.id);
    return;
  }
  const outcome = yield* closeListing(conn.diff, conn.fts, dir.id, d.entries, ctx, counts, b);
  if (outcome.kind !== "closed") return;
  if (outcome.fresh) {
    const mtime = d.mtimeNs == null ? null : BigInt(d.mtimeNs);
    conn.st.dirDone.run(mtime, ctx.gen, outcome.start, d.code ?? null, ctx.at, dir.id);
  } else conn.st.dirDoneStale.run(ctx.at, dir.id);
}

/** Applies `rows` (one transaction's worth, ≤ 2,000) in order, yielding between steps. */
export function* applyRows(conn: ApplyConn, rows: ReadonlyArray<BatchRow>, ctx: ApplyCtx, counts: ApplyCounts, b: Budget): Work {
  const ids: string[] = [];
  for (const row of rows) {
    if ("entry" in row && row.entry.entryType === "file") {
      const id = usableId(row.entry.fileId);
      if (id !== null) ids.push(Buffer.from(id).toString("hex"));
    }
  }
  prefetchIds(conn.diff, ids);
  conn.diff.unsuspected.clear();
  try {
    yield* applyEach(conn, rows, ctx, counts, b);
  } finally {
    conn.diff.ids = undefined;
  }
}

function* applyEach(conn: ApplyConn, rows: ReadonlyArray<BatchRow>, ctx: ApplyCtx, counts: ApplyCounts, b: Budget): Work {
  for (const row of rows) {
    if (b.rows >= b.step) yield* room(b, 1);
    else b.rows++;
    if ("done" in row) {
      yield* applyDone(conn, row.done, ctx, counts, b);
      continue;
    }
    const e = row.entry;
    const dir = findDir(conn, e.dir);
    if (dir === undefined) throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "unknownDir" });
    listingOf(conn.diff, ctx, dir.id).seen++;
    unsuspect(conn.diff, dir.id);
    if (e.entryType === "dir") yield* applyDir(conn, dir, e, ctx, counts, b);
    else if (e.entryType === "file") applyFile(conn, dir, e, ctx, counts);
    else counts.skipped++;
  }
}
