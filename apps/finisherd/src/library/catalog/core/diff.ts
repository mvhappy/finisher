// The catalog diff (plan 5D Task 7b, Decisions 11, 13, 16, 18, 22, 25): what a closed listing
// means for the rows it did not see, renames by `dev:fileId`, moves by fingerprint, hard
// links and collisions. Runs in the indexer thread only, inside `apply.ts`'s transactions.
//
// **Listings.** A row is seen by a listing when its entry is applied; each `(job, dir)`
// listing counts its entries and has a `start`: the earlier of its first entry's transaction
// time and the job attempt's `startedAt` (`IndexerClient.attemptStart`, taken before the
// attempt reads anything). A listing is read before it is applied, often long before (a full
// scan's batch waits behind a poll's), so `start` must be a time no later than the read. Its
// `DirDone {complete: true}` then marks **missing** the directory's rows with
// `lastSeen < start` (not seen by this listing, nor by any other applied since it began: a row
// created after this listing's read and seen by a later poll keeps its place), and every
// subdirectory so marked with its whole subtree. Nothing is marked when:
// - the listing is incomplete, or the library is offline (Decision 18);
// - it saw fewer entries than its `DirDone` counts (its first entries were applied before a
//   restart or a rolled-back transaction: the listing state is lost);
// - it is empty while the directory holds rows (`DIR_EMPTY_SUSPECT`, `emptySince` = the
//   suspect listing's apply time, an upper bound of its read): only another job's empty
//   listing whose `start` is at least 60 s later confirms it, and any entry applied under the
//   directory in the meantime clears the suspicion. A suspect listing records no `mtimeNs` and
//   no `listedGen`, so the next poll and a resumed scan list it again.
// A closed listing records `mtimeNs`, `listedGen` and `listedAt` only when it started no
// earlier than the listing recorded last: an older listing applied late never overwrites them.
// An empty **root** listing while the catalog holds files fails the whole batch file before
// anything is written (`ROOT_EMPTY_SUSPECT`): the service puts the library offline.
//
// **Nested mounts** (Decision 17): a mount point closed `ROOT_UNMOUNTED` or
// `ROOT_VOLUME_CHANGED` makes itself and its subtree `offline`; no listing below it marks
// anything, and a vanished offline directory is not marked either, until the mount point's
// own complete listing brings it back.
//
// **Excluded** (Decision 16): a known directory or file a listing reports excluded (a glob
// changed, an automount) goes missing with code `EXCLUDED`, the directory with its whole
// subtree, once that listing closes; such rows are never a move's or a rename's old row (their
// files still exist).
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
//
// Subtree changes run in steps (`steps.ts`): one transaction, event-loop turns between steps.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import { type FtsStatements, ftsRemove, ftsReplace } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";
import { type CollisionStatements, prepareCollisions, recomputeCollisions } from "./collisions.ts";
import { type Budget, type Work, chunks, room } from "./steps.ts";
import { type FileIdSink, noteFile } from "./file-events.ts";

/** A second empty listing confirms a drop to zero only when it started this long after the first was applied. */
export const EMPTY_CONFIRM_MS = 60_000;
/** Listing state is kept for this many jobs per catalog (the oldest is dropped first). */
const MAX_JOBS = 64;
/** Rows per statement of a subtree change (also bounded by the step). */
const STEP_ROWS = 2000;
/** The missing code of rows an exclusion took out of the catalog. */
export const EXCLUDED = "EXCLUDED";

export interface Listing {
  readonly start: number;
  seen: number;
  /** Directory rows it listed as excluded: their subtrees go missing (`EXCLUDED`) once it closes. */
  readonly excludedDirs: number[];
  /** Live file rows it listed as excluded: missing (`EXCLUDED`) once it closes. */
  readonly excludedFiles: number[];
}

/** Who applies and when. */
export interface DiffCtx {
  readonly job: string;
  readonly gen: number;
  /** The transaction time (never backwards per catalog). */
  readonly at: number;
  /** The job attempt's start, before any of its reads. */
  readonly startedAt: number;
}

type S = StatementSync;
export interface DiffStatements {
  readonly candidates: S;
  readonly takeOver: S;
  readonly fileFull: S;
  readonly deleteFile: S;
  readonly merge: S;
  readonly dirKey: S;
  readonly pointing: S;
  readonly clearRenames: S;
  readonly clearSeenRenames: S;
  readonly goneFiles: S;
  readonly goneDirs: S;
  readonly markFileIds: S;
  readonly markDirIds: S;
  readonly excludedLive: S;
  readonly subtree: S;
  readonly liveFilesIn: S;
  readonly cloudDirIds: S;
  readonly cloudFilesIn: S;
  readonly cloudFileIds: S;
  readonly offlineDirIds: S;
  readonly offlineFilesIn: S;
  readonly offlineFileIds: S;
  readonly underOffline: S;
  readonly hasChildren: S;
  readonly dirEmpty: S;
  readonly suspect: S;
  readonly unsuspect: S;
  readonly dirState: S;
  readonly seenOnly: S;
  readonly linkGroup: S;
  readonly setLink: S;
  readonly anyFile: S;
  readonly idsPresent: S;
}

export interface DiffConn {
  readonly st: DiffStatements;
  readonly coll: CollisionStatements;
  /** job → dirId → the open listing. */
  readonly listings: Map<string, Map<number, Listing>>;
  /** File ids (hex) whose `hardlink` flags need recomputing at the end of the transaction. */
  readonly touched: Set<string>;
  /** Directories whose empty suspicion this transaction's entries already cleared. */
  readonly unsuspected: Set<number>;
  /**
   * File ids (hex) of the current transaction's entries that some row holds (or held at its
   * start): an id outside it needs no rename lookup. `undefined` outside `applyRows`.
   */
  ids: Set<string> | undefined;
  /** The rows the current batch file changed (`Libraries.onFile`); `undefined` outside one. */
  events: FileIdSink | undefined;
  offline: boolean;
}

const SUBTREE = "WITH RECURSIVE sub (id) AS (SELECT ? UNION ALL SELECT d.id FROM dirs d JOIN sub ON d.parent = sub.id)";
const IDS = "IN (SELECT value FROM json_each(?))";
const NOT_EXCLUDED = `coalesce(code, '') != '${EXCLUDED}'`;

export function prepareDiff(db: DatabaseSync): DiffConn {
  const st: DiffStatements = {
    candidates: db.prepare(
      "SELECT id, dev, size, CAST(mtimeNs AS TEXT) AS mtimeNs, state, code, missingSince FROM files WHERE fileId = ? ORDER BY id",
    ),
    takeOver: db.prepare(
      "UPDATE files SET dirId = ?, name = ?, nameNfc = ?, nameLossy = ?, relKey = ?, ext = ?, fileId = ?, dev = ?, placeholder = ?, " +
        "state = ?, code = NULL, lastSeen = ?, missingSince = NULL, missingGen = NULL, renameOf = NULL, " +
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
        "probeVersion = ?, durationMs = ?, width = ?, height = ?, renameOf = NULL, missingSince = NULL, missingGen = NULL WHERE id = ?",
    ),
    dirKey: db.prepare("SELECT relKey FROM dirs WHERE id = ?"),
    pointing: db.prepare("SELECT id FROM files WHERE renameOf = ? AND state != 'missing' ORDER BY id"),
    clearRenames: db.prepare("UPDATE files SET renameOf = NULL WHERE renameOf = ?"),
    clearSeenRenames: db.prepare(
      "UPDATE files SET renameOf = NULL WHERE renameOf IN (SELECT id FROM files WHERE dirId = ? AND state != 'missing' AND lastSeen >= ?)",
    ),
    // Selected first, then updated by id only when there is something to mark: `RETURNING`
    // costs ~100 µs a statement, and almost every listing marks nothing.
    goneFiles: db.prepare("SELECT id, fileId FROM files WHERE dirId = ? AND state != 'missing' AND lastSeen < ? ORDER BY id LIMIT ?"),
    goneDirs: db.prepare("SELECT id FROM dirs WHERE parent = ? AND state NOT IN ('missing', 'offline') AND lastSeen < ? ORDER BY id LIMIT ?"),
    markFileIds: db.prepare(
      `UPDATE files SET state = 'missing', missingSince = ?, missingGen = ?, code = coalesce(?, code), renameOf = NULL, hardlink = 0 WHERE id ${IDS}`,
    ),
    markDirIds: db.prepare(`UPDATE dirs SET state = 'missing' WHERE id ${IDS} AND state != 'missing'`),
    excludedLive: db.prepare(`SELECT id, fileId FROM files WHERE id ${IDS} AND state != 'missing' AND lastSeen < ? ORDER BY id`),
    subtree: db.prepare(`${SUBTREE} SELECT id FROM sub`),
    liveFilesIn: db.prepare("SELECT id, fileId FROM files WHERE dirId = ? AND state != 'missing' ORDER BY id LIMIT ?"),
    cloudDirIds: db.prepare(`UPDATE dirs SET state = 'cloudOnly' WHERE id ${IDS} AND state NOT IN ('missing', 'excluded', 'cloudOnly')`),
    cloudFilesIn: db.prepare("SELECT id FROM files WHERE dirId = ? AND state NOT IN ('missing', 'cloudOnly') ORDER BY id LIMIT ?"),
    cloudFileIds: db.prepare(`UPDATE files SET state = 'cloudOnly' WHERE id ${IDS}`),
    offlineDirIds: db.prepare(`UPDATE dirs SET state = 'offline' WHERE id ${IDS} AND state NOT IN ('missing', 'excluded', 'offline')`),
    offlineFilesIn: db.prepare("SELECT id FROM files WHERE dirId = ? AND state NOT IN ('missing', 'offline') ORDER BY id LIMIT ?"),
    offlineFileIds: db.prepare(`UPDATE files SET state = 'offline' WHERE id ${IDS}`),
    underOffline: db.prepare(
      "WITH RECURSIVE up (id) AS (SELECT parent FROM dirs WHERE id = ? UNION ALL SELECT d.parent FROM dirs d JOIN up ON d.id = up.id) " +
        "SELECT EXISTS (SELECT 1 FROM dirs WHERE id IN (SELECT id FROM up) AND state = 'offline') AS x",
    ),
    hasChildren: db.prepare(
      "SELECT EXISTS (SELECT 1 FROM files WHERE dirId = ? AND state != 'missing') OR " +
        "EXISTS (SELECT 1 FROM dirs WHERE parent = ? AND state != 'missing') AS x",
    ),
    dirEmpty: db.prepare("SELECT emptySince, emptyJob, listedAt FROM dirs WHERE id = ?"),
    suspect: db.prepare(
      "UPDATE dirs SET emptySince = coalesce(emptySince, ?), emptyJob = coalesce(emptyJob, ?), code = 'DIR_EMPTY_SUSPECT', lastSeen = ? WHERE id = ?",
    ),
    unsuspect: db.prepare(
      "UPDATE dirs SET emptySince = NULL, emptyJob = NULL, code = CASE WHEN code = 'DIR_EMPTY_SUSPECT' THEN NULL ELSE code END " +
        "WHERE id = ? AND emptySince IS NOT NULL",
    ),
    dirState: db.prepare("SELECT state FROM dirs WHERE id = ?"),
    seenOnly: db.prepare("UPDATE dirs SET lastSeen = ? WHERE id = ?"),
    linkGroup: db.prepare("SELECT id, dev, state, hardlink FROM files WHERE fileId = ?"),
    setLink: db.prepare("UPDATE files SET hardlink = ? WHERE id = ?"),
    anyFile: db.prepare("SELECT EXISTS (SELECT 1 FROM files WHERE state != 'missing') AS x"),
    idsPresent: db.prepare("SELECT DISTINCT hex(fileId) AS id FROM files WHERE fileId IN (SELECT unhex(value) FROM json_each(?))"),
  };
  return {
    st, coll: prepareCollisions(db), listings: new Map(), touched: new Set(), unsuspected: new Set(), ids: undefined, events: undefined, offline: false,
  };
}

/** A file id as stored: `null` for none or an all-zero id (servers that answer 0). */
export function usableId(b64: string | null | undefined): Uint8Array | null {
  if (b64 == null) return null;
  const id = Buffer.from(b64, "base64");
  return id.length === 0 || id.every((b) => b === 0) ? null : id;
}

const hexOf = (id: Uint8Array): string => Buffer.from(id.buffer, id.byteOffset, id.byteLength).toString("hex");

export function touch(diff: DiffConn, id: Uint8Array | null): void {
  if (id !== null) diff.touched.add(hexOf(id));
}

/** The open listing of `dirId` by the job, created by its first entry. */
export function listingOf(diff: DiffConn, ctx: DiffCtx, dirId: number): Listing {
  let jobs = diff.listings.get(ctx.job);
  if (jobs === undefined) {
    if (diff.listings.size >= MAX_JOBS) diff.listings.delete(diff.listings.keys().next().value as string);
    jobs = new Map();
    diff.listings.set(ctx.job, jobs);
  }
  let l = jobs.get(dirId);
  if (l === undefined) {
    l = { start: Math.min(ctx.at, ctx.startedAt), seen: 0, excludedDirs: [], excludedFiles: [] };
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

/** An entry under `dirId` is evidence it is not empty: clears its `DIR_EMPTY_SUSPECT` (once per transaction). */
export function unsuspect(diff: DiffConn, dirId: number): void {
  if (diff.unsuspected.has(dirId)) return;
  diff.st.unsuspect.run(dirId);
  diff.unsuspected.add(dirId);
}

export interface Candidate {
  readonly id: number;
  readonly missing: boolean;
}

/** Loads `diff.ids` for a transaction's file ids (hex), one query instead of one per entry. */
export function prefetchIds(diff: DiffConn, hexIds: ReadonlyArray<string>): void {
  const rows = hexIds.length === 0 ? [] : (diff.st.idsPresent.all(JSON.stringify(hexIds)) as Array<{ id: string }>);
  diff.ids = new Set(rows.map((r) => r.id.toLowerCase()));
}

/** Records that a row now holds `fileId` (for `diff.ids`). */
export function holds(diff: DiffConn, fileId: Uint8Array | null): void {
  if (fileId !== null) diff.ids?.add(hexOf(fileId));
}

/**
 * The rename hint for a new entry: the missing row it takes over (latest `missingSince`, then
 * lowest id; never an `EXCLUDED` one), else the live row it may be a rename of (lowest id).
 */
export function renameCandidate(diff: DiffConn, fileId: Uint8Array | null, dev: string | null, size: number, mtime: bigint): Candidate | undefined {
  if (fileId === null) return undefined;
  if (diff.ids !== undefined && !diff.ids.has(hexOf(fileId))) return undefined;
  type Row = { id: number; dev: string | null; size: number; mtimeNs: string; state: string; code: string | null; missingSince: number | null };
  const all = diff.st.candidates.all(fileId) as Row[];
  // Another row with this id may make the new one a hard link, hint or not.
  if (all.length > 0) touch(diff, fileId);
  const m = mtime.toString();
  const rows = all.filter((r) => r.dev === dev && r.size === size && r.mtimeNs === m);
  let best: { id: number; since: number } | undefined;
  for (const r of rows) {
    if (r.state !== "missing" || r.code === EXCLUDED) continue;
    const since = r.missingSince ?? 0;
    if (best === undefined || since > best.since) best = { id: r.id, since };
  }
  if (best !== undefined) return { id: best.id, missing: true };
  const live = rows.find((r) => r.state !== "missing");
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

type Gone = { id: number; fileId: Uint8Array | null };

/** Resolves pending renames of rows just marked missing; answers how many merged. */
function* resolveRenames(diff: DiffConn, fts: FtsStatements, missing: ReadonlyArray<Gone>, b: Budget): Work<number> {
  let n = 0;
  for (const m of missing) {
    yield* room(b, 1);
    touch(diff, m.fileId);
    const next = diff.st.pointing.get(m.id) as { id: number } | undefined;
    if (next !== undefined && mergeInto(diff, fts, m.id, next.id, (diff.st.fileFull.get(m.id) as Full).fp !== null)) {
      n++;
      noteFile(diff.events, "renamed", m.id);
    }
  }
  return n;
}

/** Marks `ids` missing at `ctx.at` (with `code`, if any), in steps. */
function* markFiles(diff: DiffConn, ids: ReadonlyArray<Gone>, ctx: DiffCtx, code: string | null, b: Budget): Work {
  for (const part of chunks(ids, STEP_ROWS)) {
    yield* room(b, part.length);
    diff.st.markFileIds.run(ctx.at, ctx.gen, code, JSON.stringify(part.map((r) => r.id)));
    for (const r of part) noteFile(diff.events, "missing", r.id);
  }
}

const limit = (b: Budget): number => Math.min(b.step, STEP_ROWS);

/** Every live file row directly in `dirId`, read a step at a time and handed to `f`. */
function* eachLive(diff: DiffConn, dirId: number, b: Budget, f: (rows: Gone[]) => Work): Work {
  for (;;) {
    const rows = diff.st.liveFilesIn.all(dirId, limit(b)) as Gone[];
    if (rows.length === 0) return;
    yield* f(rows);
    if (rows.length < limit(b)) return;
  }
}

export interface MarkCounts {
  missing: number;
  renamed: number;
}

/**
 * Marks `dirId`'s subtree missing (the directories below it, and every file in it and
 * below), in steps; answers the files. The caller resolves their renames once every mark of
 * the listing is done, so the outcome never depends on which vanished sibling came first.
 */
export function* markSubtree(diff: DiffConn, dirId: number, ctx: DiffCtx, code: string | null, counts: MarkCounts, b: Budget): Work<Gone[]> {
  const sub = (diff.st.subtree.all(dirId) as Array<{ id: number }>).map((r) => r.id);
  for (const part of chunks(sub.filter((id) => id !== dirId), STEP_ROWS)) {
    yield* room(b, part.length);
    diff.st.markDirIds.run(JSON.stringify(part));
  }
  const files: Gone[] = [];
  for (const d of sub) {
    yield* room(b, 1);
    yield* eachLive(diff, d, b, function* (rows) {
      yield* markFiles(diff, rows, ctx, code, b);
      files.push(...rows);
    });
  }
  counts.missing += files.length;
  for (const f of files) touch(diff, f.fileId);
  return files;
}

/** Sets a state on `dirId`'s subtree (the directories below it, the files in it and below), in steps. */
function* stateSubtree(diff: DiffConn, dirId: number, b: Budget, dirIds: S, filesIn: S, fileIds: S): Work {
  const sub = (diff.st.subtree.all(dirId) as Array<{ id: number }>).map((r) => r.id);
  for (const part of chunks(sub.filter((id) => id !== dirId), STEP_ROWS)) {
    yield* room(b, part.length);
    dirIds.run(JSON.stringify(part));
  }
  for (const d of sub) {
    yield* room(b, 1);
    for (;;) {
      const rows = filesIn.all(d, limit(b)) as Array<{ id: number }>;
      if (rows.length === 0) break;
      yield* room(b, rows.length);
      fileIds.run(JSON.stringify(rows.map((r) => r.id)));
      for (const r of rows) noteFile(diff.events, "changed", r.id);
      if (rows.length < limit(b)) break;
    }
  }
}

/** A placeholder directory's subtree becomes `cloudOnly`, never missing (Decision 13), in steps. */
export function cloudSubtree(diff: DiffConn, dirId: number, b: Budget): Work {
  return stateSubtree(diff, dirId, b, diff.st.cloudDirIds, diff.st.cloudFilesIn, diff.st.cloudFileIds);
}

/** An unmounted or replaced nested mount's subtree becomes `offline`, never missing (Decision 17), in steps. */
export function offlineSubtree(diff: DiffConn, dirId: number, b: Budget): Work {
  return stateSubtree(diff, dirId, b, diff.st.offlineDirIds, diff.st.offlineFilesIn, diff.st.offlineFileIds);
}

export interface CloseCounts extends MarkCounts {
  suspect: number;
  incomplete: number;
  collisions: number;
  excluded: number;
}

export type CloseOutcome =
  | { readonly kind: "skip" | "incomplete" }
  /** `fresh`: it started no earlier than the listing last recorded, so it records `mtimeNs` and `listedGen`. */
  | { readonly kind: "closed"; readonly start: number; readonly fresh: boolean };

/**
 * Decides what a `DirDone {complete: true}` of `dirId` may do and, when it closes the
 * listing, marks, resolves and recomputes. `"skip"`: nothing is recorded but `lastSeen`.
 */
export function* closeListing(
  diff: DiffConn,
  fts: FtsStatements,
  dirId: number,
  entries: number,
  ctx: DiffCtx,
  counts: CloseCounts,
  b: Budget,
): Work<CloseOutcome> {
  const l = endListing(diff, ctx.job, dirId);
  if (diff.offline) {
    counts.incomplete++;
    return { kind: "skip" };
  }
  if ((diff.st.underOffline.get(dirId) as { x: number }).x === 1) {
    diff.st.seenOnly.run(ctx.at, dirId);
    counts.incomplete++;
    return { kind: "skip" };
  }
  const start = l?.start ?? Math.min(ctx.at, ctx.startedAt);
  const e = diff.st.dirEmpty.get(dirId) as { emptySince: number | null; emptyJob: string | null; listedAt: number | null };
  const children = (diff.st.hasChildren.get(dirId, dirId) as { x: number }).x === 1;
  if (entries === 0 && children) {
    const confirmed = e.emptySince !== null && e.emptyJob !== ctx.job && start - e.emptySince >= EMPTY_CONFIRM_MS;
    if (!confirmed) {
      diff.st.suspect.run(ctx.at, ctx.job, ctx.at, dirId);
      diff.unsuspected.delete(dirId);
      counts.suspect++;
      return { kind: "skip" };
    }
  } else if (entries > 0 && (l === undefined || l.seen < entries)) {
    diff.st.seenOnly.run(ctx.at, dirId);
    counts.incomplete++;
    return { kind: "skip" };
  }
  if (l !== undefined && l.excludedFiles.length > 0) {
    const rows = diff.st.excludedLive.all(JSON.stringify(l.excludedFiles), start) as Gone[];
    yield* markFiles(diff, rows, ctx, EXCLUDED, b);
    for (const r of rows) touch(diff, r.fileId);
    counts.missing += rows.length;
    counts.excluded += rows.length;
  }
  for (const d of l?.excludedDirs ?? []) {
    if ((diff.st.dirState.get(d) as { state: string } | undefined)?.state !== "excluded") continue;
    const before = counts.missing;
    yield* markSubtree(diff, d, ctx, EXCLUDED, counts, b);
    counts.excluded += counts.missing - before;
  }
  const files: Gone[] = [];
  for (;;) {
    const rows = diff.st.goneFiles.all(dirId, start, limit(b)) as Gone[];
    if (rows.length === 0) break;
    yield* markFiles(diff, rows, ctx, null, b);
    files.push(...rows);
    if (rows.length < limit(b)) break;
  }
  counts.missing += files.length;
  const below: Gone[] = [];
  for (;;) {
    const dirs = (diff.st.goneDirs.all(dirId, start, limit(b)) as Array<{ id: number }>).map((r) => r.id);
    if (dirs.length === 0) break;
    yield* room(b, dirs.length);
    diff.st.markDirIds.run(JSON.stringify(dirs));
    for (const d of dirs) for (const f of yield* markSubtree(diff, d, ctx, null, counts, b)) below.push(f);
    if (dirs.length < limit(b)) break;
  }
  diff.st.clearSeenRenames.run(dirId, start);
  counts.renamed += yield* resolveRenames(diff, fts, [...files, ...below], b);
  counts.collisions += recomputeCollisions(diff.coll, dirId);
  return { kind: "closed", start, fresh: e.listedAt === null || start >= e.listedAt };
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
export function* settleHardlinks(diff: DiffConn, b: Budget): Work {
  for (const hex of [...diff.touched]) {
    yield* room(b, 1);
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
