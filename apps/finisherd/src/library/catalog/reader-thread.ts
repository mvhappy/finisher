// The `library-reader` worker thread (plan 5D Decision 2): serves search, listings and lookups
// on **read-only** connections (`readOnly: true` plus `PRAGMA query_only`), so a write from a
// query fails and an answer never waits behind the indexer's writes (WAL readers see the last
// committed state). It imports the same extension modules as the indexer and runs their
// `queries`; a catalog is attached by the client only after the indexer has opened it.
import { DatabaseSync } from "node:sqlite";
import { parentPort, workerData } from "node:worker_threads";
import { type CatalogExtensionRef, type LoadedExtensions, loadExtensions } from "./extensions.ts";
import { checkLibraryId } from "./open.ts";
import { CatalogFailure, type Reply, type Request, toWire } from "./protocol.ts";

export interface ReaderData {
  readonly extensions: ReadonlyArray<CatalogExtensionRef>;
}

const BUSY_MS = 1000;
const data = workerData as ReaderData;
const port = parentPort!;
const conns = new Map<string, DatabaseSync>();
const stats = { queries: 0 };

let exts: LoadedExtensions;
try {
  exts = await loadExtensions(data.extensions);
  port.postMessage({ ready: true });
} catch (e) {
  port.postMessage({ ready: false, error: toWire(e) });
  process.exit(1);
}

function attach(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  if (typeof a.path !== "string") throw new CatalogFailure("CATALOG_ARGS_INVALID", "invalid catalog arguments", { at: "path" });
  conns.get(lib)?.close();
  const db = new DatabaseSync(a.path, { readOnly: true });
  db.exec(`PRAGMA busy_timeout = ${BUSY_MS}; PRAGMA query_only = ON;`);
  conns.set(lib, db);
  return {};
}

function detach(a: Record<string, unknown>): unknown {
  const lib = checkLibraryId(a.lib);
  conns.get(lib)?.close();
  return { detached: conns.delete(lib) };
}

function query(a: Record<string, unknown>): unknown {
  const db = conns.get(checkLibraryId(a.lib));
  if (db === undefined) throw new CatalogFailure("CATALOG_NOT_OPEN", "the library's catalog is not open", {});
  const mod = typeof a.ext === "string" ? exts.modules.get(a.ext) : undefined;
  if (mod === undefined) throw new CatalogFailure("CATALOG_EXTENSION_UNKNOWN", "no such catalog extension", { ext: String(a.ext) });
  const q = typeof a.query === "string" && Object.hasOwn(mod.queries, a.query) ? mod.queries[a.query] : undefined;
  if (q === undefined) throw new CatalogFailure("CATALOG_OP_UNKNOWN", "no such catalog query", { ext: a.ext, query: String(a.query) });
  stats.queries++;
  const value = q(db, a.args);
  if (value instanceof Promise) throw new CatalogFailure("CATALOG_OP_FAILED", "catalog queries must be synchronous", {});
  return value;
}

const METHODS: Record<string, (a: Record<string, unknown>) => unknown> = { attach, detach, query };

function reply(r: Reply): void {
  try {
    port.postMessage(r);
  } catch (e) {
    port.postMessage({ id: r.id, ok: false, error: { code: "CATALOG_RESULT_NOT_CLONEABLE", message: "a catalog result is not plain data", details: { cause: String(e) } } });
  }
}

port.on("message", (msg: Request) => {
  if (msg.method === "stats") return reply({ id: msg.id, ok: true, value: { ...stats, open: conns.size } });
  if (msg.method === "shutdown") {
    for (const db of conns.values()) db.close();
    conns.clear();
    reply({ id: msg.id, ok: true, value: {} });
    port.close();
    return;
  }
  const method = Object.hasOwn(METHODS, msg.method) ? METHODS[msg.method] : undefined;
  try {
    if (method === undefined) throw new CatalogFailure("CATALOG_OP_UNKNOWN", "no such catalog method", { method: msg.method });
    const args = msg.args;
    if (args === null || typeof args !== "object") throw new CatalogFailure("CATALOG_ARGS_INVALID", "catalog arguments must be an object", {});
    reply({ id: msg.id, ok: true, value: method(args as Record<string, unknown>) });
  } catch (e) {
    reply({ id: msg.id, ok: false, error: toWire(e) });
  }
});
