// The `library-indexer` worker thread (plan 5D Decision 2): the only writer of every catalog.
// It imports the catalog extensions at start, opens and migrates catalogs, reads and validates
// scan batch files, and runs extension ops inside transactions. Each transaction holds ≤ 2,000
// rows; a long apply yields between them, so other libraries' requests interleave, while one
// library's requests run strictly in order (a queue per library). The main thread sends only
// ids, paths and plain data, and never runs a catalog statement itself.
import { writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, relative } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { parentPort, workerData } from "node:worker_threads";
import { type BatchFileRef, readBatchFile } from "./batch-file.ts";
import { type ApplyConn, applyRows, checkChains, emptyCounts, prepareApply, resetApply, rowsOf } from "./core/apply.ts";
import { checkRootEmpty, settleHardlinks } from "./core/diff.ts";
import { DirChains } from "./core/rel.ts";
import { type CatalogExtensionRef, type LoadedExtensions, loadExtensions } from "./extensions.ts";
import { ftsRemove, ftsSettle } from "./fts.ts";
import { checkLibraryId, openCatalog } from "./open.ts";
import { CatalogFailure, type Reply, type Request, nextTurn, toWire } from "./protocol.ts";

export interface IndexerData {
  readonly extensions: ReadonlyArray<CatalogExtensionRef>;
  readonly serverData: string;
  /** Batch files and `in/` files must lie under this directory (the engine's jobs root). */
  readonly jobsRoot?: string;
  /** Rows per transaction (default and maximum 2,000; tests lower it). */
  readonly txRows?: number;
}

export const TX_ROWS = 2000;
const PURGE_BATCH = 500;

interface Catalog {
  readonly db: DatabaseSync;
  readonly path: string;
  readonly apply: ApplyConn;
  /** The latest transaction time applied: batch times never go backwards (`diff.ts`). */
  lastAt: number;
}

const data = workerData as IndexerData;
const port = parentPort!;
const txRows = Math.max(1, Math.min(TX_ROWS, data.txRows ?? TX_ROWS));
const catalogs = new Map<string, Catalog>();
const queues = new Map<string, Promise<unknown>>();
const stats = { transactions: 0, maxTxRows: 0, batchFiles: 0 };

let exts: LoadedExtensions;
try {
  exts = await loadExtensions(data.extensions);
  port.postMessage({ ready: true });
} catch (e) {
  port.postMessage({ ready: false, error: toWire(e) });
  process.exit(1);
}

function catalog(lib: unknown): Catalog {
  const c = catalogs.get(checkLibraryId(lib));
  if (c === undefined) throw new CatalogFailure("CATALOG_NOT_OPEN", "the library's catalog is not open", {});
  return c;
}

/** `BEGIN IMMEDIATE … COMMIT`, rolled back (and the directory cache reset) if `f` throws. */
function tx<A>(c: Catalog, rows: number, f: () => A): A {
  c.db.exec("BEGIN IMMEDIATE");
  try {
    const a = f();
    if (a instanceof Promise) throw new CatalogFailure("CATALOG_OP_FAILED", "catalog ops must be synchronous", {});
    c.db.exec("COMMIT");
    ftsSettle(c.apply.fts);
    stats.transactions++;
    stats.maxTxRows = Math.max(stats.maxTxRows, rows);
    return a;
  } catch (e) {
    try {
      c.db.exec("ROLLBACK");
    } catch {
      // COMMIT failed after SQLite had already rolled back.
    }
    ftsSettle(c.apply.fts);
    // Listings that saw rows in the rolled-back transaction mark nothing (`diff.ts`).
    resetApply(c.apply);
    throw e;
  }
}

const obj = (args: unknown): Record<string, unknown> => {
  if (args === null || typeof args !== "object") throw new CatalogFailure("CATALOG_ARGS_INVALID", "catalog arguments must be an object", {});
  return args as Record<string, unknown>;
};
const int = (v: unknown, at: string): number => {
  if (typeof v !== "number" || !Number.isSafeInteger(v)) throw new CatalogFailure("CATALOG_ARGS_INVALID", "invalid catalog arguments", { at });
  return v;
};

/** A path the indexer writes for a job: absolute, in a directory named `in`, under the jobs root. */
function jobInputPath(v: unknown): string {
  const bad = new CatalogFailure("CATALOG_ARGS_INVALID", "invalid catalog arguments", { at: "path" });
  if (typeof v !== "string" || !isAbsolute(v) || basename(dirname(v)) !== "in") throw bad;
  if (data.jobsRoot !== undefined) {
    const rel = relative(data.jobsRoot, v);
    if (rel === "" || rel.startsWith("..") || isAbsolute(rel)) throw bad;
  }
  return v;
}

function open(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  const known = catalogs.get(lib);
  if (known !== undefined) return { path: known.path, created: false };
  const opened = openCatalog(data.serverData, lib, exts, int(a.at, "at"));
  const apply = prepareApply(opened.db);
  const meta = opened.db.prepare("SELECT value FROM meta WHERE key = 'offline'").get() as { value: string } | undefined;
  apply.diff.offline = meta?.value === "1";
  const last = opened.db
    .prepare("SELECT max(coalesce((SELECT max(lastSeen) FROM files), 0), coalesce((SELECT max(lastSeen) FROM dirs), 0)) AS at")
    .get() as { at: number };
  catalogs.set(lib, { db: opened.db, path: opened.path, apply, lastAt: last.at });
  return { path: opened.path, created: opened.created, ...(opened.rebuilt === undefined ? {} : { rebuilt: opened.rebuilt.reason }) };
}

function close(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  const c = catalogs.get(lib);
  if (c === undefined) return { closed: false };
  catalogs.delete(lib);
  try {
    c.db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  } finally {
    c.db.close();
  }
  return { closed: true };
}

function exec(a: Record<string, unknown>): unknown {
  const c = catalog(a.lib);
  const mod = typeof a.ext === "string" ? exts.modules.get(a.ext) : undefined;
  if (mod === undefined) throw new CatalogFailure("CATALOG_EXTENSION_UNKNOWN", "no such catalog extension", { ext: String(a.ext) });
  const op = typeof a.op === "string" && Object.hasOwn(mod.ops, a.op) ? mod.ops[a.op] : undefined;
  if (op === undefined) throw new CatalogFailure("CATALOG_OP_UNKNOWN", "no such catalog op", { ext: a.ext, op: String(a.op) });
  return tx(c, 1, () => op(c.db, a.args));
}

/**
 * Reads, validates, then applies a batch file in transactions of ≤ `txRows` rows. The job is
 * the directory holding the file's `out/` (the diff's listings are per job), and an empty root
 * listing while the catalog holds files fails the file before any write (`ROOT_EMPTY_SUSPECT`).
 */
async function applyBatchFile(a: Record<string, unknown>): Promise<unknown> {
  const c = catalog(a.lib);
  const gen = int(a.gen, "gen");
  const at = int(a.at, "at");
  const ref = obj(a.file) as unknown as BatchFileRef;
  const batches = readBatchFile(ref, data.jobsRoot);
  const job = basename(dirname(ref.outDir));
  if (a.seq !== undefined && batches.some((b) => b.seq !== a.seq)) {
    throw new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason: "seq" });
  }
  const rows = rowsOf(batches);
  checkChains(c.apply, rows);
  checkRootEmpty(c.apply.diff, batches.flatMap((b) => b.dirsDone));
  const counts = emptyCounts();
  for (let i = 0; i < rows.length; i += txRows) {
    if (i > 0) await nextTurn();
    if (catalogs.get(checkLibraryId(a.lib)) !== c) throw new CatalogFailure("CATALOG_NOT_OPEN", "the library's catalog is not open", {});
    const chunk = rows.slice(i, i + txRows);
    const ctx = { job, gen, at: Math.max(at, c.lastAt) };
    tx(c, chunk.length, () => {
      applyRows(c.apply, chunk, ctx, counts);
      settleHardlinks(c.apply.diff);
    });
    c.lastAt = ctx.at;
  }
  stats.batchFiles++;
  return { ...counts, transactions: Math.ceil(rows.length / txRows) };
}

/**
 * Writes the job's `in/dirs.ndjson` (Decision 27): one line per known directory (present or
 * unreadable) `{dir: RawRel, mtimeNs: string | null, listed: bool}`, `listed` when its last
 * complete listing was in generation `gen`. Created exclusively, 0600.
 */
function writeKnownDirs(a: Record<string, unknown>): unknown {
  const c = catalog(a.lib);
  const gen = int(a.gen, "gen");
  const path = jobInputPath(a.path);
  const rows = c.db
    .prepare("SELECT id, CAST(mtimeNs AS TEXT) AS mtimeNs, listedGen FROM dirs WHERE state IN ('present', 'unreadable') ORDER BY id")
    .all() as Array<{ id: number; mtimeNs: string | null; listedGen: number | null }>;
  const chains = new DirChains(c.db);
  const lines = rows.map((r) => JSON.stringify({ dir: { components: chains.of(r.id) }, mtimeNs: r.mtimeNs, listed: r.listedGen === gen }));
  writeFileSync(path, lines.length === 0 ? "" : `${lines.join("\n")}\n`, { flag: "wx", mode: 0o600 });
  return { dirs: rows.length };
}

/**
 * Deletes rows missing since before `before` (Decision 25: 90 days), ≤ 500 per transaction,
 * with their FTS rows and the stage rows no remaining file shares; then missing directories
 * last seen before `before` that hold no rows any more, deepest first.
 */
async function purgeMissing(a: Record<string, unknown>): Promise<unknown> {
  const c = catalog(a.lib);
  const before = int(a.before, "before");
  const pick = c.db.prepare("SELECT id, fp FROM files WHERE state = 'missing' AND missingSince < ? ORDER BY id LIMIT ?");
  const del = c.db.prepare("DELETE FROM files WHERE id = ?");
  const orphan = c.db.prepare("DELETE FROM stages WHERE fp = ? AND NOT EXISTS (SELECT 1 FROM files WHERE fp = ?)");
  const fts = c.apply.fts;
  let deleted = 0;
  for (;;) {
    if (catalogs.get(checkLibraryId(a.lib)) !== c) break;
    const rows = pick.all(before, PURGE_BATCH) as Array<{ id: number; fp: string | null }>;
    if (rows.length === 0) break;
    tx(c, rows.length, () => {
      for (const r of rows) {
        ftsRemove(fts, r.id);
        del.run(r.id);
      }
      for (const fp of new Set(rows.map((r) => r.fp))) if (fp !== null) orphan.run(fp, fp);
    });
    deleted += rows.length;
    if (rows.length < PURGE_BATCH) break;
    await nextTurn();
  }
  const emptyDirs = c.db.prepare(
    "SELECT id FROM dirs d WHERE state = 'missing' AND lastSeen < ? AND id != 1 AND NOT EXISTS (SELECT 1 FROM files WHERE dirId = d.id) " +
      "AND NOT EXISTS (SELECT 1 FROM dirs e WHERE e.parent = d.id) ORDER BY id LIMIT ?",
  );
  const delDir = c.db.prepare("DELETE FROM dirs WHERE id = ?");
  let dirs = 0;
  for (;;) {
    if (catalogs.get(checkLibraryId(a.lib)) !== c) break;
    const rows = emptyDirs.all(before, PURGE_BATCH) as Array<{ id: number }>;
    if (rows.length === 0) break;
    tx(c, rows.length, () => {
      for (const r of rows) delDir.run(r.id);
    });
    dirs += rows.length;
    await nextTurn();
  }
  if (dirs > 0) resetApply(c.apply);
  return { deleted, dirs };
}

/** Persists the library's offline flag in `meta` and hands it to the diff (Decision 18). */
function setOffline(a: Record<string, unknown>): unknown {
  const c = catalog(a.lib);
  if (typeof a.offline !== "boolean") throw new CatalogFailure("CATALOG_ARGS_INVALID", "invalid catalog arguments", { at: "offline" });
  const offline = a.offline;
  tx(c, 1, () => {
    c.db.prepare("INSERT INTO meta (key, value) VALUES ('offline', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value").run(offline ? "1" : "0");
  });
  c.apply.diff.offline = offline;
  return { offline };
}

const METHODS: Record<string, (a: Record<string, unknown>) => unknown> = {
  open,
  close,
  exec,
  applyBatchFile,
  writeKnownDirs,
  purgeMissing,
  setOffline,
};

function reply(r: Reply): void {
  try {
    port.postMessage(r);
  } catch (e) {
    port.postMessage({ id: r.id, ok: false, error: { code: "CATALOG_RESULT_NOT_CLONEABLE", message: "a catalog result is not plain data", details: { cause: String(e) } } });
  }
}

/** Runs `f` after every earlier request of the same library. */
function serial(lib: string, f: () => Promise<unknown>): Promise<unknown> {
  const prev = queues.get(lib) ?? Promise.resolve();
  const next = prev.then(f, f);
  const settled = next.catch(() => undefined);
  queues.set(lib, settled);
  void settled.then(() => {
    if (queues.get(lib) === settled) queues.delete(lib);
  });
  return next;
}

function shutdown(): void {
  for (const lib of [...catalogs.keys()]) {
    try {
      close({ lib });
    } catch {
      // Closing is best effort at shutdown.
    }
  }
}

port.on("message", (msg: Request) => {
  if (msg.method === "stats") return reply({ id: msg.id, ok: true, value: { ...stats, open: catalogs.size } });
  if (msg.method === "shutdown") {
    void Promise.allSettled([...queues.values()]).then(() => {
      shutdown();
      reply({ id: msg.id, ok: true, value: {} });
      port.close();
    });
    return;
  }
  const method = Object.hasOwn(METHODS, msg.method) ? METHODS[msg.method] : undefined;
  if (method === undefined) {
    return reply({ id: msg.id, ok: false, error: { code: "CATALOG_OP_UNKNOWN", message: "no such catalog method", details: { method: msg.method } } });
  }
  const args = msg.args as Record<string, unknown>;
  const lib = typeof args?.lib === "string" ? args.lib : "";
  void serial(lib, async () => method(obj(args))).then(
    (value) => reply({ id: msg.id, ok: true, value }),
    (e: unknown) => reply({ id: msg.id, ok: false, error: toWire(e) }),
  );
});
