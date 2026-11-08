// The catalog diff (plan 5D Task 7b, Decisions 11, 13, 18, 22, 25): what a closed listing
// means for the rows it did not see, renames by `dev:fileId`, moves by fingerprint, hard
// links and collisions. Runs in the indexer thread only, inside `apply.ts`'s transactions.
//
// **Listings.** A row is seen by a listing when its entry is applied; each `(job, dir)`
// listing remembers the transaction time `start` of its first entry and how many entries it
// saw. Its `DirDone {complete: true}` then marks **missing** the directory's rows with
// `lastSeen < start` (not seen by this listing, nor by any other since it began: so a poll
// and a full scan listing the same directory never undo each other), and every subdirectory
// so marked with its whole subtree. Nothing is marked when:
// - the listing is incomplete, or the library is offline (Decision 18);
// - it saw fewer entries than its `DirDone` counts (its first entries were applied before a
//   restart or a rolled-back transaction: the listing state is lost);
// - it is empty while the directory holds rows (`DIR_EMPTY_SUSPECT`, `emptySince` set): only
//   another job's empty listing at least 60 s later confirms it. Such a listing records no
//   `mtimeNs` and no `listedGen`, so the next poll and a resumed scan list it again.
// An empty **root** listing while the catalog holds files fails the whole batch file before
// anything is written (`ROOT_EMPTY_SUSPECT`): the service puts the library offline.
//
// **Renames and hard links** (Decision 22). File ids are hints keyed with the device; an
// all-zero id counts as none. A new entry whose `dev:fileId`, size and mtime match a
// **missing** row takes that row over (it moves to the new path with its id, fingerprint and
// history); if they match a live row it is inserted with `renameOf` that row. When the old
// row's directory closes a listing without it, the old row takes the new path and the new
// row is dropped; when the listing saw it, `renameOf` is cleared and the two stay rows of
// their own. `hardlink` is derived: a live row sharing its `dev:fileId` with another live row.
//
// **Moves** (`ops.ts` `applyProbed`): a row's first fingerprint, on a row never changed since
// it was inserted, that equals a missing row's makes the missing row take the new path.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import { type FtsStatements, ftsRemove, ftsReplace } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";
import { type CollisionStatements, prepareCollisions, recomputeCollisions } from "./collisions.ts";

/** A second empty listing confirms a drop to zero only this long after the first. */
export const EMPTY_CONFIRM_MS = 60_000;
/** Listing state is kept for this many jobs per catalog (the oldest is dropped first). */
const MAX_JOBS = 64;

export interface Listing {
  readonly start: number;
  seen: number;
}

export interface DiffStatements {
  readonly candidates: StatementSync;
  readonly takeOver: StatementSync;
  readonly fileFull: StatementSync;
  readonly deleteFile: StatementSync;
  readonly merge: StatementSync;
  readonly dirKey: StatementSync;
  readonly pointing: StatementSync;
  readonly clearRenames: StatementSync;
  readonly clearSeenRenames: StatementSync;
  readonly goneFiles: StatementSync;
  readonly markFiles: StatementSync;
  readonly goneDirs: StatementSync;
  readonly markDirs: StatementSync;
  readonly markSubtreeFiles: StatementSync;
  readonly markSubtreeDirs: StatementSync;
  readonly cloudSubtreeFiles: StatementSync;
  readonly cloudSubtreeDirs: StatementSync;
  readonly hasChildren: StatementSync;
  readonly dirEmpty: StatementSync;
  readonly suspect: StatementSync;
  readonly seenOnly: StatementSync;
  readonly linkGroup: StatementSync;
  readonly setLink: StatementSync;
  readonly anyFile: StatementSync;
  readonly idsPresent: StatementSync;
}

export interface DiffConn {
  readonly st: DiffStatements;
  readonly coll: CollisionStatements;
  /** job → dirId → the open listing. */
  readonly listings: Map<string, Map<number, Listing>>;
  /** File ids (hex) whose `hardlink` flags need recomputing at the end of the transaction. */
  readonly touched: Set<string>;
  /**
   * File ids (hex) of the current transaction's entries that some row holds (or held at its
   * start): an id outside it needs no rename lookup. `undefined` outside `applyRows`.
   */
  ids: Set<string> | undefined;
  offline: boolean;
}

const SUBTREE = "WITH RECURSIVE sub (id) AS (SELECT ? UNION ALL SELECT d.id FROM dirs d JOIN sub ON d.parent = sub.id)";
const MISSING_SET = "state = 'missing', missingSince = ?, renameOf = NULL, hardlink = 0";

export function prepareDiff(db: DatabaseSync): DiffConn {
  const st: DiffStatements = {
    candidates: db.prepare(
      "SELECT id, dev, size, CAST(mtimeNs AS TEXT) AS mtimeNs, state, missingSince FROM files WHERE fileId = ? ORDER BY id",
    ),
    takeOver: db.prepare(
      "UPDATE files SET dirId = ?, name = ?, nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, fileId = ?, dev = ?, placeholder = ?, " +
        "state = ?, code = NULL, lastSeen = ?, missingSince = NULL, renameOf = NULL, " +
        "probeVersion = CASE WHEN fp IS NULL THEN NULL ELSE probeVersion END WHERE id = ?",
    ),
    fileFull: db.prepare(
      "SELECT id, dirId, name, nameNfc, nameLossy, relKey, ext, collision, size, CAST(mtimeNs AS TEXT) AS mtimeNs, fileId, dev, placeholder, fp, fullHash, " +
        "kind, probe, probeVersion, durationMs, width, height, state, code, lastSeen FROM files WHERE id = ?",
    ),
    deleteFile: db.prepare("DELETE FROM files WHERE id = ?"),
    merge: db.prepare(
      "UPDATE files SET dirId = ?, name = ?, nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, collision = ?, size = ?, mtimeNs = ?, " +
        "fileId = ?, dev = ?, placeholder = ?, lastSeen = ?, state = ?, code = ?, fp = ?, fullHash = ?, kind = ?, probe = ?, " +
        "probeVersion = ?, durationMs = ?, width = ?, height = ?, renameOf = NULL, missingSince = NULL WHERE id = ?",
    ),
    dirKey: db.prepare("SELECT relKey FROM dirs WHERE id = ?"),
    pointing: db.prepare("SELECT id FROM files WHERE renameOf = ? AND state != 'missing' ORDER BY id"),
    clearRenames: db.prepare("UPDATE files SET renameOf = NULL WHERE renameOf = ?"),
    clearSeenRenames: db.prepare(
      "UPDATE files SET renameOf = NULL WHERE renameOf IN (SELECT id FROM files WHERE dirId = ? AND state != 'missing' AND lastSeen >= ?)",
    ),
    // Selected first, then updated only when there is something to mark: `RETURNING` costs
    // ~100 µs a statement, and almost every listing marks nothing.
    goneFiles: db.prepare("SELECT id, fileId FROM files WHERE dirId = ? AND state != 'missing' AND lastSeen < ?"),
    markFiles: db.prepare(`UPDATE files SET ${MISSING_SET} WHERE dirId = ? AND state != 'missing' AND lastSeen < ?`),
    goneDirs: db.prepare("SELECT id FROM dirs WHERE parent = ? AND state != 'missing' AND lastSeen < ?"),
    markDirs: db.prepare("UPDATE dirs SET state = 'missing' WHERE parent = ? AND state != 'missing' AND lastSeen < ?"),
    markSubtreeFiles: db.prepare(
      `${SUBTREE} UPDATE files SET ${MISSING_SET} WHERE dirId IN (SELECT id FROM sub) AND state != 'missing' RETURNING id, fileId`,
    ),
    markSubtreeDirs: db.prepare(`${SUBTREE} UPDATE dirs SET state = 'missing' WHERE id IN (SELECT id FROM sub) AND id != ? AND state != 'missing'`),
    cloudSubtreeFiles: db.prepare(
      `${SUBTREE} UPDATE files SET state = 'cloudOnly' WHERE dirId IN (SELECT id FROM sub) AND state NOT IN ('missing', 'cloudOnly')`,
    ),
    cloudSubtreeDirs: db.prepare(
      `${SUBTREE} UPDATE dirs SET state = 'cloudOnly' WHERE id IN (SELECT id FROM sub) AND id != ? AND state NOT IN ('missing', 'excluded', 'cloudOnly')`,
    ),
    hasChildren: db.prepare(
      "SELECT EXISTS (SELECT 1 FROM files WHERE dirId = ? AND state != 'missing') OR " +
        "EXISTS (SELECT 1 FROM dirs WHERE parent = ? AND state != 'missing') AS x",
    ),
    dirEmpty: db.prepare("SELECT emptySince, emptyJob FROM dirs WHERE id = ?"),
    suspect: db.prepare(
      "UPDATE dirs SET emptySince = coalesce(emptySince, ?), emptyJob = coalesce(emptyJob, ?), code = 'DIR_EMPTY_SUSPECT', lastSeen = ? WHERE id = ?",
    ),
    seenOnly: db.prepare("UPDATE dirs SET lastSeen = ? WHERE id = ?"),
    linkGroup: db.prepare("SELECT id, dev, state, hardlink FROM files WHERE fileId = ?"),
    setLink: db.prepare("UPDATE files SET hardlink = ? WHERE id = ?"),
    anyFile: db.prepare("SELECT EXISTS (SELECT 1 FROM files WHERE state != 'missing') AS x"),
    idsPresent: db.prepare("SELECT DISTINCT hex(fileId) AS id FROM files WHERE fileId IN (SELECT unhex(value) FROM json_each(?))"),
  };
  return { st, coll: prepareCollisions(db), listings: new Map(), touched: new Set(), ids: undefined, offline: false };
}

/** A file id as stored: `null` for none or an all-zero id (servers that answer 0). */
export function usableId(b64: string | null | undefined): Uint8Array | null {
  if (b64 == null) return null;
  const id = Buffer.from(b64, "base64");
  return id.length === 0 || id.every((b) => b === 0) ? null : id;
}

export function touch(diff: DiffConn, id: Uint8Array | null): void {
  if (id !== null) diff.touched.add(Buffer.from(id.buffer, id.byteOffset, id.byteLength).toString("hex"));
}

/** The open listing of `dirId` by `job`, created at `at` by its first entry. */
export function listingOf(diff: DiffConn, job: string, dirId: number, at: number): Listing {
  let jobs = diff.listings.get(job);
  if (jobs === undefined) {
    if (diff.listings.size >= MAX_JOBS) diff.listings.delete(diff.listings.keys().next().value as string);
    jobs = new Map();
    diff.listings.set(job, jobs);
  }
  let l = jobs.get(dirId);
  if (l === undefined) {
    l = { start: at, seen: 0 };
    jobs.set(dirId, l);
  }
  return l;
}

/** Ends (and answers) the open listing of `dirId` by `job`. */
export function endListing(diff: DiffConn, job: string, dirId: number): Listing | undefined {
  const jobs = diff.listings.get(job);
  const l = jobs?.get(dirId);
  if (jobs !== undefined) {
    jobs.delete(dirId);
    if (jobs.size === 0) diff.listings.delete(job);
  }
  return l;
}

export interface Candidate {
  readonly id: number;
  readonly missing: boolean;
}

/**
 * The rename hint for a new entry: the missing row it takes over (latest `missingSince`, then
 * lowest id), else the live row it may be a rename of (lowest id), else none.
 */
/** Loads `diff.ids` for a transaction's file ids (hex), one query instead of one per entry. */
export function prefetchIds(diff: DiffConn, hexIds: ReadonlyArray<string>): void {
  const rows = hexIds.length === 0 ? [] : (diff.st.idsPresent.all(JSON.stringify(hexIds)) as Array<{ id: string }>);
  diff.ids = new Set(rows.map((r) => r.id.toLowerCase()));
}

const hexOf = (id: Uint8Array): string => Buffer.from(id.buffer, id.byteOffset, id.byteLength).toString("hex");

/** Records that a row now holds `fileId` (for `diff.ids`). */
export function holds(diff: DiffConn, fileId: Uint8Array | null): void {
  if (fileId !== null) diff.ids?.add(hexOf(fileId));
}

export function renameCandidate(diff: DiffConn, fileId: Uint8Array | null, dev: string | null, size: number, mtime: bigint): Candidate | undefined {
  if (fileId === null) return undefined;
  if (diff.ids !== undefined && !diff.ids.has(hexOf(fileId))) return undefined;
  const all = diff.st.candidates.all(fileId) as Array<{ id: number; dev: string | null; size: number; mtimeNs: string; state: string; missingSince: number | null }>;
  // Another row with this id may make the new one a hard link, hint or not.
  if (all.length > 0) touch(diff, fileId);
  const m = mtime.toString();
  const rows = all.filter((r) => r.dev === dev && r.size === size && r.mtimeNs === m);
  let best: { id: number; since: number } | undefined;
  for (const r of rows) {
    if (r.state !== "missing") continue;
    const since = r.missingSince ?? 0;
    if (best === undefined || since > best.since) best = { id: r.id, since };
  }
  if (best !== undefined) return { id: best.id, missing: true };
  const live = rows[0];
  return live === undefined ? undefined : { id: live.id, missing: false };
}

type Full = Record<string, unknown> & { id: number; dirId: number; fp: string | null; fileId: Uint8Array | null };

/**
 * `target` (a missing row) takes `source`'s path and metadata and keeps its id, first sight,
 * last change and (with `keepProbe`) its probe; `source` is deleted. Answers false when either
 * row is gone.
 */
export function mergeInto(diff: DiffConn, fts: FtsStatements, target: number, source: number, keepProbe: boolean): boolean {
  const t = diff.st.fileFull.get(target) as Full | undefined;
  const s = diff.st.fileFull.get(source) as Full | undefined;
  if (t === undefined || s === undefined) return false;
  const p = keepProbe ? t : s;
  const state = keepProbe ? (s.placeholder != null ? "cloudOnly" : "present") : (s.state as string);
  const code = keepProbe ? null : s.code;
  ftsRemove(fts, s.id);
  diff.st.deleteFile.run(s.id);
  diff.st.clearRenames.run(s.id);
  diff.st.merge.run(
    s.dirId, s.name as Uint8Array, s.nameNfc as string, s.nameLossy as number, s.relKey as string, (s.ext as string | null) ?? null,
    s.collision as number, s.size as number, BigInt(s.mtimeNs as string), s.fileId, (s.dev as string | null) ?? null,
    (s.placeholder as string | null) ?? null, s.lastSeen as number, state, (code as string | null) ?? null, p.fp,
    (p.fullHash as string | null) ?? null, (p.kind as string | null) ?? null, (p.probe as Uint8Array | null) ?? null,
    (p.probeVersion as number | null) ?? null, (p.durationMs as number | null) ?? null, (p.width as number | null) ?? null,
    (p.height as number | null) ?? null, t.id,
  );
  diff.st.clearRenames.run(t.id);
  const dirKey = (diff.st.dirKey.get(s.dirId) as { relKey: string }).relKey;
  ftsReplace(fts, t.id, s.nameNfc as string, dirKey);
  touch(diff, t.fileId);
  touch(diff, s.fileId);
  return true;
}

/** Resolves pending renames of rows just marked missing; answers how many merged. */
function resolveRenames(diff: DiffConn, fts: FtsStatements, missing: ReadonlyArray<{ id: number; fileId: Uint8Array | null }>): number {
  let n = 0;
  for (const m of missing) {
    touch(diff, m.fileId);
    const next = diff.st.pointing.get(m.id) as { id: number } | undefined;
    if (next !== undefined && mergeInto(diff, fts, m.id, next.id, (diff.st.fileFull.get(m.id) as Full).fp !== null)) n++;
  }
  return n;
}

export interface MarkCounts {
  missing: number;
  renamed: number;
}

/** Marks `dirId`'s subtree (below it) missing, then resolves renames of its files. */
export function markSubtree(diff: DiffConn, fts: FtsStatements, dirId: number, at: number, counts: MarkCounts): void {
  diff.st.markSubtreeDirs.run(dirId, dirId);
  const files = diff.st.markSubtreeFiles.all(dirId, at) as Array<{ id: number; fileId: Uint8Array | null }>;
  counts.missing += files.length;
  counts.renamed += resolveRenames(diff, fts, files);
}

/** A placeholder directory's subtree becomes `cloudOnly`, never missing (Decision 13). */
export function cloudSubtree(diff: DiffConn, dirId: number): void {
  diff.st.cloudSubtreeDirs.run(dirId, dirId);
  diff.st.cloudSubtreeFiles.run(dirId);
}

export interface CloseCounts extends MarkCounts {
  suspect: number;
  incomplete: number;
  collisions: number;
}

export type CloseOutcome = "closed" | "skip" | "incomplete";

/**
 * Decides what a `DirDone {complete: true}` of `dirId` by `job` may do and, when it closes the
 * listing, marks, resolves and recomputes. `"skip"`: nothing is recorded but `lastSeen`;
 * `"closed"`: the caller records `mtimeNs` and `listedGen`.
 */
export function closeListing(
  diff: DiffConn,
  fts: FtsStatements,
  dirId: number,
  entries: number,
  job: string,
  at: number,
  counts: CloseCounts,
): CloseOutcome {
  const l = endListing(diff, job, dirId);
  if (diff.offline) {
    counts.incomplete++;
    return "skip";
  }
  const children = (diff.st.hasChildren.get(dirId, dirId) as { x: number }).x === 1;
  if (entries === 0 && children) {
    const e = diff.st.dirEmpty.get(dirId) as { emptySince: number | null; emptyJob: string | null };
    const confirmed = e.emptySince !== null && e.emptyJob !== job && at - e.emptySince >= EMPTY_CONFIRM_MS;
    if (!confirmed) {
      diff.st.suspect.run(at, job, at, dirId);
      counts.suspect++;
      return "skip";
    }
  } else if (entries > 0 && (l === undefined || l.seen < entries)) {
    diff.st.seenOnly.run(at, dirId);
    counts.incomplete++;
    return "skip";
  }
  const start = l?.start ?? at;
  const files = diff.st.goneFiles.all(dirId, start) as Array<{ id: number; fileId: Uint8Array | null }>;
  if (files.length > 0) diff.st.markFiles.run(at, dirId, start);
  counts.missing += files.length;
  const dirs = diff.st.goneDirs.all(dirId, start) as Array<{ id: number }>;
  if (dirs.length > 0) diff.st.markDirs.run(dirId, start);
  for (const d of dirs) markSubtree(diff, fts, d.id, at, counts);
  diff.st.clearSeenRenames.run(dirId, start);
  counts.renamed += resolveRenames(diff, fts, files);
  counts.collisions += recomputeCollisions(diff.coll, dirId);
  return "closed";
}

/** `ROOT_EMPTY_SUSPECT`: an empty, complete root listing while the catalog holds files. */
export function checkRootEmpty(diff: DiffConn, dones: ReadonlyArray<{ dir: { components: string[] }; complete: boolean; entries: number }>): void {
  if (diff.offline) return;
  const empty = dones.some((d) => d.dir.components.length === 0 && d.complete && d.entries === 0);
  if (empty && (diff.st.anyFile.get() as { x: number }).x === 1) {
    throw new CatalogFailure("ROOT_EMPTY_SUSPECT", "the library's root listed empty while its catalog holds files", {});
  }
}

/** Recomputes `hardlink` for every touched file id: live rows sharing `dev:fileId`. */
export function settleHardlinks(diff: DiffConn): void {
  for (const hex of diff.touched) {
    const rows = diff.st.linkGroup.all(Buffer.from(hex, "hex")) as Array<{ id: number; dev: string | null; state: string; hardlink: number }>;
    const live = new Map<string, number>();
    for (const r of rows) if (r.state !== "missing") live.set(r.dev ?? "", (live.get(r.dev ?? "") ?? 0) + 1);
    for (const r of rows) {
      const want = r.state !== "missing" && (live.get(r.dev ?? "") ?? 0) > 1 ? 1 : 0;
      if (want !== r.hardlink) diff.st.setLink.run(want, r.id);
    }
  }
  diff.touched.clear();
}
