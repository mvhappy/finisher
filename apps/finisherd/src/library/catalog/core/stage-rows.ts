// The stage framework's catalog side (plan 5D Task 11; Decisions 39–42): which fingerprints
// a stage would take next, versions going stale, and stage rows by fingerprint or job. Rows
// are keyed `(fp, stage)` (one per fingerprint in this library, whatever its files); the pump
// in `finisherd` owns in-flight sets, priorities and retry times and passes what it decided.
import type { DatabaseSync } from "node:sqlite";
import { CatalogFailure } from "../protocol.ts";
import { decodeProbe } from "./probe-codec.ts";
import { DirChains, base64 } from "./rel.ts";
import { DisplayPaths } from "./search.ts";

const bad = (what: string, at: string): CatalogFailure =>
  new CatalogFailure("CATALOG_ARGS_INVALID", `catalog core.${what}: invalid arguments`, { at });
const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const strList = (v: unknown, max: number): v is string[] => Array.isArray(v) && v.length <= max && v.every(isStr);

/** One file a stage would take: what `StageFile` is built from. */
export interface StagePendingFile {
  readonly fileId: number;
  readonly rel: { components: string[] };
  readonly display: string;
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileIdBytes: string | null;
  readonly dev: string | null;
  readonly fp: string;
  readonly ext: string | null;
  readonly kind: string | null;
  readonly probe: unknown;
  readonly state: string;
}

/**
 * The stage branch of `pending {stage, version, limit ≤ 1000, maxAttempts?, dependsOn?,
 * hydrate?, retryBefore?, fps? ≤ 256}`: present (with `hydrate`, also `cloudOnly`) fingerprinted files,
 * one per fingerprint (its lowest live id), whose row for `stage` is absent, at another
 * version, `stale`, or `failed` with attempts left and failed no later than `retryBefore`;
 * each of `dependsOn` (`[{stage, version}]`) must be `done` at its version; with `fps`, only
 * those fingerprints (a prioritized one). Oldest change first.
 */
export function pendingStage(db: DatabaseSync, a: Record<string, unknown>): { files: StagePendingFile[] } {
  if (!isStr(a.version)) throw bad("pending", "version");
  if (a.maxAttempts !== undefined && a.maxAttempts !== null && !isInt(a.maxAttempts)) throw bad("pending", "maxAttempts");
  if (a.retryBefore !== undefined && !isInt(a.retryBefore)) throw bad("pending", "retryBefore");
  if (a.hydrate !== undefined && typeof a.hydrate !== "boolean") throw bad("pending", "hydrate");
  const deps = a.dependsOn ?? [];
  if (!Array.isArray(deps) || deps.length > 16 || !deps.every((d) => isObj(d) && isStr(d.stage) && isStr(d.version))) throw bad("pending", "dependsOn");
  if (a.fps !== undefined && !strList(a.fps, 256)) throw bad("pending", "fps");
  const max = (a.maxAttempts as number | undefined) ?? 3;
  const before = (a.retryBefore as number | undefined) ?? Number.MAX_SAFE_INTEGER;
  const states = JSON.stringify(a.hydrate === true ? ["present", "cloudOnly"] : ["present"]);
  const depSql = (deps as Array<{ stage: string; version: string }>)
    .map(() => " AND EXISTS (SELECT 1 FROM stages d WHERE d.fp = f.fp AND d.stage = ? AND d.version = ? AND d.state = 'done')")
    .join("");
  const sql =
    "SELECT f.id, f.dirId, f.name, f.nameNfc, f.size, CAST(f.mtimeNs AS TEXT) AS mtimeNs, f.fileId, f.dev, f.fp, f.ext, f.kind, f.probe, f.state " +
    "FROM files f LEFT JOIN stages s ON s.fp = f.fp AND s.stage = ? " +
    "WHERE f.state IN (SELECT value FROM json_each(?)) AND f.fp IS NOT NULL " +
    "AND (s.fp IS NULL OR s.version != ? OR s.state = 'stale' OR (s.state = 'failed' AND s.attempts < ? AND s.at <= ?))" +
    depSql +
    (a.fps === undefined ? "" : " AND f.fp IN (SELECT value FROM json_each(?))") +
    " AND f.id = (SELECT min(g.id) FROM files g WHERE g.fp = f.fp AND g.state IN (SELECT value FROM json_each(?))) " +
    "ORDER BY f.lastChange, f.id LIMIT ?";
  const params: Array<string | number> = [a.stage as string, states, a.version, max, before];
  for (const d of deps as Array<{ stage: string; version: string }>) params.push(d.stage, d.version);
  if (a.fps !== undefined) params.push(JSON.stringify(a.fps));
  params.push(states, a.limit as number);
  const rows = db.prepare(sql).all(...params) as Array<Record<string, unknown>>;
  const chains = new DirChains(db);
  const paths = new DisplayPaths(db);
  return {
    files: rows.map((r) => {
      const fp = r.fp as string;
      const kind = (r.kind as string | null) ?? null;
      return {
        fileId: r.id as number,
        rel: chains.file(r.dirId as number, r.name as Uint8Array),
        display: paths.file(r.dirId as number, r.nameNfc as string),
        size: r.size as number,
        mtimeNs: r.mtimeNs as string,
        fileIdBytes: r.fileId === null ? null : base64(r.fileId as Uint8Array),
        dev: (r.dev as string | null) ?? null,
        fp,
        ext: (r.ext as string | null) ?? null,
        kind,
        probe: decodeProbe((r.probe as Uint8Array | null) ?? null, { fp, size: r.size as number, kind }),
        state: r.state as string,
      };
    }),
  };
}

/**
 * `stagesStale {stage, version}` (Decision 42): the stage's rows at any other version that are
 * not `stale` yet become `stale` (the pump takes them again); other stages are untouched.
 */
function stagesStale(db: DatabaseSync, args: unknown): { stale: number } {
  if (!isObj(args) || !isStr(args.stage) || !isStr(args.version)) throw bad("stagesStale", "args");
  const r = db.prepare("UPDATE stages SET state = 'stale' WHERE stage = ? AND version != ? AND state != 'stale'").run(args.stage, args.version);
  return { stale: Number(r.changes) };
}

export interface StageRow {
  readonly fp: string;
  readonly stage: string;
  readonly version: string;
  readonly state: string;
  readonly code: string | null;
  readonly jobId: string | null;
  readonly attempts: number;
  readonly at: number;
}

/**
 * `stageRows {fps? ≤ 1000, jobIds? ≤ 1000, states?, stage?, limit? ≤ 5000}`: stage rows by
 * fingerprint, by job or by state (each given filter applies), ordered by stage then fp.
 */
function stageRows(db: DatabaseSync, args: unknown): { rows: StageRow[] } {
  if (!isObj(args)) throw bad("stageRows", "args");
  const where: string[] = [];
  const params: Array<string | number> = [];
  for (const [key, col, max] of [["fps", "fp", 1000], ["jobIds", "jobId", 1000], ["states", "state", 8]] as const) {
    const v = args[key];
    if (v === undefined) continue;
    if (!strList(v, max)) throw bad("stageRows", key);
    where.push(`${col} IN (SELECT value FROM json_each(?))`);
    params.push(JSON.stringify(v));
  }
  if (args.stage !== undefined) {
    if (!isStr(args.stage)) throw bad("stageRows", "stage");
    where.push("stage = ?");
    params.push(args.stage);
  }
  const limit = args.limit ?? 5000;
  if (!isInt(limit) || limit < 1 || limit > 5000) throw bad("stageRows", "limit");
  const sql = `SELECT fp, stage, version, state, code, jobId, attempts, at FROM stages${where.length === 0 ? "" : ` WHERE ${where.join(" AND ")}`} ORDER BY stage, fp LIMIT ?`;
  return { rows: db.prepare(sql).all(...params, limit) as unknown as StageRow[] };
}

export const stageOps = { stagesStale };
export const stageQueries = { stageRows };
