// Search pages across libraries (plan 5D Decision 43), in the reader thread.
// - A query **with terms** ranks by bm25, which depends on corpus statistics that change with
//   every write, so a keyset on scores is not stable. The first page ranks up to 2,000 hits
//   across the requested libraries (merged by score, ties by library id and row id) and keeps
//   them as a **snapshot** under an opaque cursor; later pages slice it, so pages never repeat
//   or skip while catalogs change. Past 2,000 hits the answer says `truncated`. Snapshots live
//   5 minutes after their last use, at most 64 at a time (least recently used out first).
// - A query **without terms** lists by `lastChange` descending with a score-free keyset on
//   `(lastChange, library id, row id)`: nothing is kept, and corpus changes cannot move it.
// A cursor is bound to the arguments of the query that made it (a mismatch is
// `CURSOR_INVALID`), a dropped snapshot is `CURSOR_EXPIRED`.
import { createHash, randomBytes } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { CatalogFailure } from "../protocol.ts";
import { type After, DisplayPaths, type Hit, type HitRow, type SearchArgs, libraryOffline, parseSearchArgs, searchRows, toHit } from "./search.ts";

export const SNAPSHOT_HITS = 2000;
export const MAX_PAGE = 200;
export const MAX_LIBRARIES = 256;

export interface SnapshotOptions {
  /** Idle time after which a snapshot is gone (default 5 minutes). */
  readonly ttlMs?: number;
  /** Live snapshots (default 64). */
  readonly max?: number;
  readonly now?: () => number;
  readonly token?: () => string;
}

export interface SearchRequest {
  readonly libraries: ReadonlyArray<string>;
  readonly args: unknown;
  readonly cursor?: string;
  readonly limit: number;
}

export interface SearchPage {
  readonly hits: ReadonlyArray<Hit>;
  readonly next?: string;
  readonly truncated: boolean;
}

interface Snapshot {
  readonly sig: string;
  readonly hits: ReadonlyArray<Hit>;
  readonly truncated: boolean;
  touched: number;
}

const invalid = (at: string): CatalogFailure => new CatalogFailure("CURSOR_INVALID", "the search cursor does not belong to this query", { at });
const expired = (): CatalogFailure => new CatalogFailure("CURSOR_EXPIRED", "the search cursor has expired; search again", {});

/** The order of merged hits with terms: score, then library id, then row id. */
const byScore = (a: Hit, b: Hit): number => a.score - b.score || (a.library < b.library ? -1 : a.library > b.library ? 1 : 0) || a.fileId - b.fileId;
/** The order without terms: newest first, then library id and row id. */
const byTime = (a: Hit, b: Hit): number => b.lastChange - a.lastChange || (a.library < b.library ? -1 : a.library > b.library ? 1 : 0) || a.fileId - b.fileId;

export class Snapshots {
  readonly #conns: ReadonlyMap<string, DatabaseSync>;
  readonly #ttl: number;
  readonly #max: number;
  readonly #now: () => number;
  readonly #token: () => string;
  /** Insertion order is recency: a use deletes and re-inserts. */
  readonly #snaps = new Map<string, Snapshot>();

  constructor(conns: ReadonlyMap<string, DatabaseSync>, o: SnapshotOptions = {}) {
    this.#conns = conns;
    this.#ttl = o.ttlMs ?? 5 * 60_000;
    this.#max = o.max ?? 64;
    this.#now = o.now ?? Date.now;
    this.#token = o.token ?? (() => randomBytes(15).toString("base64url"));
  }

  /** Live snapshots, expired ones dropped first. */
  get size(): number {
    this.#sweep();
    return this.#snaps.size;
  }

  search(req: SearchRequest): SearchPage {
    if (!Number.isSafeInteger(req.limit) || req.limit < 1 || req.limit > MAX_PAGE) {
      throw new CatalogFailure("CATALOG_ARGS_INVALID", "catalog search: invalid arguments", { at: "limit" });
    }
    const libs = [...req.libraries];
    if (libs.length === 0 || libs.length > MAX_LIBRARIES || new Set(libs).size !== libs.length || !libs.every((l) => typeof l === "string")) {
      throw new CatalogFailure("CATALOG_ARGS_INVALID", "catalog search: invalid arguments", { at: "libraries" });
    }
    libs.sort();
    const args = parseSearchArgs(req.args);
    const dbs = libs.map((id) => {
      const db = this.#conns.get(id);
      if (db === undefined) throw new CatalogFailure("CATALOG_NOT_OPEN", "the library's catalog is not open", { library: id });
      return { id, db };
    });
    const sig = signature(args, libs);
    return args.match === undefined ? this.#keyset(dbs, args, sig, req) : this.#ranked(dbs, args, sig, req);
  }

  #sweep(): void {
    const now = this.#now();
    for (const [token, s] of this.#snaps) if (now - s.touched >= this.#ttl) this.#snaps.delete(token);
  }

  #ranked(dbs: Dbs, args: SearchArgs, sig: string, req: SearchRequest): SearchPage {
    this.#sweep();
    if (req.cursor !== undefined) {
      const m = /^s1\.([A-Za-z0-9_-]{8,64})\.(\d{1,6})$/.exec(req.cursor);
      if (m === null) throw invalid("cursor");
      const snap = this.#snaps.get(m[1]!);
      if (snap === undefined) throw expired();
      if (snap.sig !== sig) throw invalid("args");
      const offset = Number(m[2]);
      if (offset > snap.hits.length) throw invalid("cursor");
      this.#snaps.delete(m[1]!);
      snap.touched = this.#now();
      this.#snaps.set(m[1]!, snap);
      return this.#slice(m[1]!, snap, offset, req.limit);
    }
    const merged: Array<{ library: string; row: HitRow; db: DatabaseSync }> = [];
    for (const { id, db } of dbs) for (const row of searchRows(db, args, SNAPSHOT_HITS + 1)) merged.push({ library: id, row, db });
    const truncated = merged.length > SNAPSHOT_HITS;
    const order = (x: { library: string; row: HitRow }, y: { library: string; row: HitRow }) =>
      x.row.score - y.row.score || (x.library < y.library ? -1 : x.library > y.library ? 1 : 0) || x.row.id - y.row.id;
    const kept = merged.sort(order).slice(0, SNAPSHOT_HITS);
    const hits = materialise(kept, dbs);
    if (hits.length <= req.limit) return { hits, truncated };
    const token = this.#token();
    this.#snaps.set(token, { sig, hits, truncated, touched: this.#now() });
    while (this.#snaps.size > this.#max) this.#snaps.delete(this.#snaps.keys().next().value!);
    return this.#slice(token, this.#snaps.get(token)!, 0, req.limit);
  }

  #slice(token: string, snap: Snapshot, offset: number, limit: number): SearchPage {
    const end = offset + limit;
    return {
      hits: snap.hits.slice(offset, end),
      truncated: snap.truncated,
      ...(end < snap.hits.length ? { next: `s1.${token}.${end}` } : {}),
    };
  }

  #keyset(dbs: Dbs, args: SearchArgs, sig: string, req: SearchRequest): SearchPage {
    let pos: { t: number; lib: string; id: number } | undefined;
    if (req.cursor !== undefined) {
      if (!req.cursor.startsWith("k1.")) throw invalid("cursor");
      try {
        const v = JSON.parse(Buffer.from(req.cursor.slice(3), "base64url").toString("utf8")) as Record<string, unknown>;
        if (v.sig !== sig || !Number.isSafeInteger(v.t) || typeof v.lib !== "string" || !Number.isSafeInteger(v.id)) throw invalid("args");
        pos = { t: v.t as number, lib: v.lib, id: v.id as number };
      } catch (e) {
        throw e instanceof CatalogFailure ? e : invalid("cursor");
      }
    }
    const merged: Array<{ library: string; row: HitRow; db: DatabaseSync }> = [];
    for (const { id, db } of dbs) {
      const after: After | undefined =
        pos === undefined ? undefined : { lastChange: pos.t, id: pos.id, cmp: id < pos.lib ? -1 : id > pos.lib ? 1 : 0 };
      for (const row of searchRows(db, args, req.limit + 1, after)) merged.push({ library: id, row, db });
    }
    merged.sort((x, y) => y.row.lastChange - x.row.lastChange || (x.library < y.library ? -1 : x.library > y.library ? 1 : 0) || x.row.id - y.row.id);
    const page = merged.slice(0, req.limit);
    const hits = materialise(page, dbs);
    const last = page.at(-1);
    const next =
      merged.length > req.limit && last !== undefined
        ? `k1.${Buffer.from(JSON.stringify({ sig, t: last.row.lastChange, lib: last.library, id: last.row.id })).toString("base64url")}`
        : undefined;
    return { hits, truncated: false, ...(next === undefined ? {} : { next }) };
  }
}

type Dbs = ReadonlyArray<{ id: string; db: DatabaseSync }>;

/** Fills in display paths and offline flags of the chosen rows, one library at a time. */
function materialise(rows: ReadonlyArray<{ library: string; row: HitRow; db: DatabaseSync }>, dbs: Dbs): Hit[] {
  const paths = new Map<string, DisplayPaths>();
  const offline = new Map<string, boolean>();
  for (const { id, db } of dbs) {
    if (!rows.some((r) => r.library === id)) continue;
    paths.set(id, new DisplayPaths(db));
    offline.set(id, libraryOffline(db));
  }
  return rows.map(({ library, row }) => toHit(library, row, paths.get(library)!.file(row.dirId, row.nameNfc), offline.get(library)!));
}

function signature(a: SearchArgs, libs: ReadonlyArray<string>): string {
  const canon = JSON.stringify([libs, a.match ?? null, a.kind ?? null, a.state ?? null, a.since ?? null, a.durationMs ?? null, a.width ?? null, a.height ?? null, a.size ?? null]);
  return createHash("sha256").update(canon).digest("base64url").slice(0, 22);
}
