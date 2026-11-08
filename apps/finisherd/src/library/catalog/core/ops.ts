// The `core` extension's ops (plan 5D Task 7a): writes the indexer runs inside one
// transaction each, through `IndexerClient`'s typed wrappers. The scan-batch apply, the
// missing-row purge and the known-directories file need several transactions or a file
// write, so they are built into `indexer-thread.ts` instead.
//
// Every op checks its arguments: they come from the main thread, which got them from
// workers, whose output is untrusted (spec §6.5).
import type { DatabaseSync, StatementSync } from "node:sqlite";
import { type FtsStatements, prepareFts } from "../fts.ts";
import { CatalogFailure } from "../protocol.ts";
import { type DiffConn, EXCLUDED, mergeInto, prepareDiff, settleHardlinks } from "./diff.ts";
import { drain, unlimited } from "./steps.ts";
import { encodeProbe } from "./probe-codec.ts";
import { DirChains } from "./rel.ts";

const bad = (op: string, at: string): CatalogFailure =>
  new CatalogFailure("CATALOG_ARGS_INVALID", `catalog op core.${op}: invalid arguments`, { at });

const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const optStr = (v: unknown): boolean => v === undefined || v === null || typeof v === "string";
const optInt = (v: unknown): boolean => v === undefined || v === null || isInt(v);
const I64 = /^-?\d{1,19}$/;

function list(op: string, args: unknown, key: string, max: number): Array<Record<string, unknown>> {
  if (!isObj(args) || !Array.isArray(args[key]) || (args[key] as unknown[]).length > max) throw bad(op, key);
  const rows = args[key] as unknown[];
  rows.forEach((r, i) => {
    if (!isObj(r)) throw bad(op, `${key}[${i}]`);
  });
  return rows as Array<Record<string, unknown>>;
}

const PROBED_STATES = new Set(["present", "probeFailed", "skipped"]);

interface Merging {
  readonly diff: DiffConn;
  readonly fts: FtsStatements;
  readonly before: StatementSync;
  readonly moveFrom: StatementSync;
}

const merging = new WeakMap<DatabaseSync, Merging>();

function mergingOf(db: DatabaseSync): Merging {
  let m = merging.get(db);
  if (m === undefined) {
    m = {
      diff: prepareDiff(db),
      fts: prepareFts(db),
      before: db.prepare("SELECT fp, renameOf, firstSeen = lastChange AS untouched FROM files WHERE id = ?"),
      moveFrom: db.prepare(
        `SELECT id FROM files WHERE fp = ? AND state = 'missing' AND coalesce(code, '') != '${EXCLUDED}' AND id != ? ORDER BY missingSince DESC, id LIMIT 1`,
      ),
    };
    merging.set(db, m);
  }
  return m;
}

/**
 * `applyProbed {results ≤ 256}`: each `{fileId, size, mtimeNs, fp?, probe?, probeVersion,
 * state: present|probeFailed|skipped, code?, kind?, durationMs?, width?, height?}` lands only
 * when the row still has the probed size and mtime (a row changed since keeps its cleared
 * fields and is probed again). A row's **first** fingerprint, on a row unchanged since it was
 * inserted and not a pending rename, that equals a missing row's (never an `EXCLUDED` one: its
 * file still exists) is a **move** (Decision 25):
 * the missing row (latest `missingSince`, then lowest id) takes the new row's path and this
 * probe, keeps its id and history, and the new row is dropped. Answers `{updated, stale, moved}`.
 */
function applyProbed(db: DatabaseSync, args: unknown): { updated: number; stale: number; moved: Array<{ from: number; to: number }> } {
  const rows = list("applyProbed", args, "results", 256);
  const st = db.prepare(
    "UPDATE files SET fp = ?, probe = ?, probeVersion = ?, state = ?, code = ?, kind = ?, durationMs = ?, width = ?, height = ? " +
      "WHERE id = ? AND size = ? AND mtimeNs = ? AND state NOT IN ('missing', 'cloudOnly', 'offline')",
  );
  const m = mergingOf(db);
  let updated = 0;
  const moved: Array<{ from: number; to: number }> = [];
  rows.forEach((r, i) => {
    const okRow =
      isInt(r.fileId) && isInt(r.size) && isStr(r.mtimeNs) && I64.test(r.mtimeNs) && isInt(r.probeVersion) &&
      isStr(r.state) && PROBED_STATES.has(r.state) && optStr(r.fp) && optStr(r.code) && optStr(r.kind) &&
      optInt(r.durationMs) && optInt(r.width) && optInt(r.height);
    if (!okRow) throw bad("applyProbed", `results[${i}]`);
    const fp = (r.fp as string | null | undefined) ?? null;
    const kind = (r.kind as string | null | undefined) ?? null;
    const probe = encodeProbe(r.probe, { fp, size: r.size as number, kind });
    const prior = m.before.get(r.fileId as number) as { fp: string | null; renameOf: number | null; untouched: number } | undefined;
    const n = Number(
      st.run(
        fp,
        probe,
        r.probeVersion as number,
        r.state as string,
        (r.code as string | null | undefined) ?? null,
        kind,
        (r.durationMs as number | null | undefined) ?? null,
        (r.width as number | null | undefined) ?? null,
        (r.height as number | null | undefined) ?? null,
        r.fileId as number,
        r.size as number,
        BigInt(r.mtimeNs as string),
      ).changes,
    );
    updated += n;
    if (n !== 1 || fp === null || prior === undefined || prior.fp !== null || prior.renameOf !== null || prior.untouched !== 1) return;
    const from = m.moveFrom.get(fp, r.fileId as number) as { id: number } | undefined;
    if (from !== undefined && mergeInto(m.diff, m.fts, from.id, r.fileId as number, false)) moved.push({ from: r.fileId as number, to: from.id });
  });
  drain(settleHardlinks(m.diff, unlimited()));
  return { updated, stale: rows.length - updated, moved };
}

const HASH = /^blake3:[0-9a-f]{64}$/;

/** `applyHashes {hashes ≤ 1000}`: `{fileId, size, mtimeNs, fullHash}`, kept only if the row is unchanged. */
function applyHashes(db: DatabaseSync, args: unknown): { updated: number; stale: number } {
  const rows = list("applyHashes", args, "hashes", 1000);
  const st = db.prepare("UPDATE files SET fullHash = ? WHERE id = ? AND size = ? AND mtimeNs = ?");
  let updated = 0;
  rows.forEach((r, i) => {
    if (!isInt(r.fileId) || !isInt(r.size) || !isStr(r.mtimeNs) || !I64.test(r.mtimeNs) || !isStr(r.fullHash) || !HASH.test(r.fullHash)) {
      throw bad("applyHashes", `hashes[${i}]`);
    }
    updated += Number(st.run(r.fullHash, r.fileId, r.size, BigInt(r.mtimeNs)).changes);
  });
  return { updated, stale: rows.length - updated };
}

const STAGE_STATES = new Set(["queued", "running", "done", "failed", "stale"]);

/**
 * `stagesSet {rows ≤ 2000}`: upserts `{fp, stage, version, state, code?, jobId?, attempts?,
 * at}` keyed `(fp, stage)`; a row without `attempts` keeps the stored count.
 */
function stagesSet(db: DatabaseSync, args: unknown): { set: number } {
  const rows = list("stagesSet", args, "rows", 2000);
  const st = db.prepare(
    "INSERT INTO stages (fp, stage, version, state, code, jobId, attempts, at) VALUES (?, ?, ?, ?, ?, ?, coalesce(?, 0), ?) " +
      "ON CONFLICT (fp, stage) DO UPDATE SET version = excluded.version, state = excluded.state, code = excluded.code, " +
      "jobId = excluded.jobId, attempts = coalesce(?, attempts), at = excluded.at",
  );
  rows.forEach((r, i) => {
    const okRow =
      isStr(r.fp) && isStr(r.stage) && isStr(r.version) && isStr(r.state) && STAGE_STATES.has(r.state) && optStr(r.code) &&
      optStr(r.jobId) && optInt(r.attempts) && isInt(r.at);
    if (!okRow) throw bad("stagesSet", `rows[${i}]`);
    const attempts = (r.attempts as number | null | undefined) ?? null;
    const code = (r.code as string | null | undefined) ?? null;
    const jobId = (r.jobId as string | null | undefined) ?? null;
    st.run(r.fp as string, r.stage as string, r.version as string, r.state as string, code, jobId, attempts, r.at as number, attempts);
  });
  return { set: rows.length };
}

const MODES = new Set(["full", "dirs", "poll"]);
const TRIGGERS = new Set(["add", "schedule", "demand", "watch", "mount"]);

/** `finishScan {jobId?, mode, trigger, gen, startedAt, finishedAt, outcome, code?, counts?}` → `{scanId}`. */
function finishScan(db: DatabaseSync, args: unknown): { scanId: number } {
  const a = args as Record<string, unknown>;
  const okArgs =
    isObj(args) && optStr(a.jobId) && isStr(a.mode) && MODES.has(a.mode) && isStr(a.trigger) && TRIGGERS.has(a.trigger) &&
    isInt(a.gen) && isInt(a.startedAt) && isInt(a.finishedAt) && isStr(a.outcome) && optStr(a.code) &&
    (a.counts === undefined || isObj(a.counts));
  if (!okArgs) throw bad("finishScan", "args");
  const row = db
    .prepare(
      "INSERT INTO scans (jobId, mode, trigger, gen, startedAt, finishedAt, outcome, code, counts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .get(
      (a.jobId as string | null | undefined) ?? null,
      a.mode as string,
      a.trigger as string,
      a.gen as number,
      a.startedAt as number,
      a.finishedAt as number,
      a.outcome as string,
      (a.code as string | null | undefined) ?? null,
      a.counts === undefined ? null : JSON.stringify(a.counts),
    ) as { id: number };
  return { scanId: row.id };
}

export interface PendingFile {
  readonly fileId: number;
  readonly rel: { components: string[] };
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileIdBytes: string | null;
  readonly fp: string | null;
  readonly ext: string | null;
  readonly state: string;
}

/**
 * `pending {stage, limit ≤ 1000, …}`: the next files a stage would take, oldest change first.
 * - `probe` (with `probeVersion`, and `exts`: the library's `kinds` extensions, or `null` for
 *   all): present files never probed at this version;
 * - any other stage (with `version`, `maxAttempts`, default 3): present, fingerprinted files
 *   without a row at this version, or failed with attempts left, one per fingerprint.
 * The stage pump (Task 11) owns dependencies, in-flight sets and retry times.
 */
function pending(db: DatabaseSync, args: unknown): { files: PendingFile[] } {
  const a = args as Record<string, unknown>;
  if (!isObj(args) || !isStr(a.stage) || !isInt(a.limit) || a.limit < 1 || a.limit > 1000) throw bad("pending", "args");
  let rows: Array<Record<string, unknown>>;
  const cols = "f.id, f.dirId, f.name, f.size, CAST(f.mtimeNs AS TEXT) AS mtimeNs, f.fileId, f.fp, f.ext, f.state";
  if (a.stage === "probe") {
    if (!isInt(a.probeVersion)) throw bad("pending", "probeVersion");
    const exts = a.exts;
    if (exts !== null && (!Array.isArray(exts) || !exts.every(isStr))) throw bad("pending", "exts");
    const filter = exts === null ? "" : `AND f.ext IN (SELECT value FROM json_each(?))`;
    const sql =
      `SELECT ${cols} FROM files f WHERE f.state = 'present' AND (f.probeVersion IS NULL OR f.probeVersion < ?) ${filter} ` +
      "ORDER BY f.lastChange, f.id LIMIT ?";
    const params: Array<number | string> = [a.probeVersion];
    if (exts !== null) params.push(JSON.stringify(exts));
    params.push(a.limit);
    rows = db.prepare(sql).all(...params) as Array<Record<string, unknown>>;
  } else {
    if (!isStr(a.version) || !optInt(a.maxAttempts)) throw bad("pending", "version");
    const max = (a.maxAttempts as number | undefined) ?? 3;
    rows = db
      .prepare(
        `SELECT ${cols} FROM files f LEFT JOIN stages s ON s.fp = f.fp AND s.stage = ? ` +
          "WHERE f.state = 'present' AND f.fp IS NOT NULL AND (s.fp IS NULL OR s.version != ? OR (s.state = 'failed' AND s.attempts < ?)) " +
          "AND f.id = (SELECT min(g.id) FROM files g WHERE g.fp = f.fp AND g.state = 'present') ORDER BY f.lastChange, f.id LIMIT ?",
      )
      .all(a.stage, a.version, max, a.limit) as Array<Record<string, unknown>>;
  }
  const chains = new DirChains(db);
  const files = rows.map((r) => ({
    fileId: r.id as number,
    rel: chains.file(r.dirId as number, r.name as Uint8Array),
    size: r.size as number,
    mtimeNs: r.mtimeNs as string,
    fileIdBytes: r.fileId === null ? null : Buffer.from(r.fileId as Uint8Array).toString("base64"),
    fp: (r.fp as string | null) ?? null,
    ext: (r.ext as string | null) ?? null,
    state: r.state as string,
  }));
  return { files };
}

/**
 * `acceptUnmounted {dirs?: [{components}]}` → `{dirs}`: the owner removed nested mounts for good
 * (Decision 17). Each offline mount point (`ROOT_UNMOUNTED`/`ROOT_VOLUME_CHANGED`; all of them,
 * or the ones named) forgets that it was a mount and which volume it held, and its chain is
 * answered: the service lists those directories, and their own complete listings bring them
 * back like any directory (what is gone goes missing, after the usual empty confirmation).
 */
function acceptUnmounted(db: DatabaseSync, args: unknown): { dirs: Array<{ components: string[] }> } {
  const a = args as Record<string, unknown>;
  if (!isObj(args)) throw bad("acceptUnmounted", "args");
  let only: Set<string> | undefined;
  if (a.dirs !== undefined) {
    if (!Array.isArray(a.dirs) || a.dirs.length > 1000) throw bad("acceptUnmounted", "dirs");
    only = new Set(
      a.dirs.map((d, i) => {
        const c = isObj(d) ? d.components : undefined;
        if (!Array.isArray(c) || !c.every(isStr)) throw bad("acceptUnmounted", `dirs[${i}]`);
        return c.join("/");
      }),
    );
  }
  const chains = new DirChains(db);
  const rows = db
    .prepare("SELECT id FROM dirs WHERE state = 'offline' AND mount IS NOT NULL AND code IN ('ROOT_UNMOUNTED', 'ROOT_VOLUME_CHANGED') ORDER BY id")
    .all() as Array<{ id: number }>;
  const clear = db.prepare("UPDATE dirs SET mount = NULL, volume = NULL WHERE id = ?");
  const dirs: Array<{ components: string[] }> = [];
  for (const r of rows) {
    const components = chains.of(r.id);
    if (only !== undefined && !only.has(components.join("/"))) continue;
    clear.run(r.id);
    dirs.push({ components });
  }
  return { dirs };
}

export const coreOps = { applyProbed, applyHashes, stagesSet, finishScan, pending, acceptUnmounted };
