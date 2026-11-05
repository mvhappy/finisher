/** A rejected fin-node call: `message` is the core message; `code` is a stable SCREAMING_SNAKE code. */
export interface FinNodeError extends Error {
  code: string;
  details: Record<string, unknown>;
  fix: string | undefined;
  /** Always `true`: the code comes from fin-core/fin-store (not Node or SQLite). */
  finNode: true;
}

/**
 * One project's op log. Short calls run on the libuv thread pool; long calls (`create`,
 * `open`, `takeover`, `verify`, `rebuild`, `compact`, `close`) run one at a time, in call
 * order, on the engine's own worker thread, as do the daily backup and compaction the
 * maintenance thread finds due (operations `"backup (auto)"`, `"compaction (auto)"`). While a
 * long call runs, a short call waits up to 250 ms and then rejects with `ENGINE_BUSY`
 * (`details: { operation, sinceMs }`, transient); `status()` answers at once from a cached
 * status with `busy` set. Every method rejects with a
 * `FinNodeError`. JSON payloads follow the fin-core schema (`@finisher/core-types`) and the
 * fin-store `ProjectStatus` / report shapes (camelCase).
 *
 * A maintenance thread ticks the store every 500 ms (following the folder, exporting, driving
 * the lease; scheduling backups and compaction) until `close`. An engine garbage-collected
 * without `close` is closed (and flushed) on its worker thread.
 */
export declare class Engine {
  private constructor();
  /** Creates a project; `rate` is a rational string such as `"30"` or `"30000/1001"`. */
  static create(serverData: string, projectDir: string, name: string, rate: string): Promise<Engine>;
  /**
   * Opens a project: acquires the lease or follows, imports, rebuilds a missing or damaged DB.
   * `relocate`: the project now lives in `projectDir` although the folder it was recorded at
   * cannot be reached (otherwise `PROJECT_ORIGINAL_UNREACHABLE`). Never set it automatically.
   */
  static open(serverData: string, projectDir: string, options?: { relocate?: boolean }): Promise<Engine>;
  readonly projectId: string;
  /** Head revision of `branch` (0 when it does not exist). */
  head(branch: string): Promise<number>;
  /**
   * Commits a `Tx` (JSON); resolves to the `CommittedTx` JSON. With `generationJson`
   * (`{"incarnation": string, "n": number}`, from `opsSinceAt` or `status`), rejects with
   * `HISTORY_REWOUND` when the branch was rewound or the database rebuilt since.
   */
  commit(txJson: string, generationJson?: string): Promise<string>;
  /** Resolves to `{ rev, project, generation: { incarnation, n } }` JSON, read together. */
  snapshot(branch: string): Promise<string>;
  /** Resolves to the `CommittedTx[]` JSON with `rev > rev`. */
  opsSince(branch: string, rev: number): Promise<string>;
  /** Resolves to `{ generation: { incarnation, n }, ops: CommittedTx[] }` JSON, read together. */
  opsSinceAt(branch: string, rev: number): Promise<string>;
  /**
   * Resolves to the `ProjectStatus` JSON (`fin project status`) plus `busy`: `null`, or
   * `{ operation, sinceMs }` while a long call runs (the status is then the one cached after the
   * last tick or long call).
   */
  status(): Promise<string>;
  /**
   * What became of `txId`: the `CommittedTx` JSON; or, when it was set aside,
   * `{ "setAside": { txId, branch, reason, origin } }` JSON (`reason`: `SUPERSEDED`,
   * `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`, `MALFORMED`,
   * `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`); or `null` when unknown.
   */
  txGet(txId: string): Promise<string | null>;
  /** Stops status reporting the rewind of `branch` that started `generation`; `false` if none. */
  acknowledgeRewind(branch: string, generation: number): Promise<boolean>;
  /** Acknowledges the incomplete salvage of kept copy `copy`; `false` if none is listed. */
  acknowledgeSalvage(copy: string): Promise<boolean>;
  /** Long call: takes the lease over; resolves to the `ProjectStatus` JSON. */
  takeover(confirm: boolean): Promise<string>;
  /** Long call: resolves to the `VerifyReport` JSON (`{ ok, checks: [{ name, ok, details }] }`). */
  verify(): Promise<string>;
  /** Long call: rebuilds the database from the folder; resolves to the `RebuildReport` JSON. */
  rebuild(): Promise<string>;
  /**
   * Long call: compacts now (forced); resolves to `{ skipped, manifests, packsWritten, replaced,
   * finished, deleted, waiting: [name, why][], abandoned: [name, why][] }` JSON.
   */
  compact(): Promise<string>;
  /**
   * Delivers every later store event to `cb` (replacing an earlier callback); events from
   * before the first registration are buffered (the newest 1024). Does not keep the process
   * alive. A callback that throws is reported with `process.emitWarning` (code
   * `FIN_NODE_EVENT_CALLBACK`). Events (JSON):
   * - `{ type: "applied", tx: CommittedTx }`: a record imported from the folder (by a tick, at
   *   open, by a takeover or a rebuild);
   * - `{ type: "status", status: ProjectStatus }`: the status changed;
   * - `{ type: "rewound", branch, fromRev, quarantined, own, generation, incarnation }`;
   * - `{ type: "rebuilt", incarnation }`: the database was rebuilt; earlier revs may differ now;
   * - `{ type: "error", error: { code, message, details, fix? }, fatal: boolean }`: a tick or an
   *   automatic backup/compaction failed (`fatal`: it panicked and the engine is dead);
   * - `{ type: "lagged", dropped }`: first after registration when the buffer overflowed.
   */
  onEvent(cb: (json: string) => void): void;
  /**
   * Stops maintenance, flushes the export and closes the database (after long calls queued
   * before it); later calls reject with `ENGINE_CLOSED`. Idempotent.
   */
  close(): Promise<void>;
  /** @internal Test builds only (`test-hooks`): rejects with `INTERNAL` and kills the engine. */
  panicForTest(): Promise<void>;
  /** @internal Test builds only: the next maintenance tick panics. */
  panicNextTickForTest(): void;
  /** @internal Test builds only: the next long call keeps the store `ms` longer. */
  delayNextLongCallForTest(ms: number): void;
  /** @internal Test builds only: emits a synthetic `rewound` event. */
  emitRewoundForTest(branch: string, fromRev: number): void;
  /** @internal Test builds only: whether the maintenance thread runs. */
  maintenanceRunningForTest(): boolean;
}

/** A held exclusive file lock; the descriptor stays open until `release()` or process exit. */
export declare class ServerLock {
  private constructor();
  readonly path: string;
  /** Unlocks and closes the file (left in place). Idempotent. */
  release(): void;
}

/**
 * Opens or creates `path` (0600) and takes a non-blocking exclusive kernel lock
 * (`flock(LOCK_EX|LOCK_NB)` / `LockFileEx`). Throws a `FinNodeError` with code
 * `SERVER_RUNNING` if it is held (also by another descriptor in this process), or
 * `SERVER_START_FAILED` on I/O errors. The pid is written into the file for diagnostics only.
 */
export declare function lockExclusive(path: string): ServerLock;

/** Probes an MP4/MOV file; resolves to `{ duration, video, audio, fingerprint, size }` JSON. */
export declare function probe(path: string): Promise<string>;

/** @internal Present only in binaries built with the `test-hooks` feature (`build:test`). */
export declare const testHooks:
  | {
      writeFixtureMp4(
        path: string,
        opts?: { seconds?: number; fps?: number; audio?: boolean; sampleRate?: number },
      ): void;
    }
  | undefined;
