// Opening a library's catalog in the indexer thread (plan 5D Decision 23):
// `<server-data>/libraries/<id>/catalog.db`, `node:sqlite`, WAL.
// - `PRAGMA quick_check` at open; a damaged file (or one that is not SQLite) is moved to
//   `catalog.db.corrupt-<ulid>` and an empty catalog is created, `rebuilt: corrupt`, which the
//   service answers with a full scan (stage results survive in the caches, by fingerprint);
// - `meta.libraryId` must equal the directory's id: a catalog copied from another library is
//   moved to `catalog.db.foreign-<ulid>`, `rebuilt: foreign`;
// - every extension's `minReaderVersion` is checked: above this code's `formatVersion` is
//   `CATALOG_TOO_NEW` and the file is left alone (the library goes offline);
// - pending migrations of every extension run in one transaction (`core` first).
// Catalogs are rebuildable from the root, so there is no pre-migration backup.
import { chmodSync, closeSync, existsSync, mkdirSync, openSync, renameSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { applyMigrations, planMigrations } from "../../control/migrations.ts";
import { isUlid, ulid } from "../../control/ulid.ts";
import type { LoadedExtensions } from "./extensions.ts";
import { CatalogFailure, sqliteCode } from "./protocol.ts";

export const CATALOG_DB = "catalog.db";
const BUSY_MS = 250;
const SQLITE_NOTADB = 26;
const SIDE_FILES = ["", "-wal", "-shm"] as const;

export function libraryDir(serverData: string, lib: string): string {
  return join(serverData, "libraries", lib);
}

export function catalogPath(serverData: string, lib: string): string {
  return join(libraryDir(serverData, lib), CATALOG_DB);
}

/** Library ids are ULIDs: the only thing that becomes a path segment. */
export function checkLibraryId(lib: unknown): string {
  if (typeof lib !== "string" || !isUlid(lib)) throw new CatalogFailure("CATALOG_ARGS_INVALID", "a library id is a ULID", { at: "lib" });
  return lib;
}

export interface OpenedCatalog {
  readonly db: DatabaseSync;
  readonly path: string;
  readonly created: boolean;
  /** Set when an unusable file was moved aside and the catalog started empty. */
  readonly rebuilt?: { readonly reason: "corrupt" | "foreign"; readonly aside: string };
}

class Corrupt extends Error {}

function isNotADatabase(e: unknown): boolean {
  const code = sqliteCode(e);
  if (code !== undefined) return code === SQLITE_NOTADB;
  return /file is not a database/i.test(String((e as Error | null)?.message ?? e));
}

function createPrivate(path: string): void {
  closeSync(openSync(path, "a", 0o600));
  if (process.platform !== "win32") chmodSync(path, 0o600);
}

function connect(path: string): DatabaseSync {
  const db = new DatabaseSync(path);
  try {
    db.exec(`PRAGMA busy_timeout = ${BUSY_MS}`);
    const rows = db.prepare("PRAGMA quick_check").all() as Array<Record<string, unknown>>;
    const ok = rows.length === 1 && Object.values(rows[0] ?? {})[0] === "ok";
    if (!ok) throw new Corrupt("quick_check");
    const mode = db.prepare("PRAGMA journal_mode = WAL").get() as { journal_mode: string };
    if (mode.journal_mode !== "wal") throw new Error(`journal_mode is ${mode.journal_mode}, not wal`);
    db.exec("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;");
    return db;
  } catch (e) {
    db.close();
    throw e;
  }
}

function moveAside(path: string, kind: "corrupt" | "foreign", now: number): string {
  const aside = `${path}.${kind}-${ulid(now)}`;
  for (const suffix of SIDE_FILES) if (existsSync(path + suffix)) renameSync(path + suffix, aside + suffix);
  return aside;
}

function storedLibraryId(db: DatabaseSync): string | undefined {
  const hasMeta = db.prepare("SELECT 1 AS x FROM sqlite_master WHERE type = 'table' AND name = 'meta'").get() !== undefined;
  if (!hasMeta) return undefined;
  return (db.prepare("SELECT value FROM meta WHERE key = 'libraryId'").get() as { value: string } | undefined)?.value;
}

/** Opens (creating, migrating or rebuilding) a library's catalog; see the header. */
export function openCatalog(serverData: string, libArg: unknown, exts: LoadedExtensions, now: number): OpenedCatalog {
  const lib = checkLibraryId(libArg);
  const dir = libraryDir(serverData, lib);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const path = join(dir, CATALOG_DB);
  const created = !existsSync(path);
  let rebuilt: OpenedCatalog["rebuilt"];
  createPrivate(path);
  let db: DatabaseSync;
  try {
    db = connect(path);
  } catch (e) {
    if (!(e instanceof Corrupt) && !isNotADatabase(e)) throw e;
    rebuilt = { reason: "corrupt", aside: moveAside(path, "corrupt", now) };
    createPrivate(path);
    db = connect(path);
  }
  const stored = storedLibraryId(db);
  if (stored !== undefined && stored !== lib) {
    db.close();
    rebuilt = { reason: "foreign", aside: moveAside(path, "foreign", now) };
    createPrivate(path);
    db = connect(path);
  }
  try {
    const plan = planMigrations(db, exts.chains);
    if (plan.tooNew.length > 0) {
      throw new CatalogFailure("CATALOG_TOO_NEW", "the library's catalog was written by a newer finisherd", { extensions: plan.tooNew });
    }
    db.exec("BEGIN IMMEDIATE");
    try {
      if (plan.pending.length > 0) applyMigrations(db, plan, now);
      if (storedLibraryId(db) === undefined) {
        db.prepare("INSERT INTO meta (key, value) VALUES ('libraryId', ?), ('createdAt', ?)").run(lib, String(now));
      }
      db.exec("COMMIT");
    } catch (e) {
      db.exec("ROLLBACK");
      throw e;
    }
  } catch (e) {
    db.close();
    throw e;
  }
  return { db, path, created: created || rebuilt !== undefined, ...(rebuilt === undefined ? {} : { rebuilt }) };
}
