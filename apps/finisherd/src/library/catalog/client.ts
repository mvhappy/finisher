// The main thread's side of the catalog threads (plan 5D Decisions 2, 24): `IndexerClient`
// (writes, in the `library-indexer` thread) and `ReaderClient` (reads, in `library-reader`),
// all `Effect`s over `MessagePort`s. The main thread never opens a catalog, never runs a
// statement and never reads a batch file: it posts ids, paths and plain data, checked by
// `assertCloneable` before posting, so no function crosses a port.
import { basename, dirname } from "node:path";
import { Worker } from "node:worker_threads";
import { FinError } from "@finisher/contracts";
import { Context, Effect, Layer } from "effect";
import type { FileRow, DirRow, FileLineRow } from "./core/queries.ts";
import type { SearchPage, SearchRequest } from "./core/snapshot.ts";
import type { PendingFile } from "./core/ops.ts";
import type { StagePendingFile, StageRow } from "./core/stage-rows.ts";
import type { ApplyCounts } from "./core/apply.ts";
import type { FileChanges } from "./core/file-events.ts";
import { guardedClock } from "./clock.ts";
import { CATALOG_EXTENSIONS, type CatalogExtensionRegistry } from "./extensions.ts";
import { CatalogFailure, type Ready, type Reply, type WireError, assertCloneable } from "./protocol.ts";
import type { WatchEvent } from "../watch.ts";

export interface CatalogThreadsOptions {
  /** `<server-data>`: catalogs live in `libraries/<id>/catalog.db`. */
  readonly serverData: string;
  /** The engine's jobs root: batch files and `in/` files must lie under it. */
  readonly jobsRoot: string;
  /** The extensions to load; default every `registerCatalogExtension`ed one. */
  readonly extensions?: CatalogExtensionRegistry;
  /** Rows per indexer transaction (≤ 2,000; tests lower it). */
  readonly txRows?: number;
  /** Default `guardedClock()`: the wall clock with forward jumps held off. */
  readonly clock?: () => number;
  /**
   * Called once when a catalog thread dies after start (it is not restarted: every pending
   * and later call fails with its `CATALOG_THREAD_EXITED`/`CATALOG_THREAD_FAILED`). Default: a
   * line on stderr. The service turns it into a fatal health event.
   */
  readonly onFatal?: (e: FinError) => void;
  /** Search snapshots (Decision 43): idle lifetime in ms and live count; tests lower them. */
  readonly snapshot?: { readonly ttlMs?: number; readonly max?: number };
  /** Enables `IndexerClient.testFault` and `watchFault` (tests only). */
  readonly testHooks?: boolean;
  /** Quiet time before a watch hint is posted (default 2 s, Decision 30). */
  readonly watchQuietMs?: number;
  /** `fs.inotify.max_user_watches` as the indexer's watches see it (tests). */
  readonly watchMaxUserWatches?: number;
}

/** A thread's health: `dead` once it exited or failed after start. */
export type CatalogHealth = { readonly ok: true } | { readonly ok: false; readonly error: FinError };

const toFin = (e: WireError): FinError => new FinError({ code: e.code, message: e.message, details: e.details });

/** One catalog thread and its request/reply bookkeeping. */
class Thread {
  readonly #worker: Worker;
  readonly #pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: FinError) => void }>();
  #next = 1;
  #dead: FinError | undefined;
  #started = false;
  #stopping = false;
  readonly posted = { count: 0 };
  readonly onFatal: (e: FinError) => void;
  /** Unsolicited `{event}` messages (the indexer's watch hints). */
  onEvent: (e: unknown) => void = () => {};

  private constructor(worker: Worker, onFatal: (e: FinError) => void) {
    this.#worker = worker;
    this.onFatal = onFatal;
  }

  get dead(): FinError | undefined {
    return this.#dead;
  }

  static start(file: string, name: string, workerData: unknown, onFatal: (e: FinError) => void): Promise<Thread> {
    const worker = new Worker(new URL(file, import.meta.url), { name, workerData });
    const thread = new Thread(worker, onFatal);
    return new Promise((resolve, reject) => {
      const fail = (e: FinError) => {
        const first = thread.#dead === undefined;
        thread.#die(e);
        if (thread.#started && first && !thread.#stopping) thread.onFatal(e);
        reject(e);
      };
      worker.once("message", (m: Ready) => {
        if (!m.ready) return fail(toFin(m.error));
        worker.on("message", (r: Reply | { event: unknown }) => ("event" in r ? thread.onEvent(r.event) : thread.#settle(r)));
        thread.#started = true;
        resolve(thread);
      });
      worker.on("error", (e) => fail(new FinError({ code: "CATALOG_THREAD_FAILED", message: `the ${name} thread failed`, details: { cause: String(e) } })));
      worker.once("exit", (code) =>
        fail(new FinError({ code: "CATALOG_THREAD_EXITED", message: `the ${name} thread exited`, details: { exitCode: code } })),
      );
    });
  }

  #settle(r: Reply): void {
    const p = this.#pending.get(r.id);
    if (p === undefined) return;
    this.#pending.delete(r.id);
    if (r.ok) p.resolve(r.value);
    else p.reject(toFin(r.error));
  }

  #die(e: FinError): void {
    this.#dead ??= e;
    for (const p of this.#pending.values()) p.reject(e);
    this.#pending.clear();
  }

  call(method: string, args: unknown): Promise<unknown> {
    if (this.#dead !== undefined) return Promise.reject(this.#dead);
    try {
      assertCloneable(args);
    } catch (e) {
      const f = e as CatalogFailure;
      return Promise.reject(new FinError({ code: f.code, message: f.message, details: f.details }));
    }
    const id = this.#next++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.posted.count++;
      this.#worker.postMessage({ id, method, args });
    });
  }

  async stop(): Promise<void> {
    if (this.#dead !== undefined) return;
    this.#stopping = true;
    const exited = new Promise<void>((resolve) => this.#worker.once("exit", () => resolve()));
    const timer = setTimeout(() => void this.#worker.terminate(), 5000);
    await this.call("shutdown", {}).catch(() => undefined);
    await exited;
    clearTimeout(timer);
  }
}

const fx = <A>(f: () => Promise<unknown>): Effect.Effect<A, FinError> =>
  Effect.tryPromise({
    try: async () => (await f()) as A,
    catch: (e) => (e instanceof FinError ? e : new FinError({ code: "INTERNAL", message: String(e), details: {} })),
  });

export interface OpenInfo {
  readonly path: string;
  readonly created: boolean;
  /** An unusable catalog was moved aside: the service answers with a full scan. */
  readonly rebuilt?: "corrupt" | "foreign";
}

export interface CatalogCounts {
  readonly files: Record<string, number>;
  readonly dirs: Record<string, number>;
  /** Live rows with collision bits (`NAME_COLLISION {count}`). */
  readonly collisions: number;
}

export interface ProbedApplied {
  readonly updated: number;
  readonly stale: number;
  /** `from`: the new row, deleted; `to`: the missing row that took its path and probe (it keeps its id). */
  readonly moved: ReadonlyArray<{ readonly from: number; readonly to: number }>;
  /** Every row that landed, by its id after the apply (a moved one by `to`), with its fingerprint. */
  readonly probed?: ReadonlyArray<{ readonly fileId: number; readonly fp: string | null }>;
}

/** Picks the one file a key names (Decision 11); see `ReaderClient.resolve`. */
export function pickMatch<R extends { readonly id: number; readonly state: string }>(matches: ReadonlyArray<R>): R {
  const live = matches.filter((m) => m.state !== "missing");
  const pick = live.length > 0 ? live : matches;
  if (pick.length === 1) return pick[0]!;
  if (pick.length === 0) throw new FinError({ code: "FILE_NOT_FOUND", message: "no file in the library has this path", details: {} });
  throw new FinError({
    code: "PATH_AMBIGUOUS",
    message: "several files in the library have this path; address one by its file id",
    details: { fileIds: pick.map((m) => m.id) },
  });
}

export interface BatchApplied extends ApplyCounts {
  readonly transactions: number;
  /** The rows the batch file changed, by what each became (`Libraries.onFile`, Task 11). */
  readonly files?: FileChanges;
}

export interface ScanRecord {
  readonly jobId?: string;
  readonly mode: "full" | "dirs" | "poll";
  readonly trigger: "add" | "schedule" | "demand" | "watch" | "mount";
  readonly gen: number;
  readonly startedAt: number;
  readonly finishedAt: number;
  readonly outcome: string;
  readonly code?: string;
  readonly counts?: Record<string, number>;
}

export interface ProbedRow {
  readonly fileId: number;
  readonly size: number;
  readonly mtimeNs: string;
  readonly fp?: string | null;
  readonly probe?: unknown;
  readonly probeVersion: number;
  readonly state: "present" | "probeFailed" | "skipped";
  readonly code?: string | null;
  readonly kind?: string | null;
  readonly durationMs?: number | null;
  readonly width?: number | null;
  readonly height?: number | null;
}

export interface StageRowSet {
  readonly fp: string;
  readonly stage: string;
  readonly version: string;
  readonly state: "queued" | "running" | "done" | "failed" | "stale" | "skipped";
  readonly code?: string | null;
  readonly jobId?: string | null;
  readonly attempts?: number | null;
  readonly at: number;
}

export type PendingArgs =
  | { readonly stage: "probe"; readonly probeVersion: number; readonly exts: ReadonlyArray<string> | null; readonly limit: number }
  | {
      readonly stage: string;
      readonly version: string;
      readonly maxAttempts?: number;
      readonly limit: number;
      /** Each must be `done` at its version. */
      readonly dependsOn?: ReadonlyArray<{ readonly stage: string; readonly version: string }>;
      /** Also `cloudOnly` files (a hydrating library). */
      readonly hydrate?: boolean;
      /** Failed rows only when they failed no later than this (the pump's retry time). */
      readonly retryBefore?: number;
      /** Only these fingerprints (≤ 256). */
      readonly fps?: ReadonlyArray<string>;
    };

export interface IndexerClient {
  /** Runs `ext`'s op `op` in the indexer, inside one transaction. */
  exec(lib: string, ext: string, op: string, args: unknown): Effect.Effect<unknown, FinError>;
  /** Opens (creating, migrating or rebuilding) the catalog, then attaches the reader to it. */
  open(lib: string): Effect.Effect<OpenInfo, FinError>;
  /** Detaches the reader, then closes the catalog. */
  close(lib: string): Effect.Effect<void, FinError>;
  /**
   * A job attempt's start in catalog time: strictly after every write so far. Take it before
   * the attempt reads anything (the scan kind's `prepare`, each attempt) and pass it as
   * `startedAt` with every batch file of the attempt.
   */
  attemptStart(lib: string): Effect.Effect<number, FinError>;
  /**
   * Reads, validates and applies a scan batch file at `path` (a plain name in a job's `out/`).
   * `startedAt`: the attempt's `attemptStart` (a wall-clock attempt start works too, but a
   * clock jump then makes the diff mark less until the clock catches up).
   */
  applyBatchFile(lib: string, path: string, job: BatchJob): Effect.Effect<BatchApplied, FinError>;
  finishScan(lib: string, scan: ScanRecord): Effect.Effect<{ scanId: number }, FinError>;
  /** Offline nested mount points the owner removed for good forget their mount (all, or `dirs`); answers their chains to list. */
  acceptUnmounted(lib: string, dirs?: ReadonlyArray<{ readonly components: ReadonlyArray<string> }>): Effect.Effect<{ dirs: Array<{ components: string[] }> }, FinError>;
  /** Lands probe results; `moved`: new rows merged into the missing row of their fingerprint. */
  applyProbed(lib: string, results: ReadonlyArray<ProbedRow>): Effect.Effect<ProbedApplied, FinError>;
  applyHashes(
    lib: string,
    hashes: ReadonlyArray<{ fileId: number; size: number; mtimeNs: string; fullHash: string }>,
  ): Effect.Effect<{ updated: number; stale: number }, FinError>;
  stagesSet(lib: string, rows: ReadonlyArray<StageRowSet>): Effect.Effect<{ set: number }, FinError>;
  /** Writes the job's `in/dirs.ndjson` (Decision 27) at `path`. */
  writeKnownDirs(lib: string, gen: number, path: string): Effect.Effect<{ dirs: number }, FinError>;
  /** The text of a job's `in/dirs.ndjson` (Decision 27), for a `prepare` that writes it through `ctx.open`. */
  knownDirs(lib: string, gen: number): Effect.Effect<{ text: string; dirs: number }, FinError>;
  pending(lib: string, args: PendingArgs): Effect.Effect<{ files: PendingFile[] }, FinError>;
  /** A stage's next files (`pendingStage`). */
  pendingStage(lib: string, args: Exclude<PendingArgs, { readonly stage: "probe" }>): Effect.Effect<{ files: StagePendingFile[] }, FinError>;
  /** A stage's rows at other versions become `stale` (Decision 42). */
  stagesStale(lib: string, stage: string, version: string): Effect.Effect<{ stale: number }, FinError>;
  /**
   * Deletes rows missing since before `before` and marked missing in a generation before
   * `beforeGen` (both: a clock jump alone purges nothing), ≤ 500 per transaction.
   */
  purgeMissing(lib: string, cutoff: { before: number; beforeGen: number }): Effect.Effect<{ deleted: number; dirs: number }, FinError>;
  /**
   * Marks the library offline or online for the diff (persisted in the catalog): while offline
   * no listing marks anything missing (Decision 18).
   */
  setOffline(lib: string, offline: boolean): Effect.Effect<{ offline: boolean }, FinError>;
  /**
   * Turns the mass-drop guard on (an accepted volume) or off (a full scan succeeded, or the
   * owner confirmed): while on, a root listing that would mark more than half of the root's
   * directories missing fails with `ROOT_DROP_SUSPECT` and marks nothing.
   */
  setDropGuard(lib: string, on: boolean): Effect.Effect<{ on: boolean }, FinError>;
  /**
   * Watches a local root in the indexer thread (Decision 30; the catalog must be open). Hints
   * and degradations arrive through `Catalogs.onWatch`. `{ok: false, reason}`: the OS refused.
   */
  watch(lib: string, root: string): Effect.Effect<{ ok: true } | { ok: false; reason: string }, FinError>;
  unwatch(lib: string): Effect.Effect<void, FinError>;
  /** Tests (`testHooks`): the library's watcher fails as on an overflow. */
  watchFault(lib: string): Effect.Effect<{ faulted: boolean }, FinError>;
  /** Tests (`testHooks`): the library's next batch file fails in its `transaction`-th transaction, after its writes. */
  testFault(lib: string, transaction: number): Effect.Effect<void, FinError>;
  /** `maxTxRows`: batch rows in one transaction; `maxStepRows`: rows written between two event-loop turns. */
  stats(): Effect.Effect<{ transactions: number; maxTxRows: number; maxStepRows: number; batchFiles: number; open: number }, FinError>;
}

/** One batch file's job: its generation, its attempt's start and (optionally) its `seq`. */
export interface BatchJob {
  readonly gen: number;
  readonly startedAt: number;
  readonly seq?: number;
}

export interface ReaderClient {
  /** Runs `ext`'s query `name` in the reader, on its read-only connection. */
  query(lib: string, ext: string, name: string, args: unknown): Effect.Effect<unknown, FinError>;
  files(lib: string, args: Record<string, unknown>): Effect.Effect<{ dir?: DirRow; matches?: DirRow[]; dirs: DirRow[]; files: FileRow[]; next?: { nameNfc: string; id: number } }, FinError>;
  file(
    lib: string,
    ref: { readonly fileId: number } | { readonly relKey: string },
  ): Effect.Effect<{ matches: Array<FileRow & { rel: { components: string[] }; display: string; dev: string | null }>; stages: unknown[] }, FinError>;
  search(lib: string, args: Record<string, unknown>): Effect.Effect<{ hits: Array<FileRow & { score: number }> }, FinError>;
  lookup(lib: string, args: { fingerprints?: string[]; hashes?: string[] }): Effect.Effect<{ matches: Array<{ key: string; display: string; file: FileRow }> }, FinError>;
  /**
   * Present rows without a full hash whose size is one of `sizes` (≤ 1,000 sizes), at most
   * `limit` (≤ 1,000) ids, lowest first: what `library.lookup {hashCandidates}` hashes.
   */
  hashCandidates(lib: string, args: { sizes: number[]; limit: number }): Effect.Effect<{ fileIds: number[]; more: boolean }, FinError>;
  /**
   * One page of a name search across `libraries` (Decision 43): with terms, the first page
   * snapshots up to 2,000 ranked hits in the reader and `next` pages that snapshot; without
   * terms, a score-free keyset by `lastChange`. `args` are the query and filters
   * (`{query?, kind?, state?, since?, durationMs?, width?, height?, size?}`) and must be the
   * same on every page of one search. Failures: `CURSOR_EXPIRED`, `CURSOR_INVALID`,
   * `SEARCH_QUERY_INVALID`, `CATALOG_NOT_OPEN`.
   */
  searchPage(req: SearchRequest): Effect.Effect<SearchPage, FinError>;
  counts(lib: string): Effect.Effect<CatalogCounts, FinError>;
  /**
   * The `in/files.ndjson` lines of a probe or hash job (Decision 20): raw chain and `Expect`
   * (with `dev`) of each readable row among `fileIds` (≤ 1,000); `absent` the others.
   */
  fileLines(lib: string, args: { fileIds: number[]; hydrate: boolean }): Effect.Effect<{ lines: FileLineRow[]; absent: number[] }, FinError>;
  /**
   * The one file at `relKey` (live rows first, then missing ones): several answer
   * `PATH_AMBIGUOUS` with `details.fileIds`, none `FILE_NOT_FOUND` (Decision 11).
   */
  resolve(lib: string, relKey: string): Effect.Effect<FileRow & { rel: { components: string[] } }, FinError>;
  stageCounts(lib: string): Effect.Effect<{ stages: Array<{ stage: string; version: string; state: string; n: number }> }, FinError>;
  /** Stage rows by fingerprint, job, state or stage (`stageRows`). */
  stageRows(
    lib: string,
    args: { readonly fps?: ReadonlyArray<string>; readonly jobIds?: ReadonlyArray<string>; readonly states?: ReadonlyArray<string>; readonly stage?: string; readonly limit?: number },
  ): Effect.Effect<{ rows: StageRow[] }, FinError>;
  stats(): Effect.Effect<{ queries: number; searches: number; open: number; snapshots: number }, FinError>;
}

export interface Catalogs {
  readonly indexer: IndexerClient;
  readonly reader: ReaderClient;
  /** Messages posted to each thread (tests). */
  readonly posted: { readonly indexer: { count: number }; readonly reader: { count: number } };
  /** Dead once either thread exited or failed after start (it is never restarted silently). */
  health(): CatalogHealth;
  /** Watch hints and degradations from the indexer thread (one listener; the service). */
  onWatch(listener: (e: WatchEvent) => void): void;
  /** Closes every catalog and stops both threads. Idempotent. */
  shutdown(): Promise<void>;
}

const logFatal = (e: FinError): void => {
  process.stderr.write(`finisherd: catalog thread died: ${e.code} ${JSON.stringify(e.details)}\n`);
};

/** Starts both threads; resolves once each has imported every extension. */
export async function startCatalogThreads(opts: CatalogThreadsOptions): Promise<Catalogs> {
  const clock = opts.clock ?? guardedClock();
  const extensions = (opts.extensions ?? CATALOG_EXTENSIONS).start();
  let fatal = false;
  const onFatal = (e: FinError) => {
    if (fatal) return;
    fatal = true;
    (opts.onFatal ?? logFatal)(e);
  };
  const [ix, rd] = await Promise.allSettled([
    Thread.start(
      "./indexer-thread.ts",
      "library-indexer",
      {
        extensions,
        serverData: opts.serverData,
        jobsRoot: opts.jobsRoot,
        ...(opts.txRows === undefined ? {} : { txRows: opts.txRows }),
        ...(opts.testHooks === true ? { testHooks: true } : {}),
        ...(opts.watchQuietMs === undefined ? {} : { watchQuietMs: opts.watchQuietMs }),
        ...(opts.watchMaxUserWatches === undefined ? {} : { watchMaxUserWatches: opts.watchMaxUserWatches }),
      },
      onFatal,
    ),
    Thread.start("./reader-thread.ts", "library-reader", { extensions, ...(opts.snapshot?.ttlMs === undefined ? {} : { snapshotTtlMs: opts.snapshot.ttlMs }), ...(opts.snapshot?.max === undefined ? {} : { snapshotMax: opts.snapshot.max }) }, onFatal),
  ]);
  if (ix.status === "rejected" || rd.status === "rejected") {
    if (ix.status === "fulfilled") await ix.value.stop();
    if (rd.status === "fulfilled") await rd.value.stop();
    throw ix.status === "rejected" ? ix.reason : (rd as PromiseRejectedResult).reason;
  }
  const indexer = ix.value;
  const reader = rd.value;
  const core = <A>(lib: string, op: string, args: unknown) => fx<A>(() => indexer.call("exec", { lib, ext: "core", op, args }));
  const read = <A>(lib: string, query: string, args: unknown) => fx<A>(() => reader.call("query", { lib, ext: "core", query, args }));
  const indexerClient: IndexerClient = {
    exec: (lib, ext, op, args) => fx(() => indexer.call("exec", { lib, ext, op, args })),
    open: (lib) =>
      fx<OpenInfo>(async () => {
        const info = (await indexer.call("open", { lib, at: clock() })) as OpenInfo;
        await reader.call("attach", { lib, path: info.path });
        return info;
      }),
    close: (lib) =>
      fx<void>(async () => {
        await reader.call("detach", { lib });
        await indexer.call("close", { lib });
      }),
    attemptStart: (lib) => fx<number>(async () => ((await indexer.call("attemptStart", { lib, at: clock() })) as { startedAt: number }).startedAt),
    applyBatchFile: (lib, path, job) =>
      fx(() =>
        indexer.call("applyBatchFile", {
          lib, file: { outDir: dirname(path), file: basename(path) }, gen: job.gen, startedAt: job.startedAt, at: clock(),
          ...(job.seq === undefined ? {} : { seq: job.seq }),
        }),
      ),
    finishScan: (lib, scan) => core(lib, "finishScan", scan),
    acceptUnmounted: (lib, dirs) => core(lib, "acceptUnmounted", dirs === undefined ? {} : { dirs }),
    applyProbed: (lib, results) => core(lib, "applyProbed", { results }),
    applyHashes: (lib, hashes) => core(lib, "applyHashes", { hashes }),
    stagesSet: (lib, rows) => core(lib, "stagesSet", { rows }),
    writeKnownDirs: (lib, gen, path) => fx(() => indexer.call("writeKnownDirs", { lib, gen, path })),
    knownDirs: (lib, gen) => fx(() => indexer.call("knownDirs", { lib, gen })),
    pending: (lib, args) => core(lib, "pending", args),
    pendingStage: (lib, args) => core(lib, "pending", args),
    stagesStale: (lib, stage, version) => core(lib, "stagesStale", { stage, version }),
    purgeMissing: (lib, cutoff) => fx(() => indexer.call("purgeMissing", { lib, before: cutoff.before, beforeGen: cutoff.beforeGen })),
    testFault: (lib, transaction) => fx<void>(async () => void (await indexer.call("testFault", { lib, transaction }))),
    setOffline: (lib, offline) => fx(() => indexer.call("setOffline", { lib, offline })),
    setDropGuard: (lib, on) => fx(() => indexer.call("setDropGuard", { lib, on })),
    watch: (lib, root) => fx(() => indexer.call("watch", { lib, root })),
    unwatch: (lib) => fx<void>(async () => void (await indexer.call("unwatch", { lib }))),
    watchFault: (lib) => fx(() => indexer.call("watchFault", { lib })),
    stats: () => fx(() => indexer.call("stats", {})),
  };
  const readerClient: ReaderClient = {
    query: (lib, ext, name, args) => fx(() => reader.call("query", { lib, ext, query: name, args })),
    files: (lib, args) => read(lib, "files", args),
    file: (lib, ref) => read(lib, "file", ref),
    search: (lib, args) => read(lib, "search", args),
    lookup: (lib, args) => read(lib, "lookup", args),
    hashCandidates: (lib, args) => read(lib, "hashCandidates", args),
    searchPage: (req) => fx(() => reader.call("searchPage", req)),
    counts: (lib) => read(lib, "counts", {}),
    fileLines: (lib, args) => read(lib, "fileLines", args),
    resolve: (lib, relKey) =>
      Effect.flatMap(read<{ matches: Array<FileRow & { rel: { components: string[] } }> }>(lib, "file", { relKey }), (r) =>
        Effect.try({ try: () => pickMatch(r.matches), catch: (e) => e as FinError }),
      ),
    stageCounts: (lib) => read(lib, "stageCounts", {}),
    stageRows: (lib, args) => read(lib, "stageRows", args),
    stats: () => fx(() => reader.call("stats", {})),
  };
  let stopped: Promise<void> | undefined;
  return {
    indexer: indexerClient,
    reader: readerClient,
    posted: { indexer: indexer.posted, reader: reader.posted },
    onWatch: (listener) => {
      indexer.onEvent = (e) => listener(e as WatchEvent);
    },
    health: () => {
      const error = indexer.dead ?? reader.dead;
      return error === undefined || stopped !== undefined ? { ok: true } : { ok: false, error };
    },
    shutdown: () => {
      stopped ??= (async () => {
        await reader.stop();
        await indexer.stop();
      })();
      return stopped;
    },
  };
}

export class CatalogsTag extends Context.Service<CatalogsTag, Catalogs>()("finisherd/LibraryCatalogs") {}

/**
 * Both catalog threads for the enclosing scope: started at build, stopped at release. The
 * production layer takes no test hooks, and `jobsRoot` is required: batch files outside it
 * are refused.
 */
export const layerCatalogs = (opts: Omit<CatalogThreadsOptions, "testHooks" | "txRows"> & { readonly jobsRoot: string }): Layer.Layer<CatalogsTag, FinError> =>
  Layer.effect(
    CatalogsTag,
    Effect.acquireRelease(fx<Catalogs>(() => startCatalogThreads(opts)), (c) => Effect.promise(() => c.shutdown())),
  );
