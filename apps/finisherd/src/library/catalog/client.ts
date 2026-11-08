// The main thread's side of the catalog threads (plan 5D Decisions 2, 24): `IndexerClient`
// (writes, in the `library-indexer` thread) and `ReaderClient` (reads, in `library-reader`),
// all `Effect`s over `MessagePort`s. The main thread never opens a catalog, never runs a
// statement and never reads a batch file: it posts ids, paths and plain data, checked by
// `assertCloneable` before posting, so no function crosses a port.
import { basename, dirname } from "node:path";
import { Worker } from "node:worker_threads";
import { FinError } from "@finisher/contracts";
import { Context, Effect, Layer } from "effect";
import type { FileRow, DirRow } from "./core/queries.ts";
import type { PendingFile } from "./core/ops.ts";
import type { ApplyCounts } from "./core/apply.ts";
import { CATALOG_EXTENSIONS, type CatalogExtensionRegistry } from "./extensions.ts";
import { CatalogFailure, type Ready, type Reply, type WireError, assertCloneable } from "./protocol.ts";

export interface CatalogThreadsOptions {
  /** `<server-data>`: catalogs live in `libraries/<id>/catalog.db`. */
  readonly serverData: string;
  /** The engine's jobs root: batch files and `in/` files must lie under it. */
  readonly jobsRoot?: string;
  /** The extensions to load; default every `registerCatalogExtension`ed one. */
  readonly extensions?: CatalogExtensionRegistry;
  /** Rows per indexer transaction (≤ 2,000; tests lower it). */
  readonly txRows?: number;
  readonly clock?: () => number;
}

const toFin = (e: WireError): FinError => new FinError({ code: e.code, message: e.message, details: e.details });

/** One catalog thread and its request/reply bookkeeping. */
class Thread {
  readonly #worker: Worker;
  readonly #pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: FinError) => void }>();
  #next = 1;
  #dead: FinError | undefined;
  readonly posted = { count: 0 };

  private constructor(worker: Worker) {
    this.#worker = worker;
  }

  static start(file: string, name: string, workerData: unknown): Promise<Thread> {
    const worker = new Worker(new URL(file, import.meta.url), { name, workerData });
    const thread = new Thread(worker);
    return new Promise((resolve, reject) => {
      const fail = (e: FinError) => {
        thread.#die(e);
        reject(e);
      };
      worker.once("message", (m: Ready) => {
        if (!m.ready) return fail(toFin(m.error));
        worker.on("message", (r: Reply) => thread.#settle(r));
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
  readonly moved: ReadonlyArray<{ readonly from: number; readonly to: number }>;
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
  readonly state: "queued" | "running" | "done" | "failed" | "stale";
  readonly code?: string | null;
  readonly jobId?: string | null;
  readonly attempts?: number | null;
  readonly at: number;
}

export type PendingArgs =
  | { readonly stage: "probe"; readonly probeVersion: number; readonly exts: ReadonlyArray<string> | null; readonly limit: number }
  | { readonly stage: string; readonly version: string; readonly maxAttempts?: number; readonly limit: number };

export interface IndexerClient {
  /** Runs `ext`'s op `op` in the indexer, inside one transaction. */
  exec(lib: string, ext: string, op: string, args: unknown): Effect.Effect<unknown, FinError>;
  /** Opens (creating, migrating or rebuilding) the catalog, then attaches the reader to it. */
  open(lib: string): Effect.Effect<OpenInfo, FinError>;
  /** Detaches the reader, then closes the catalog. */
  close(lib: string): Effect.Effect<void, FinError>;
  /** Reads, validates and applies a scan batch file at `path` (a plain name in a job's `out/`). */
  applyBatchFile(lib: string, path: string, gen: number, seq?: number): Effect.Effect<BatchApplied, FinError>;
  finishScan(lib: string, scan: ScanRecord): Effect.Effect<{ scanId: number }, FinError>;
  /** Lands probe results; `moved`: new rows merged into the missing row of their fingerprint. */
  applyProbed(lib: string, results: ReadonlyArray<ProbedRow>): Effect.Effect<ProbedApplied, FinError>;
  applyHashes(
    lib: string,
    hashes: ReadonlyArray<{ fileId: number; size: number; mtimeNs: string; fullHash: string }>,
  ): Effect.Effect<{ updated: number; stale: number }, FinError>;
  stagesSet(lib: string, rows: ReadonlyArray<StageRowSet>): Effect.Effect<{ set: number }, FinError>;
  /** Writes the job's `in/dirs.ndjson` (Decision 27) at `path`. */
  writeKnownDirs(lib: string, gen: number, path: string): Effect.Effect<{ dirs: number }, FinError>;
  pending(lib: string, args: PendingArgs): Effect.Effect<{ files: PendingFile[] }, FinError>;
  /** Deletes rows missing since before `before`, ≤ 500 per transaction. */
  purgeMissing(lib: string, before: number): Effect.Effect<{ deleted: number; dirs: number }, FinError>;
  /**
   * Marks the library offline or online for the diff (persisted in the catalog): while offline
   * no listing marks anything missing (Decision 18).
   */
  setOffline(lib: string, offline: boolean): Effect.Effect<{ offline: boolean }, FinError>;
  stats(): Effect.Effect<{ transactions: number; maxTxRows: number; batchFiles: number; open: number }, FinError>;
}

export interface ReaderClient {
  /** Runs `ext`'s query `name` in the reader, on its read-only connection. */
  query(lib: string, ext: string, name: string, args: unknown): Effect.Effect<unknown, FinError>;
  files(lib: string, args: Record<string, unknown>): Effect.Effect<{ dir?: DirRow; matches?: DirRow[]; dirs: DirRow[]; files: FileRow[]; next?: { nameNfc: string; id: number } }, FinError>;
  file(
    lib: string,
    ref: { readonly fileId: number } | { readonly relKey: string },
  ): Effect.Effect<{ matches: Array<FileRow & { rel: { components: string[] } }>; stages: unknown[] }, FinError>;
  search(lib: string, args: Record<string, unknown>): Effect.Effect<{ hits: Array<FileRow & { score: number }> }, FinError>;
  lookup(lib: string, args: { fingerprints?: string[]; hashes?: string[] }): Effect.Effect<{ matches: Array<{ key: string; file: FileRow }> }, FinError>;
  counts(lib: string): Effect.Effect<CatalogCounts, FinError>;
  /**
   * The one file at `relKey` (live rows first, then missing ones): several answer
   * `PATH_AMBIGUOUS` with `details.fileIds`, none `FILE_NOT_FOUND` (Decision 11).
   */
  resolve(lib: string, relKey: string): Effect.Effect<FileRow & { rel: { components: string[] } }, FinError>;
  stageCounts(lib: string): Effect.Effect<{ stages: Array<{ stage: string; version: string; state: string; n: number }> }, FinError>;
  stats(): Effect.Effect<{ queries: number; open: number }, FinError>;
}

export interface Catalogs {
  readonly indexer: IndexerClient;
  readonly reader: ReaderClient;
  /** Messages posted to each thread (tests). */
  readonly posted: { readonly indexer: { count: number }; readonly reader: { count: number } };
  /** Closes every catalog and stops both threads. Idempotent. */
  shutdown(): Promise<void>;
}

/** Starts both threads; resolves once each has imported every extension. */
export async function startCatalogThreads(opts: CatalogThreadsOptions): Promise<Catalogs> {
  const clock = opts.clock ?? Date.now;
  const extensions = (opts.extensions ?? CATALOG_EXTENSIONS).start();
  const [ix, rd] = await Promise.allSettled([
    Thread.start("./indexer-thread.ts", "library-indexer", {
      extensions,
      serverData: opts.serverData,
      ...(opts.jobsRoot === undefined ? {} : { jobsRoot: opts.jobsRoot }),
      ...(opts.txRows === undefined ? {} : { txRows: opts.txRows }),
    }),
    Thread.start("./reader-thread.ts", "library-reader", { extensions }),
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
    applyBatchFile: (lib, path, gen, seq) =>
      fx(() =>
        indexer.call("applyBatchFile", { lib, file: { outDir: dirname(path), file: basename(path) }, gen, at: clock(), ...(seq === undefined ? {} : { seq }) }),
      ),
    finishScan: (lib, scan) => core(lib, "finishScan", scan),
    applyProbed: (lib, results) => core(lib, "applyProbed", { results }),
    applyHashes: (lib, hashes) => core(lib, "applyHashes", { hashes }),
    stagesSet: (lib, rows) => core(lib, "stagesSet", { rows }),
    writeKnownDirs: (lib, gen, path) => fx(() => indexer.call("writeKnownDirs", { lib, gen, path })),
    pending: (lib, args) => core(lib, "pending", args),
    purgeMissing: (lib, before) => fx(() => indexer.call("purgeMissing", { lib, before })),
    setOffline: (lib, offline) => fx(() => indexer.call("setOffline", { lib, offline })),
    stats: () => fx(() => indexer.call("stats", {})),
  };
  const readerClient: ReaderClient = {
    query: (lib, ext, name, args) => fx(() => reader.call("query", { lib, ext, query: name, args })),
    files: (lib, args) => read(lib, "files", args),
    file: (lib, ref) => read(lib, "file", ref),
    search: (lib, args) => read(lib, "search", args),
    lookup: (lib, args) => read(lib, "lookup", args),
    counts: (lib) => read(lib, "counts", {}),
    resolve: (lib, relKey) =>
      Effect.flatMap(read<{ matches: Array<FileRow & { rel: { components: string[] } }> }>(lib, "file", { relKey }), (r) =>
        Effect.try({ try: () => pickMatch(r.matches), catch: (e) => e as FinError }),
      ),
    stageCounts: (lib) => read(lib, "stageCounts", {}),
    stats: () => fx(() => reader.call("stats", {})),
  };
  let stopped: Promise<void> | undefined;
  return {
    indexer: indexerClient,
    reader: readerClient,
    posted: { indexer: indexer.posted, reader: reader.posted },
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

/** Both catalog threads for the enclosing scope: started at build, stopped at release. */
export const layerCatalogs = (opts: CatalogThreadsOptions): Layer.Layer<CatalogsTag, FinError> =>
  Layer.effect(
    CatalogsTag,
    Effect.acquireRelease(fx<Catalogs>(() => startCatalogThreads(opts)), (c) => Effect.promise(() => c.shutdown())),
  );
