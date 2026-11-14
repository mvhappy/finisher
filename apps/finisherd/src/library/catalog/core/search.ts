// Name search over one catalog (plan 5D Task 12, Decision 43): the reader-thread half that
// `snapshot.ts` merges across libraries. Argument checking, the filters, the two orders (bm25
// for a query with terms, `lastChange` for one without) and the lean hit rows that a snapshot
// keeps. Everything here runs on the read-only connection, in the `library-reader` thread.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import { matchExpression, queryTerms } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";

/** At most this many terms, and this many characters, in a query: more is `SEARCH_QUERY_INVALID`. */
export const MAX_TERMS = 16;
export const MAX_QUERY_CHARS = 512;

export interface Range {
  readonly min?: number;
  readonly max?: number;
}

/** The checked search arguments, as the snapshot and the keyset compare them. */
export interface SearchArgs {
  /** The FTS5 expression of prefix terms; absent for a query without terms. */
  readonly match?: string;
  readonly kind?: string;
  readonly state?: string;
  readonly since?: number;
  readonly durationMs?: Range;
  readonly width?: Range;
  readonly height?: Range;
  readonly size?: Range;
}

/** One hit as it is kept in a snapshot and crosses the port; absent fields are `null` in the row. */
export interface Hit {
  readonly library: string;
  readonly fileId: number;
  readonly relKey: string;
  /** The NFC path from the root in the names' true case, `/`-separated. */
  readonly display: string;
  readonly kind?: string;
  readonly size: number;
  readonly durationMs?: number;
  readonly width?: number;
  readonly height?: number;
  readonly fp?: string;
  readonly state: string;
  readonly collision: boolean;
  /** bm25 (lower is better); 0 for a query without terms. */
  readonly score: number;
  /** The library is offline (`meta.offline`) or the file's state is `offline`. */
  readonly offline: boolean;
  readonly lastChange: number;
}

/** A row before its path and the library's offline flag are known. */
export interface HitRow {
  readonly id: number;
  readonly dirId: number;
  readonly nameNfc: string;
  readonly relKey: string;
  readonly kind: string | null;
  readonly size: number;
  readonly durationMs: number | null;
  readonly width: number | null;
  readonly height: number | null;
  readonly fp: string | null;
  readonly state: string;
  readonly collision: number;
  readonly lastChange: number;
  readonly score: number;
}

const bad = (at: string): CatalogFailure => new CatalogFailure("CATALOG_ARGS_INVALID", "catalog search: invalid arguments", { at });
const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);

function range(v: unknown, at: string): Range | undefined {
  if (v === undefined) return undefined;
  if (!isObj(v) || (v.min !== undefined && !isInt(v.min)) || (v.max !== undefined && !isInt(v.max))) throw bad(at);
  return { ...(v.min === undefined ? {} : { min: v.min as number }), ...(v.max === undefined ? {} : { max: v.max as number }) };
}

/** Checks and normalises the arguments (the query already tokenised into `match`). */
export function parseSearchArgs(raw: unknown): SearchArgs {
  if (!isObj(raw)) throw bad("args");
  let match: string | undefined;
  if (raw.query !== undefined) {
    if (typeof raw.query !== "string") throw bad("query");
    if (raw.query.length > MAX_QUERY_CHARS || queryTerms(raw.query).length > MAX_TERMS) {
      throw new CatalogFailure("SEARCH_QUERY_INVALID", "the search query is too long", { maxTerms: MAX_TERMS, maxChars: MAX_QUERY_CHARS });
    }
    match = matchExpression(raw.query);
  }
  for (const key of ["kind", "state"] as const) if (raw[key] !== undefined && typeof raw[key] !== "string") throw bad(key);
  if (raw.since !== undefined && !isInt(raw.since)) throw bad("since");
  const [durationMs, width, height, size] = (["durationMs", "width", "height", "size"] as const).map((k) => range(raw[k], k));
  return {
    ...(match === undefined ? {} : { match }),
    ...(raw.kind === undefined ? {} : { kind: raw.kind as string }),
    ...(raw.state === undefined ? {} : { state: raw.state as string }),
    ...(raw.since === undefined ? {} : { since: raw.since as number }),
    ...(durationMs === undefined ? {} : { durationMs }),
    ...(width === undefined ? {} : { width }),
    ...(height === undefined ? {} : { height }),
    ...(size === undefined ? {} : { size }),
  };
}

/**
 * The `WHERE` terms of the filters over `files f`. Without a `state` filter a search skips
 * `missing` rows (they stay in the index until purged); asking for `state: "missing"` lists them.
 */
function filters(a: SearchArgs): { sql: string[]; params: Array<string | number> } {
  const sql: string[] = [];
  const params: Array<string | number> = [];
  sql.push(a.state === undefined ? "f.state != 'missing'" : "f.state = ?");
  if (a.state !== undefined) params.push(a.state);
  if (a.kind !== undefined) (sql.push("f.kind = ?"), params.push(a.kind));
  if (a.since !== undefined) (sql.push("f.lastChange >= ?"), params.push(a.since));
  for (const col of ["durationMs", "width", "height", "size"] as const) {
    const r = a[col];
    if (r?.min !== undefined) (sql.push(`f.${col} >= ?`), params.push(r.min));
    if (r?.max !== undefined) (sql.push(`f.${col} <= ?`), params.push(r.max));
  }
  return { sql, params };
}

const COLS = "f.id, f.dirId, f.nameNfc, f.relKey, f.kind, f.size, f.durationMs, f.width, f.height, f.fp, f.state, f.collision, f.lastChange";

/**
 * Where an empty query resumes: after `(lastChange, id)` of this library's last hit on the
 * previous page. `cmp` compares this library's id with the cursor's library: the order is
 * `lastChange` descending, then library id and row id ascending, so a library that sorts
 * before the cursor's resumes strictly below the cursor's time, one that sorts after it at the
 * cursor's time too, and the cursor's own after its row id.
 */
export interface After {
  readonly lastChange: number;
  readonly id: number;
  readonly cmp: -1 | 0 | 1;
}

/**
 * Up to `limit` rows of one catalog: with terms by `bm25(names, 4.0, 1.0)` then row id (the
 * terms are prefix terms over `name` and `dir`, every one required); without, by `lastChange`
 * descending then row id, optionally resuming `after` a keyset position.
 */
export function searchRows(db: DatabaseSync, a: SearchArgs, limit: number, after?: After): HitRow[] {
  const f = filters(a);
  if (a.match !== undefined) {
    const rows = db
      .prepare(
        `SELECT ${COLS}, bm25(names, 4.0, 1.0) AS score FROM names JOIN files f ON f.id = names.rowid ` +
          `WHERE names MATCH ? AND ${f.sql.join(" AND ")} ORDER BY score, f.id LIMIT ?`,
      )
      .all(a.match, ...f.params, limit);
    return rows as unknown as HitRow[];
  }
  const where = [...f.sql];
  const params = [...f.params];
  if (after !== undefined) {
    if (after.cmp < 0) (where.push("f.lastChange < ?"), params.push(after.lastChange));
    else if (after.cmp > 0) (where.push("f.lastChange <= ?"), params.push(after.lastChange));
    else (where.push("(f.lastChange < ? OR (f.lastChange = ? AND f.id > ?))"), params.push(after.lastChange, after.lastChange, after.id));
  }
  const rows = db
    .prepare(`SELECT ${COLS}, 0 AS score FROM files f WHERE ${where.join(" AND ")} ORDER BY f.lastChange DESC, f.id LIMIT ?`)
    .all(...params, limit);
  return rows as unknown as HitRow[];
}

const DISPLAY_SQL =
  "WITH RECURSIVE up (id, parent, nameNfc, depth) AS (" +
  "SELECT id, parent, nameNfc, 0 FROM dirs WHERE id = ? " +
  "UNION ALL SELECT d.id, d.parent, d.nameNfc, up.depth + 1 FROM dirs d JOIN up ON d.id = up.parent) " +
  "SELECT nameNfc FROM up WHERE parent IS NOT NULL ORDER BY depth DESC";

/** Display paths (NFC, true case) of directories, one query per distinct directory. */
export class DisplayPaths {
  readonly #stmt: StatementSync;
  readonly #cache = new Map<number, string>();
  constructor(db: DatabaseSync) {
    this.#stmt = db.prepare(DISPLAY_SQL);
  }

  /** The directory's path below the root (`""` for the root). */
  dir(dirId: number): string {
    let hit = this.#cache.get(dirId);
    if (hit === undefined) {
      hit = (this.#stmt.all(dirId) as Array<{ nameNfc: string }>).map((r) => r.nameNfc).join("/");
      this.#cache.set(dirId, hit);
    }
    return hit;
  }

  file(dirId: number, nameNfc: string): string {
    const dir = this.dir(dirId);
    return dir === "" ? nameNfc : `${dir}/${nameNfc}`;
  }
}

/** Whether the library is offline (`meta.offline`, Decision 18). */
export function libraryOffline(db: DatabaseSync): boolean {
  const row = db.prepare("SELECT value FROM meta WHERE key = 'offline'").get() as { value: string } | undefined;
  return row?.value === "1";
}

/** A row with its display path as the hit a client sees. */
export function toHit(library: string, row: HitRow, display: string, libOffline: boolean): Hit {
  return {
    library,
    fileId: row.id,
    relKey: row.relKey,
    display,
    ...(row.kind === null ? {} : { kind: row.kind }),
    size: row.size,
    ...(row.durationMs === null ? {} : { durationMs: row.durationMs }),
    ...(row.width === null ? {} : { width: row.width }),
    ...(row.height === null ? {} : { height: row.height }),
    ...(row.fp === null ? {} : { fp: row.fp }),
    state: row.state,
    collision: row.collision !== 0,
    score: row.score,
    offline: libOffline || row.state === "offline",
    lastChange: row.lastChange,
  };
}
