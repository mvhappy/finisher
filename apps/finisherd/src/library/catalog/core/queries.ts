// The `core` extension's queries (plan 5D Task 7a): reads the reader thread runs on its
// read-only connection. Task 12 builds search snapshots and cursors over `search`; the RPCs
// (Task 13) shape the answers. Rows cross the port as plain data: raw bytes as base64,
// nanosecond times as decimal strings, probes decoded here (off the main thread).
import type { DatabaseSync } from "node:sqlite";
import { matchExpression } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";
import { decodeProbe } from "./probe-codec.ts";
import { DirChains, base64 } from "./rel.ts";
import { DisplayPaths } from "./search.ts";
import { stageQueries } from "./stage-rows.ts";
import { viewQueries } from "./view-queries.ts";

const bad = (q: string, at: string): CatalogFailure =>
  new CatalogFailure("CATALOG_ARGS_INVALID", `catalog query core.${q}: invalid arguments`, { at });
const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);
const isStr = (v: unknown): v is string => typeof v === "string";

export interface FileRow {
  readonly id: number;
  readonly dirId: number;
  readonly name: string;
  readonly nameNfc: string;
  readonly nameLossy: boolean;
  readonly relKey: string;
  readonly ext: string | null;
  readonly collision: number;
  readonly hardlink: boolean;
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileId: string | null;
  readonly placeholder: string | null;
  readonly fp: string | null;
  readonly fullHash: string | null;
  readonly kind: string | null;
  readonly probe: unknown;
  readonly probeVersion: number | null;
  readonly durationMs: number | null;
  readonly width: number | null;
  readonly height: number | null;
  readonly state: string;
  readonly code: string | null;
  readonly firstSeen: number;
  readonly lastSeen: number;
  readonly lastChange: number;
  readonly missingSince: number | null;
}

const FILE_FIELDS = [
  "id", "dirId", "name", "nameNfc", "nameLossy", "relKey", "ext", "collision", "hardlink", "size", "mtimeNs", "fileId",
  "placeholder", "fp", "fullHash", "kind", "probe", "probeVersion", "durationMs", "width", "height", "state", "code",
  "firstSeen", "lastSeen", "lastChange", "missingSince",
] as const;

/** The columns `fileRow` reads, optionally qualified (`f.`); `mtimeNs` as text (it passes 2^53). */
export const fileCols = (t = ""): string =>
  FILE_FIELDS.map((c) => (c === "mtimeNs" ? `CAST(${t}mtimeNs AS TEXT) AS mtimeNs` : `${t}${c}`)).join(", ");
const FILE_COLS = fileCols();

export function fileRow(r: Record<string, unknown>): FileRow {
  return {
    id: r.id as number,
    dirId: r.dirId as number,
    name: base64(r.name as Uint8Array),
    nameNfc: r.nameNfc as string,
    nameLossy: r.nameLossy === 1,
    relKey: r.relKey as string,
    ext: (r.ext as string | null) ?? null,
    collision: r.collision as number,
    hardlink: r.hardlink === 1,
    size: r.size as number,
    mtimeNs: r.mtimeNs as string,
    fileId: r.fileId === null ? null : base64(r.fileId as Uint8Array),
    placeholder: (r.placeholder as string | null) ?? null,
    fp: (r.fp as string | null) ?? null,
    fullHash: (r.fullHash as string | null) ?? null,
    kind: (r.kind as string | null) ?? null,
    probe: decodeProbe((r.probe as Uint8Array | null) ?? null, {
      fp: (r.fp as string | null) ?? null,
      size: r.size as number,
      kind: (r.kind as string | null) ?? null,
    }),
    probeVersion: (r.probeVersion as number | null) ?? null,
    durationMs: (r.durationMs as number | null) ?? null,
    width: (r.width as number | null) ?? null,
    height: (r.height as number | null) ?? null,
    state: r.state as string,
    code: (r.code as string | null) ?? null,
    firstSeen: r.firstSeen as number,
    lastSeen: r.lastSeen as number,
    lastChange: r.lastChange as number,
    missingSince: (r.missingSince as number | null) ?? null,
  };
}

export interface DirRow {
  readonly id: number;
  readonly parent: number | null;
  readonly name: string;
  readonly nameNfc: string;
  readonly relKey: string;
  readonly mtimeNs: string | null;
  readonly placeholder: string | null;
  readonly collision: number;
  readonly state: string;
  readonly code: string | null;
  readonly lastSeen: number;
}

const DIR_COLS = "id, parent, name, nameNfc, relKey, CAST(mtimeNs AS TEXT) AS mtimeNs, placeholder, collision, state, code, lastSeen";
const dirRow = (r: Record<string, unknown>): DirRow => ({
  id: r.id as number,
  parent: (r.parent as number | null) ?? null,
  name: base64(r.name as Uint8Array),
  nameNfc: r.nameNfc as string,
  relKey: r.relKey as string,
  mtimeNs: (r.mtimeNs as string | null) ?? null,
  placeholder: (r.placeholder as string | null) ?? null,
  collision: r.collision as number,
  state: r.state as string,
  code: (r.code as string | null) ?? null,
  lastSeen: r.lastSeen as number,
});

/**
 * `files {dirId? | dir? (relKey), state?, kind?, after?: {nameNfc, id}, limit ≤ 500}`: one
 * directory's subdirectories and files, keyset on `(nameNfc, id)`. A `dir` key matching
 * several directories answers them all in `matches` and lists none (the caller answers
 * `PATH_AMBIGUOUS`).
 */
function files(db: DatabaseSync, args: unknown): { dir?: DirRow; matches?: DirRow[]; dirs: DirRow[]; files: FileRow[]; next?: { nameNfc: string; id: number } } {
  if (!isObj(args) || !isInt(args.limit) || args.limit < 1 || args.limit > 500) throw bad("files", "limit");
  let dir: DirRow | undefined;
  if (args.dir !== undefined) {
    if (!isStr(args.dir)) throw bad("files", "dir");
    const found = (db.prepare(`SELECT ${DIR_COLS} FROM dirs WHERE relKey = ?`).all(args.dir) as Array<Record<string, unknown>>).map(dirRow);
    if (found.length !== 1) return { matches: found, dirs: [], files: [] };
    dir = found[0];
  } else {
    const id = args.dirId ?? 1;
    if (!isInt(id)) throw bad("files", "dirId");
    const row = db.prepare(`SELECT ${DIR_COLS} FROM dirs WHERE id = ?`).get(id) as Record<string, unknown> | undefined;
    if (row === undefined) return { matches: [], dirs: [], files: [] };
    dir = dirRow(row);
  }
  const after = args.after;
  if (after !== undefined && (!isObj(after) || !isStr(after.nameNfc) || !isInt(after.id))) throw bad("files", "after");
  const where: string[] = ["dirId = ?"];
  const params: Array<string | number> = [dir!.id];
  if (args.state !== undefined) {
    if (!isStr(args.state)) throw bad("files", "state");
    where.push("state = ?");
    params.push(args.state);
  }
  if (args.kind !== undefined) {
    if (!isStr(args.kind)) throw bad("files", "kind");
    where.push("kind = ?");
    params.push(args.kind);
  }
  if (after !== undefined) {
    where.push("(nameNfc, id) > (?, ?)");
    params.push(after.nameNfc as string, after.id as number);
  }
  const rows = db
    .prepare(`SELECT ${FILE_COLS} FROM files WHERE ${where.join(" AND ")} ORDER BY nameNfc, id LIMIT ?`)
    .all(...params, args.limit + 1) as Array<Record<string, unknown>>;
  const more = rows.length > args.limit;
  const list = rows.slice(0, args.limit).map(fileRow);
  const dirs =
    after === undefined
      ? (db.prepare(`SELECT ${DIR_COLS} FROM dirs WHERE parent = ? ORDER BY nameNfc, id`).all(dir!.id) as Array<Record<string, unknown>>).map(dirRow)
      : [];
  const last = list.at(-1);
  return { dir: dir!, dirs, files: list, ...(more && last !== undefined ? { next: { nameNfc: last.nameNfc, id: last.id } } : {}) };
}

/**
 * `file {fileId} | {relKey}`: every row with that id or key (several → `PATH_AMBIGUOUS` for
 * the caller), each with its raw `rel` chain, its `dev` and its NFC `display` path, and the
 * stage rows of a single match.
 */
function file(db: DatabaseSync, args: unknown): { matches: Array<FileRow & { rel: { components: string[] }; display: string; dev: string | null }>; stages: unknown[] } {
  if (!isObj(args)) throw bad("file", "args");
  let rows: Array<Record<string, unknown>>;
  if (args.fileId !== undefined) {
    if (!isInt(args.fileId)) throw bad("file", "fileId");
    rows = db.prepare(`SELECT ${FILE_COLS} FROM files WHERE id = ?`).all(args.fileId) as Array<Record<string, unknown>>;
  } else if (isStr(args.relKey)) {
    rows = db.prepare(`SELECT ${FILE_COLS} FROM files WHERE relKey = ? ORDER BY id`).all(args.relKey) as Array<Record<string, unknown>>;
  } else throw bad("file", "fileId");
  const chains = new DirChains(db);
  const paths = new DisplayPaths(db);
  const devOf = db.prepare("SELECT dev FROM files WHERE id = ?");
  const matches = rows.map((r) => ({
    ...fileRow(r),
    rel: chains.file(r.dirId as number, r.name as Uint8Array),
    display: paths.file(r.dirId as number, r.nameNfc as string),
    dev: ((devOf.get(r.id as number) as { dev: string | null } | undefined)?.dev ?? null) as string | null,
  }));
  const fp = matches.length === 1 ? matches[0]?.fp : null;
  const stages =
    fp == null ? [] : db.prepare("SELECT stage, version, state, code, jobId, attempts, at FROM stages WHERE fp = ? ORDER BY stage").all(fp);
  return { matches, stages };
}

const range = (v: unknown): v is { min?: number; max?: number } =>
  isObj(v) && (v.min === undefined || isInt(v.min)) && (v.max === undefined || isInt(v.max));

/**
 * `search {query?, kind?, durationMs?, width?, height? ({min?, max?}), since?, state?, limit ≤ 2000}`:
 * prefix terms over `name` and `dir`, scored `bm25(names, 4.0, 1.0)` (lower is better), ties by
 * row id; with no terms, by `lastChange` descending. Task 12 snapshots and pages these hits.
 */
function search(db: DatabaseSync, args: unknown): { hits: Array<FileRow & { score: number }> } {
  if (!isObj(args) || !isInt(args.limit) || args.limit < 1 || args.limit > 2000) throw bad("search", "limit");
  if (args.query !== undefined && !isStr(args.query)) throw bad("search", "query");
  const match = isStr(args.query) ? matchExpression(args.query) : undefined;
  const where: string[] = [];
  const params: Array<string | number> = [];
  const cols = fileCols("f.");
  for (const [key, col] of [["durationMs", "durationMs"], ["width", "width"], ["height", "height"]] as const) {
    const r = args[key];
    if (r === undefined) continue;
    if (!range(r)) throw bad("search", key);
    if (r.min !== undefined) (where.push(`f.${col} >= ?`), params.push(r.min));
    if (r.max !== undefined) (where.push(`f.${col} <= ?`), params.push(r.max));
  }
  for (const key of ["kind", "state"] as const) {
    const v = args[key];
    if (v === undefined) continue;
    if (!isStr(v)) throw bad("search", key);
    where.push(`f.${key} = ?`);
    params.push(v);
  }
  if (args.since !== undefined) {
    if (!isInt(args.since)) throw bad("search", "since");
    where.push("f.lastChange >= ?");
    params.push(args.since);
  }
  const filter = where.length === 0 ? "" : ` AND ${where.join(" AND ")}`;
  const rows =
    match === undefined
      ? (db
          .prepare(`SELECT ${cols}, 0 AS score FROM files f WHERE 1 = 1${filter} ORDER BY f.lastChange DESC, f.id LIMIT ?`)
          .all(...params, args.limit) as Array<Record<string, unknown>>)
      : (db
          .prepare(
            `SELECT ${cols}, bm25(names, 4.0, 1.0) AS score FROM names JOIN files f ON f.id = names.rowid ` +
              `WHERE names MATCH ?${filter} ORDER BY score, f.id LIMIT ?`,
          )
          .all(match, ...params, args.limit) as Array<Record<string, unknown>>);
  return { hits: rows.map((r) => ({ ...fileRow(r), score: r.score as number })) };
}

/**
 * `lookup {fingerprints? ≤ 1000, hashes? ≤ 1000}`: exact matches in any state (the caller
 * sees `state`), `key` the matched value, `display` the file's NFC path.
 */
function lookup(db: DatabaseSync, args: unknown): { matches: Array<{ key: string; display: string; file: FileRow }> } {
  if (!isObj(args)) throw bad("lookup", "args");
  const out: Array<{ key: string; display: string; file: FileRow }> = [];
  const paths = new DisplayPaths(db);
  for (const [key, col] of [["fingerprints", "fp"], ["hashes", "fullHash"]] as const) {
    const vals = args[key];
    if (vals === undefined) continue;
    if (!Array.isArray(vals) || vals.length > 1000 || !vals.every(isStr)) throw bad("lookup", key);
    const rows = db
      .prepare(`SELECT ${FILE_COLS} FROM files WHERE ${col} IN (SELECT value FROM json_each(?)) ORDER BY id`)
      .all(JSON.stringify(vals)) as Array<Record<string, unknown>>;
    for (const r of rows) out.push({ key: r[col] as string, display: paths.file(r.dirId as number, r.nameNfc as string), file: fileRow(r) });
  }
  return { matches: out };
}

/**
 * `hashCandidates {sizes ≤ 1000, limit ≤ 1000}`: ids of `present` rows without a full hash
 * whose size is one of `sizes`, lowest first; `more` when the limit cut the list.
 */
function hashCandidates(db: DatabaseSync, args: unknown): { fileIds: number[]; more: boolean } {
  if (!isObj(args) || !isInt(args.limit) || args.limit < 1 || args.limit > 1000) throw bad("hashCandidates", "limit");
  const sizes = args.sizes;
  if (!Array.isArray(sizes) || sizes.length > 1000 || !sizes.every(isInt)) throw bad("hashCandidates", "sizes");
  if (sizes.length === 0) return { fileIds: [], more: false };
  const rows = db
    .prepare("SELECT id FROM files WHERE state = 'present' AND fullHash IS NULL AND size IN (SELECT value FROM json_each(?)) ORDER BY id LIMIT ?")
    .all(JSON.stringify(sizes), args.limit + 1) as Array<{ id: number }>;
  return { fileIds: rows.slice(0, args.limit).map((r) => r.id), more: rows.length > args.limit };
}

/**
 * `counts {}`: files per state, directories per state, and live rows with collision bits
 * (the library's `NAME_COLLISION {count}`, Decision 11).
 */
function counts(db: DatabaseSync): { files: Record<string, number>; dirs: Record<string, number>; collisions: number } {
  const fold = (rows: unknown[]) => Object.fromEntries((rows as Array<{ state: string; n: number }>).map((r) => [r.state, r.n]));
  const live = "collision != 0 AND state != 'missing'";
  const collisions = db.prepare(`SELECT (SELECT count(*) FROM files WHERE ${live}) + (SELECT count(*) FROM dirs WHERE ${live}) AS n`).get() as {
    n: number;
  };
  return {
    files: fold(db.prepare("SELECT state, count(*) AS n FROM files GROUP BY state").all()),
    dirs: fold(db.prepare("SELECT state, count(*) AS n FROM dirs WHERE id != 1 GROUP BY state").all()),
    collisions: collisions.n,
  };
}

/** `stageCounts {}`: rows per stage, version and state. */
function stageCounts(db: DatabaseSync): { stages: Array<{ stage: string; version: string; state: string; n: number }> } {
  const rows = db.prepare("SELECT stage, version, state, count(*) AS n FROM stages GROUP BY stage, version, state ORDER BY stage, version, state").all();
  return { stages: rows as Array<{ stage: string; version: string; state: string; n: number }> };
}

/** One line of a probe or hash job's `in/files.ndjson` (the generated `LibraryFileLine`). */
export interface FileLineRow {
  readonly id: number;
  readonly rel: { components: string[] };
  readonly expect: { size: number; mtimeNs: string; fileId?: string; dev?: string };
}

/** Rows a reader may open: live and not a placeholder (a placeholder only for a hydrating library). */
const READABLE = ["present", "probeFailed", "skipped", "unreadable"];

/**
 * `fileLines {fileIds ≤ 1000, hydrate}`: the lines of a probe or hash job's `in/files.ndjson`
 * (Decision 20) for the readable rows among `fileIds`, in their order: the raw chain and the
 * `Expect` the opener checks (size, mtime, file id, and `dev` from `files.dev`: a nested mount's
 * file carries its own device). `absent`: the ids with no readable row (gone, missing, offline,
 * or `cloudOnly` without `hydrate`).
 */
function fileLines(db: DatabaseSync, args: unknown): { lines: FileLineRow[]; absent: number[] } {
  if (!isObj(args) || !Array.isArray(args.fileIds) || args.fileIds.length > 1000 || !args.fileIds.every(isInt)) throw bad("fileLines", "fileIds");
  if (typeof args.hydrate !== "boolean") throw bad("fileLines", "hydrate");
  const states = args.hydrate ? [...READABLE, "cloudOnly"] : READABLE;
  const get = db.prepare(
    "SELECT id, dirId, name, size, CAST(mtimeNs AS TEXT) AS mtimeNs, fileId, dev, state FROM files WHERE id = ? " +
      "AND state IN (SELECT value FROM json_each(?))",
  );
  const chains = new DirChains(db);
  const lines: FileLineRow[] = [];
  const absent: number[] = [];
  const statesJson = JSON.stringify(states);
  for (const id of args.fileIds as number[]) {
    const r = get.get(id, statesJson) as Record<string, unknown> | undefined;
    if (r === undefined) {
      absent.push(id);
      continue;
    }
    const fileId = r.fileId === null ? undefined : base64(r.fileId as Uint8Array);
    const dev = (r.dev as string | null) ?? undefined;
    lines.push({
      id,
      rel: chains.file(r.dirId as number, r.name as Uint8Array),
      expect: { size: r.size as number, mtimeNs: r.mtimeNs as string, ...(fileId === undefined ? {} : { fileId }), ...(dev === undefined ? {} : { dev }) },
    });
  }
  return { lines, absent };
}

/**
 * `dirRel {dir: relKey} | {dirId}` → `{rel?}`: the raw chain of the one directory named (a
 * listing's freshness pass lists exactly it, Decision 28); none when the key names none or
 * several.
 */
function dirRel(db: DatabaseSync, args: unknown): { rel?: { components: string[] } } {
  if (!isObj(args)) throw bad("dirRel", "args");
  let ids: number[];
  if (args.dir !== undefined) {
    if (!isStr(args.dir)) throw bad("dirRel", "dir");
    ids = (db.prepare("SELECT id FROM dirs WHERE relKey = ? AND state != 'missing'").all(args.dir) as Array<{ id: number }>).map((r) => r.id);
  } else {
    const id = args.dirId ?? 1;
    if (!isInt(id)) throw bad("dirRel", "dirId");
    ids = (db.prepare("SELECT id FROM dirs WHERE id = ?").all(id) as Array<{ id: number }>).map((r) => r.id);
  }
  if (ids.length !== 1) return {};
  return { rel: { components: new DirChains(db).of(ids[0]!) } };
}

export const coreQueries = { files, file, search, lookup, hashCandidates, counts, stageCounts, fileLines, dirRel, ...viewQueries, ...stageQueries };
