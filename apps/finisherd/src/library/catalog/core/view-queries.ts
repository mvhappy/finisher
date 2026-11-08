// Two small reader queries behind the RPCs' views (plan 5D Task 13): a directory's display
// path (a listing shows its entries' paths from it) and the per-directory warnings of a
// library (`ROOT_UNMOUNTED {dir}`, `ROOT_VOLUME_CHANGED {dir}`, `DIR_EMPTY_SUSPECT {dir}`).
import type { DatabaseSync } from "node:sqlite";
import { CatalogFailure } from "../protocol.ts";
import { DisplayPaths } from "./search.ts";

/** Directories named in a view's warnings at most (`total` counts them all). */
export const MAX_DIR_WARNINGS = 20;

const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);

/** `dirDisplay {dirId}` → `{display}`: the NFC path below the root (`""` for the root). */
function dirDisplay(db: DatabaseSync, args: unknown): { display: string } {
  const id = (args as { dirId?: unknown } | null)?.dirId;
  if (!isInt(id)) throw new CatalogFailure("CATALOG_ARGS_INVALID", "catalog query core.dirDisplay: invalid arguments", { at: "dirId" });
  return { display: new DisplayPaths(db).dir(id) };
}

/**
 * `dirWarnings {}` → `{warnings: [{code, dir}], total}`: directories listed as an offline
 * mount (`ROOT_UNMOUNTED`, `ROOT_VOLUME_CHANGED`) or suspected empty (`DIR_EMPTY_SUSPECT`),
 * lowest id first, at most [`MAX_DIR_WARNINGS`].
 */
function dirWarnings(db: DatabaseSync): { warnings: Array<{ code: string; dir: string }>; total: number } {
  const where = "state != 'missing' AND code IN ('ROOT_UNMOUNTED', 'ROOT_VOLUME_CHANGED', 'DIR_EMPTY_SUSPECT')";
  const total = (db.prepare(`SELECT count(*) AS n FROM dirs WHERE ${where}`).get() as { n: number }).n;
  const rows = db.prepare(`SELECT id, code FROM dirs WHERE ${where} ORDER BY id LIMIT ?`).all(MAX_DIR_WARNINGS) as Array<{ id: number; code: string }>;
  const paths = new DisplayPaths(db);
  return { warnings: rows.map((r) => ({ code: r.code, dir: paths.dir(r.id) })), total };
}

export const viewQueries = { dirDisplay, dirWarnings };
