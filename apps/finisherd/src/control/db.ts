// `<server-data>/control.db`: the control plane's own SQLite database (plan 5A Decisions 1–3,
// spec §3.1). It holds the environment id, the server-id map and the audit log (chain `core`),
// and each later area's tables in its own migration chain (`migrations.ts`).
//
// `node:sqlite` APIs used, all present in Node 24.22 (the repository's Node and the one Electron 44/45 embed):
// `new DatabaseSync(path)`, `exec`, `prepare` with `run`/`get`/`all`, `close`, and the
// `errcode` property of its errors (with a message fallback). PRAGMAs set the busy timeout and
// the other options rather than constructor options, so nothing depends on a newer `open` API.
//
// The event loop: `finisherd`'s main thread is the only writer. Its connection never
// checkpoints (`wal_autocheckpoint = 0`); `checkpoint-worker.ts` does that on its own thread
// and connection. Statements are prepared once by their owners and kept small (p99 ≤ 1 ms);
// bulk deletes go through `deleteInBatches`, ≤ 500 rows per statement, one statement per turn.
import { chmodSync, closeSync, existsSync, openSync, renameSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync, type StatementSync } from "node:sqlite";
import { Worker } from "node:worker_threads";
import { FinError } from "@finisher/contracts";
import { Context, Effect, Layer } from "effect";
import { ServerStartError, startFailure } from "../config.ts";
import { type ChainRegistry, CONTROL_CHAINS, applyMigrations, planMigrations, registerChain } from "./migrations.ts";
import { core0001 } from "./migrations/core/0001.ts";
import { ulid } from "./ulid.ts";

/** The `core` chain's format this code writes. */
export const CONTROL_FORMAT_VERSION = 1;
/** The oldest reader of the `core` chain that can use what this code writes. */
export const CONTROL_MIN_READER_VERSION = 1;

registerChain("core", [core0001], { formatVersion: CONTROL_FORMAT_VERSION, minReaderVersion: CONTROL_MIN_READER_VERSION });

export interface ControlStore {
  readonly db: DatabaseSync;
  /** The environment's id: minted once, renewed only by `renewEnv` (5B `env.reset`). */
  readonly envId: string;
  /** This start's `server-id`. */
  readonly serverId: string;
  /**
   * Writes a new `envId` (5B Decision 31, `env.reset`: the data is a copy, so it becomes another
   * environment) to `meta` and maps this `server-id` to it in `server_ids`; the other server ids
   * keep the old one. It writes inside the caller's transaction (`tx`); `useEnvId` switches what
   * `envId` reads once that transaction committed.
   */
  renewEnv(next: string): void;
  useEnvId(next: string): void;
  /**
   * Runs `f` in `BEGIN IMMEDIATE … COMMIT`, rolling back if it throws (nested calls use a
   * savepoint). A lock held past the 250 ms busy timeout throws `FinError CONTROL_DB_BUSY`.
   */
  tx<A>(f: () => A): A;
  /**
   * Deletes in batches without holding the event loop: `sql` is a `DELETE` whose **last**
   * placeholder is the batch size, e.g.
   * `DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE at < ? ORDER BY id LIMIT ?)`.
   * It runs with `[...params, batch]` (batch ≤ 500) once per event-loop turn until a run
   * deletes fewer than `batch` rows, and succeeds with the total deleted.
   */
  deleteInBatches(sql: string, params: unknown[], batch?: number): Effect.Effect<number>;
  /** Stops the checkpoint worker, runs a final `TRUNCATE` checkpoint and closes. Idempotent. */
  close(): Promise<void>;
}

/** Checkpoint worker timings (tests shorten them). */
export interface CheckpointOptions {
  /** `PASSIVE` this often while there are writes (default 30 s). */
  readonly passiveMs?: number;
  /** `PASSIVE` at once when the WAL passes this size (default 4 MiB). */
  readonly walLimitBytes?: number;
  /** `TRUNCATE` after this long without writes (default 60 s). */
  readonly idleTruncateMs?: number;
  /** How often the worker looks (default 1 s). */
  readonly pollMs?: number;
}

export interface OpenControlOptions {
  /** This server data's current `server-id` (fin-node `serverId`), recorded in `server_ids`. */
  readonly serverId: string;
  /** The chains to run; default every `registerChain`ed one. */
  readonly chains?: ChainRegistry;
  readonly checkpoint?: CheckpointOptions | false;
  readonly clock?: () => number;
}

export const CONTROL_DB = "control.db";
const BUSY_MS = 250;
const MAX_BATCH = 500;

export function controlDbPath(serverData: string): string {
  return join(serverData, CONTROL_DB);
}

const SQLITE_BUSY = 5;
const SQLITE_LOCKED = 6;
const SQLITE_NOTADB = 26;

function sqliteCode(e: unknown): number | undefined {
  const code = (e as { errcode?: unknown } | null)?.errcode;
  return typeof code === "number" ? code & 0xff : undefined;
}

/** True for `SQLITE_BUSY`/`SQLITE_LOCKED`. */
export function isBusy(e: unknown): boolean {
  const code = sqliteCode(e);
  if (code !== undefined) return code === SQLITE_BUSY || code === SQLITE_LOCKED;
  return /database is locked|database table is locked/i.test(String((e as Error | null)?.message ?? e));
}

/** Not a SQLite database at all (`SQLITE_NOTADB`): the one thrown error that resets the store. */
function isNotADatabase(e: unknown): boolean {
  const code = sqliteCode(e);
  if (code !== undefined) return code === SQLITE_NOTADB;
  return /file is not a database/i.test(String((e as Error | null)?.message ?? e));
}

/**
 * A schema this SQLite cannot parse: most likely written by a newer SQLite (a newer
 * finisherd's Node), not damage, so the start is refused and nothing is reset.
 */
function isUnreadableSchema(e: unknown): boolean {
  return /malformed database schema/i.test(String((e as Error | null)?.message ?? e));
}

export function controlBusy(e: unknown): FinError {
  return new FinError({
    code: "CONTROL_DB_BUSY",
    message: "the control database is locked by another connection",
    details: { cause: String((e as Error | null)?.message ?? e) },
  });
}

class Corrupt extends Error {}

/** Opens a connection with the store's pragmas; `quick_check` must answer `ok`. */
function connect(path: string): DatabaseSync {
  const db = new DatabaseSync(path);
  try {
    db.exec(`PRAGMA busy_timeout = ${BUSY_MS}`);
    const rows = db.prepare("PRAGMA quick_check").all() as Array<Record<string, unknown>>;
    const ok = rows.length === 1 && Object.values(rows[0] ?? {})[0] === "ok";
    if (!ok) throw new Corrupt(`quick_check: ${JSON.stringify(rows.slice(0, 3))}`);
    const mode = db.prepare("PRAGMA journal_mode = WAL").get() as { journal_mode: string };
    if (mode.journal_mode !== "wal") throw new Error(`journal_mode is ${mode.journal_mode}, not wal`);
    db.exec("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA wal_autocheckpoint = 0;");
    return db;
  } catch (e) {
    db.close();
    throw e;
  }
}

/** Creates the file 0600 before SQLite does (its `-wal`/`-shm` files copy the mode). */
function createPrivate(path: string): void {
  closeSync(openSync(path, "a", 0o600));
  if (process.platform !== "win32") chmodSync(path, 0o600);
}

const SIDE_FILES = ["", "-wal", "-shm"] as const;

/** Moves a damaged database aside as `control.db.corrupt-<ulid>` (plus its `-wal`/`-shm`). */
function moveAside(path: string, now: number): string {
  const aside = `${path}.corrupt-${ulid(now)}`;
  for (const suffix of SIDE_FILES) if (existsSync(path + suffix)) renameSync(path + suffix, aside + suffix);
  return aside;
}

/** `VACUUM INTO control.db.pre-migrate.bak`; the previous one becomes `.bak.1` (two kept). */
function backupBeforeMigrating(db: DatabaseSync, path: string): void {
  const bak = `${path}.pre-migrate.bak`;
  if (existsSync(bak)) renameSync(bak, `${bak}.1`);
  db.prepare("VACUUM INTO ?").run(bak);
  if (process.platform !== "win32") chmodSync(bak, 0o600);
}

function tooNew(tooNew: ReturnType<typeof planMigrations>["tooNew"]): ServerStartError {
  const first = tooNew[0];
  return new ServerStartError(
    "CONTROL_DB_TOO_NEW",
    `control.db chain ${first?.chain} needs reader version ${first?.minReaderVersion}; this finisherd reads ${first?.readerVersion}`,
    { chains: tooNew },
  );
}

/**
 * Opens (creating, migrating or resetting) `<server-data>/control.db` (Decisions 1–3):
 * `quick_check` at open (a damaged file is moved aside and the store starts empty with a new
 * `envId` and `audit {kind: "control.reset"}` as its first row); every chain's
 * `minReaderVersion` checked (`CONTROL_DB_TOO_NEW`); pending steps run in one transaction after
 * a `VACUUM INTO` backup; the `envId` minted once; `serverId` recorded in `server_ids`.
 * Throws `ServerStartError` (`CONTROL_DB_TOO_NEW`, `CONTROL_DB_BUSY`, `SERVER_START_FAILED`).
 */
export function openControlStore(serverData: string, opts: OpenControlOptions): ControlStore {
  const clock = opts.clock ?? Date.now;
  const path = controlDbPath(serverData);
  let reset: { aside: string; reason: "quickCheck" | "notADatabase" } | undefined;
  let db: DatabaseSync;
  try {
    createPrivate(path);
    try {
      db = connect(path);
    } catch (e) {
      // Reset only for damage `quick_check` reports, or a file that is not SQLite at all.
      if (!(e instanceof Corrupt) && !isNotADatabase(e)) throw e;
      reset = { aside: moveAside(path, clock()), reason: e instanceof Corrupt ? "quickCheck" : "notADatabase" };
      createPrivate(path);
      db = connect(path);
    }
  } catch (e) {
    if (isBusy(e)) throw new ServerStartError("CONTROL_DB_BUSY", "control.db is locked", { cause: String(e) });
    if (isUnreadableSchema(e)) {
      throw new ServerStartError("CONTROL_DB_TOO_NEW", "control.db has a schema this finisherd's SQLite cannot read", {
        cause: String((e as Error).message),
      });
    }
    throw startFailure(e);
  }
  try {
    const plan = planMigrations(db, opts.chains ?? CONTROL_CHAINS);
    if (plan.tooNew.length > 0) throw tooNew(plan.tooNew);
    if (plan.pending.length > 0 && plan.existing) backupBeforeMigrating(db, path);
    const now = clock();
    let envId = "";
    runTx(db, () => {
      if (plan.pending.length > 0) applyMigrations(db, plan, now);
      const row = db.prepare("SELECT value FROM meta WHERE key = 'envId'").get() as { value: string } | undefined;
      envId = row?.value ?? ulid(now);
      if (row === undefined) {
        db.prepare("INSERT INTO meta (key, value) VALUES ('envId', ?), ('createdAt', ?)").run(envId, String(now));
      }
      if (reset !== undefined) {
        db.prepare(
          "INSERT INTO audit (at, kind, principalKind, principalId, outcome, details) VALUES (?, 'control.reset', 'system', 'finisherd', 'ok', ?)",
        ).run(now, JSON.stringify({ reason: reset.reason }));
      }
      db.prepare("INSERT OR IGNORE INTO server_ids (serverId, envId, since) VALUES (?, ?, ?)").run(opts.serverId, envId, now);
    });
    if (reset !== undefined) process.stderr.write(`finisherd: warning: control.db was damaged; moved to ${reset.aside} and started empty\n`);
    return makeStore(db, path, envId, opts.checkpoint, opts.serverId, clock);
  } catch (e) {
    db.close();
    if (e instanceof ServerStartError) throw e;
    if (e instanceof FinError) throw new ServerStartError(e.code, e.message, e.details as Record<string, unknown>);
    throw startFailure(e);
  }
}

/** Open transactions per connection (`isTransaction` is newer than some Node 24 builds' docs). */
const depth = new WeakMap<DatabaseSync, number>();

/** `BEGIN IMMEDIATE` … `COMMIT` (a savepoint when already in a transaction). */
function runTx<A>(db: DatabaseSync, f: () => A): A {
  const level = depth.get(db) ?? 0;
  if (level > 0) {
    const sp = `sp${level}`;
    db.exec(`SAVEPOINT ${sp}`);
    depth.set(db, level + 1);
    try {
      const a = f();
      db.exec(`RELEASE ${sp}`);
      return a;
    } catch (e) {
      db.exec(`ROLLBACK TO ${sp}; RELEASE ${sp}`);
      throw e;
    } finally {
      depth.set(db, level);
    }
  }
  try {
    db.exec("BEGIN IMMEDIATE");
  } catch (e) {
    throw isBusy(e) ? controlBusy(e) : e;
  }
  depth.set(db, 1);
  try {
    const a = f();
    db.exec("COMMIT");
    return a;
  } catch (e) {
    try {
      db.exec("ROLLBACK");
    } catch {
      // COMMIT failed after SQLite already rolled back.
    }
    throw isBusy(e) ? controlBusy(e) : e;
  } finally {
    depth.set(db, 0);
  }
}

/** One event-loop turn (not just a fiber yield), so I/O and timers run between batches. */
const nextTurn = Effect.promise(() => new Promise<void>((resolve) => setImmediate(resolve)));

function makeStore(
  db: DatabaseSync,
  path: string,
  firstEnvId: string,
  checkpoint: CheckpointOptions | false | undefined,
  serverId: string,
  clock: () => number,
): ControlStore {
  let envId = firstEnvId;
  const worker = checkpoint === false ? undefined : startCheckpointWorker(path, checkpoint ?? {});
  let closed: Promise<void> | undefined;
  // Prepared once per SQL text (the callers pass a few fixed statements).
  const statements = new Map<string, StatementSync>();
  return {
    db,
    get envId() {
      return envId;
    },
    serverId,
    renewEnv: (next) => {
      db.prepare("UPDATE meta SET value = ? WHERE key = 'envId'").run(next);
      db.prepare("INSERT INTO server_ids (serverId, envId, since) VALUES (?, ?, ?) ON CONFLICT (serverId) DO UPDATE SET envId = excluded.envId, since = excluded.since").run(
        serverId,
        next,
        clock(),
      );
    },
    useEnvId: (next) => {
      envId = next;
    },
    tx: (f) => runTx(db, f),
    deleteInBatches: (sql, params, batch = MAX_BATCH) =>
      Effect.suspend(() => {
        const size = Math.max(1, Math.min(MAX_BATCH, Math.floor(batch)));
        let stmt = statements.get(sql);
        if (stmt === undefined) {
          stmt = db.prepare(sql);
          statements.set(sql, stmt);
        }
        const loop = (total: number): Effect.Effect<number> =>
          Effect.flatMap(
            Effect.sync(() => Number(stmt.run(...(params as never[]), size as never).changes)),
            (n) => (n < size ? Effect.succeed(total + n) : Effect.flatMap(nextTurn, () => loop(total + n))),
          );
        return loop(0);
      }),
    close: () => {
      closed ??= (async () => {
        if (worker !== undefined) await worker.stop();
        try {
          db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
        } catch (e) {
          process.stderr.write(`finisherd: warning: final control.db checkpoint failed: ${String(e)}\n`);
        }
        db.close();
      })();
      return closed;
    },
  };
}

interface CheckpointWorker {
  stop(): Promise<void>;
}

function startCheckpointWorker(path: string, o: CheckpointOptions): CheckpointWorker {
  const worker = new Worker(new URL("./checkpoint-worker.ts", import.meta.url), {
    workerData: {
      path,
      busyMs: BUSY_MS,
      passiveMs: o.passiveMs ?? 30_000,
      walLimitBytes: o.walLimitBytes ?? 4 * 1024 * 1024,
      idleTruncateMs: o.idleTruncateMs ?? 60_000,
      pollMs: o.pollMs ?? 1000,
    },
  });
  worker.unref();
  worker.on("error", (e) => process.stderr.write(`finisherd: warning: control.db checkpoint worker failed: ${String(e)}\n`));
  const exited = new Promise<void>((resolve) => worker.once("exit", () => resolve()));
  return {
    stop: async () => {
      worker.postMessage("stop");
      const timer = setTimeout(() => void worker.terminate(), 2000);
      await exited;
      clearTimeout(timer);
    },
  };
}

export class ControlStoreTag extends Context.Service<ControlStoreTag, ControlStore>()("finisherd/ControlStore") {}

/** The store for the enclosing scope: opened at build, closed (final checkpoint) at release. */
export const layerControlStore = (serverData: string, serverId: string): Layer.Layer<ControlStoreTag, ServerStartError> =>
  Layer.effect(
    ControlStoreTag,
    Effect.acquireRelease(
      Effect.try({ try: () => openControlStore(serverData, { serverId }), catch: startFailure }),
      (store) => Effect.promise(() => store.close()),
    ),
  );
