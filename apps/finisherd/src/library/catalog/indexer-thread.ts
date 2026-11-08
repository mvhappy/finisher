// The `library-indexer` worker thread (plan 5D Decision 2): the only writer of every catalog.
// It imports the catalog extensions at start, opens and migrates catalogs, reads and validates
// scan batch files, and runs extension ops inside transactions. Each transaction holds ≤ 2,000
// batch rows; a long apply yields between them, and a subtree change inside one (thousands of
// rows under a vanished or excluded directory) yields between steps of ≤ 2,000 rows while its
// transaction stays open (readers see it whole). So other libraries' requests interleave,
// while one library's requests run strictly in order (a queue per library). The main thread
// sends only ids, paths and plain data, and never runs a catalog statement itself.
//
// **Catalog time.** Every write is stamped `max(clock, lastAt)`, so times never go backwards
// per catalog; `attemptStart` answers a time strictly after every write so far (a job
// attempt's `startedAt`, `diff.ts`), and the latest time is kept in `meta.clock` across a
// close. The main thread's clock also holds forward jumps (`clock.ts`).
import { writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, relative } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { parentPort, workerData } from "node:worker_threads";
import { type BatchFileRef, readBatchFile } from "./batch-file.ts";
import { type ApplyConn, type ApplyCounts, ROOT_DIR, applyRows, checkChains, emptyCounts, prepareApply, resetApply, rowsOf } from "./core/apply.ts";
import { type DiffCtx, checkRootEmpty, settleHardlinks } from "./core/diff.ts";
import { type Work, budget, settle } from "./core/steps.ts";
import { DirChains } from "./core/rel.ts";
import { type CatalogExtensionRef, type LoadedExtensions, loadExtensions } from "./extensions.ts";
import { ftsRemove, ftsSettle } from "./fts.ts";
import { checkLibraryId, openCatalog } from "./open.ts";
import { CatalogFailure, type Reply, type Request, nextTurn, toWire } from "./protocol.ts";
import { Watches } from "../watch.ts";
import { newSink, settleEvents } from "./core/file-events.ts";

export interface IndexerData {
  readonly extensions: ReadonlyArray<CatalogExtensionRef>;
  readonly serverData: string;
  /** Batch files and `in/` files must lie under this directory (the engine's jobs root). */
  readonly jobsRoot: string;
  /** Rows per transaction (default and maximum 2,000; tests lower it). */
  readonly txRows?: number;
  /** Enables `testFault` and `watchFault` (tests only). */
  readonly testHooks?: boolean;
  /** Quiet time before a watch hint is posted (default 2 s; tests lower it). */
  readonly watchQuietMs?: number;
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
const stats = { transactions: 0, maxTxRows: 0, maxStepRows: 0, batchFiles: 0 };
/** `testFault`: the batch transaction (1-based, per library) that fails next. */
const faults = new Map<string, number>();

/** Local roots' watches (Decision 30): hints posted as `{event}` messages. */
const watches = new Watches((event) => port.postMessage({ event }), {
  ...(data.watchQuietMs === undefined ? {} : { quietMs: data.watchQuietMs }),
  known: knownPrefix,
});

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

function rollback(c: Catalog): void {
  try {
    c.db.exec("ROLLBACK");
  } catch {
    // COMMIT failed after SQLite had already rolled back.
  }
  ftsSettle(c.apply.fts);
}

function committed(c: Catalog, rows: number): void {
  c.db.exec("COMMIT");
  ftsSettle(c.apply.fts);
  stats.transactions++;
  stats.maxTxRows = Math.max(stats.maxTxRows, rows);
}

/** `BEGIN IMMEDIATE … COMMIT`, rolled back if `f` throws. */
function tx<A>(c: Catalog, rows: number, f: () => A): A {
  c.db.exec("BEGIN IMMEDIATE");
  try {
    const a = f();
    if (a instanceof Promise) throw new CatalogFailure("CATALOG_OP_FAILED", "catalog ops must be synchronous", {});
    committed(c, rows);
    return a;
  } catch (e) {
    rollback(c);
    throw e;
  }
}

/**
 * One batch transaction: runs `work`, taking an event-loop turn at each of its yields (the
 * transaction stays open), then commits. On a failure it rolls back and drops the cached
 * directories and every open listing: those that saw rows in the rolled-back transaction must
 * mark nothing (`diff.ts`).
 */
async function batchTx(c: Catalog, rows: number, work: Work): Promise<void> {
  c.db.exec("BEGIN IMMEDIATE");
  try {
    for (;;) {
      const r = work.next();
      if (r.done === true) break;
      await nextTurn();
    }
    committed(c, rows);
  } catch (e) {
    rollback(c);
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
  const rel = relative(data.jobsRoot, v);
  if (rel === "" || rel.startsWith("..") || isAbsolute(rel)) throw bad;
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
    .prepare(
      "SELECT max(coalesce((SELECT max(lastSeen) FROM files), 0), coalesce((SELECT max(lastSeen) FROM dirs), 0), " +
        "coalesce((SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'clock'), 0)) AS at",
    )
    .get() as { at: number };
  catalogs.set(lib, { db: opened.db, path: opened.path, apply, lastAt: last.at });
  return { path: opened.path, created: opened.created, ...(opened.rebuilt === undefined ? {} : { rebuilt: opened.rebuilt.reason }) };
}

function close(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  const c = catalogs.get(lib);
  if (c === undefined) return { closed: false };
  catalogs.delete(lib);
  watches.unwatch(lib);
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

const CLOCK_SET = "INSERT INTO meta (key, value) VALUES ('clock', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value";

/** The catalog time for a write now: never before `lastAt`. */
function stamp(c: Catalog, at: number): number {
  c.lastAt = Math.max(at, c.lastAt);
  return c.lastAt;
}

/**
 * A job attempt's `startedAt` (`diff.ts`): a catalog time strictly after every write so far
 * and no later than any write after it, kept in `meta.clock`.
 */
function attemptStart(a: Record<string, unknown>): unknown {
  const c = catalog(a.lib);
  const t = Math.max(int(a.at, "at"), c.lastAt + 1);
  tx(c, 1, () => c.db.prepare(CLOCK_SET).run(String(t)));
  c.lastAt = t;
  return { startedAt: t };
}

/**
 * Reads, validates, then applies a batch file in transactions of ≤ `txRows` rows. The job is
 * the directory holding the file's `out/` (the diff's listings are per job), `startedAt` the
 * attempt's `attemptStart`, and an empty root listing while the catalog holds files fails the
 * file before any write (`ROOT_EMPTY_SUSPECT`).
 */
async function applyBatchFile(a: Record<string, unknown>): Promise<unknown> {
  const c = catalog(a.lib);
  const gen = int(a.gen, "gen");
  const at = int(a.at, "at");
  const startedAt = int(a.startedAt, "startedAt");
  const lib = checkLibraryId(a.lib);
  const fault = faults.get(lib);
  faults.delete(lib);
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
  const ctx = { job, gen, at: stamp(c, at), startedAt };
  const sink = newSink();
  c.apply.diff.events = sink;
  try {
    await applyRowsInTx(c, lib, rows, ctx, counts, fault);
  } catch (e) {
    // The time was handed out even though the transaction rolled back: keep it (`attemptStart`).
    if (catalogs.get(lib) === c) tx(c, 1, () => c.db.prepare(CLOCK_SET).run(String(ctx.at)));
    throw e;
  } finally {
    c.apply.diff.events = undefined;
  }
  stats.batchFiles++;
  // Which rows changed, for `Libraries.onFile` (read after the last transaction committed).
  return { ...counts, transactions: Math.ceil(rows.length / txRows), files: settleEvents(c.db, sink) };
}

async function applyRowsInTx(
  c: Catalog,
  lib: string,
  rows: ReturnType<typeof rowsOf>,
  ctx: DiffCtx,
  counts: ApplyCounts,
  faultAt: number | undefined,
): Promise<void> {
  for (let i = 0, n = 1; i < rows.length; i += txRows, n++) {
    if (i > 0) await nextTurn();
    if (catalogs.get(lib) !== c) throw new CatalogFailure("CATALOG_NOT_OPEN", "the library's catalog is not open", {});
    const chunk = rows.slice(i, i + txRows);
    const b = budget(txRows);
    const fault = faultAt === n;
    await batchTx(c, chunk.length, (function* () {
      c.db.prepare(CLOCK_SET).run(String(ctx.at));
      yield* applyRows(c.apply, chunk, ctx, counts, b);
      yield* settleHardlinks(c.apply.diff, b);
      if (fault) throw new CatalogFailure("CATALOG_TEST_FAULT", "a test fault", { transaction: n });
    })());
    settle(b);
    stats.maxStepRows = Math.max(stats.maxStepRows, b.maxRows);
  }
}

/** Tests: makes the `transaction`-th transaction (1-based) of the library's next batch file fail after its writes. */
function testFault(a: Record<string, unknown>): unknown {
  if (data.testHooks !== true) throw new CatalogFailure("CATALOG_OP_UNKNOWN", "no such catalog method", { method: "testFault" });
  faults.set(checkLibraryId(a.lib), int(a.transaction, "transaction"));
  return {};
}

/**
 * The lines of a job's `in/dirs.ndjson` (Decision 27): one per known directory (present,
 * unreadable, or offline under an unmounted nested mount)
 * `{dir: RawRel, mtimeNs: string | null, listed: bool, entries?, mountRoot, volume?}` (what
 * `fin_library::KnownDirs::from_ndjson` reads): `listed` when its last complete listing was in
 * generation `gen`, `entries` that listing's count (for the walker's N → 0 rule), `mountRoot`
 * for a nested mount point and `volume` its volume as last listed (for `ROOT_UNMOUNTED` and
 * `ROOT_VOLUME_CHANGED`).
 */
function knownDirLines(c: Catalog, gen: number): string[] {
  type Row = { id: number; mtimeNs: string | null; listedGen: number | null; entries: number | null; mount: string | null; volume: string | null };
  const rows = c.db
    .prepare(
      "SELECT id, CAST(mtimeNs AS TEXT) AS mtimeNs, listedGen, entries, mount, volume FROM dirs " +
        "WHERE state IN ('present', 'unreadable', 'offline') ORDER BY id",
    )
    .all() as Row[];
  const chains = new DirChains(c.db);
  return rows.map((r) =>
    JSON.stringify({
      dir: { components: chains.of(r.id) },
      mtimeNs: r.mtimeNs,
      listed: r.listedGen === gen,
      ...(r.entries === null ? {} : { entries: r.entries }),
      mountRoot: r.mount !== null,
      ...(r.volume === null ? {} : { volume: JSON.parse(r.volume) as unknown }),
    }),
  );
}

const ndjson = (lines: ReadonlyArray<string>): string => (lines.length === 0 ? "" : `${lines.join("\n")}\n`);

/** Writes the job's `in/dirs.ndjson` at `path` (`knownDirLines`), created exclusively, 0600. */
function writeKnownDirs(a: Record<string, unknown>): unknown {
  const c = catalog(a.lib);
  const gen = int(a.gen, "gen");
  const path = jobInputPath(a.path);
  const lines = knownDirLines(c, gen);
  writeFileSync(path, ndjson(lines), { flag: "wx", mode: 0o600 });
  return { dirs: lines.length };
}

/**
 * The text of a job's `in/dirs.ndjson` (`knownDirLines`), for a kind's `prepare` to write
 * through its `ctx.open` (hooks get no raw paths).
 */
function knownDirs(a: Record<string, unknown>): unknown {
  const lines = knownDirLines(catalog(a.lib), int(a.gen, "gen"));
  return { text: ndjson(lines), dirs: lines.length };
}

/**
 * Deletes rows missing since before `before` (Decision 25: 90 days) **and** marked missing in
 * a generation before `beforeGen` (so a forward clock jump alone purges nothing: full scans
 * must have run since), ≤ 500 per transaction, with their FTS rows and the stage rows no
 * remaining file shares; then missing directories last seen before `before` that hold no
 * rows any more, deepest first.
 */
async function purgeMissing(a: Record<string, unknown>): Promise<unknown> {
  const c = catalog(a.lib);
  const before = int(a.before, "before");
  const beforeGen = int(a.beforeGen, "beforeGen");
  const pick = c.db.prepare(
    "SELECT id, fp FROM files WHERE state = 'missing' AND missingSince < ? AND coalesce(missingGen, 0) < ? ORDER BY id LIMIT ?",
  );
  const del = c.db.prepare("DELETE FROM files WHERE id = ?");
  const orphan = c.db.prepare("DELETE FROM stages WHERE fp = ? AND NOT EXISTS (SELECT 1 FROM files WHERE fp = ?)");
  const fts = c.apply.fts;
  let deleted = 0;
  for (;;) {
    if (catalogs.get(checkLibraryId(a.lib)) !== c) break;
    const rows = pick.all(before, beforeGen, PURGE_BATCH) as Array<{ id: number; fp: string | null }>;
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

/** The deepest directory of `parts` the catalog knows, as base64 components (a watch hint's target). */
function knownPrefix(lib: string, parts: ReadonlyArray<Uint8Array>): string[] {
  const c = catalogs.get(lib);
  const out: string[] = [];
  if (c === undefined) return out;
  let id = ROOT_DIR;
  for (const p of parts) {
    const row = c.apply.st.dirGet.get(id, p) as { id: number } | undefined;
    if (row === undefined) break;
    id = row.id;
    out.push(Buffer.from(p.buffer, p.byteOffset, p.byteLength).toString("base64"));
  }
  return out;
}

/** `watch {lib, root}`: a local root's recursive watch (the catalog must be open). */
function watch(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  catalog(lib);
  if (typeof a.root !== "string" || !isAbsolute(a.root)) throw new CatalogFailure("CATALOG_ARGS_INVALID", "invalid catalog arguments", { at: "root" });
  return watches.watch(lib, a.root);
}

function unwatch(a: Record<string, unknown>): unknown {
  watches.unwatch(checkLibraryId(a.lib));
  return {};
}

function watchFault(a: Record<string, unknown>): unknown {
  if (data.testHooks !== true) throw new CatalogFailure("CATALOG_OP_UNKNOWN", "no such catalog method", { method: "watchFault" });
  return { faulted: watches.fault(checkLibraryId(a.lib)) };
}

const METHODS: Record<string, (a: Record<string, unknown>) => unknown> = {
  open,
  close,
  exec,
  applyBatchFile,
  attemptStart,
  testFault,
  writeKnownDirs,
  knownDirs,
  purgeMissing,
  setOffline,
  watch,
  unwatch,
  watchFault,
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
  watches.closeAll();
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
