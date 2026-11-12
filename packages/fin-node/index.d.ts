/** A rejected fin-node call: `message` is the core message; `code` is a stable SCREAMING_SNAKE code. */
export interface FinNodeError extends Error {
  code: string;
  details: Record<string, unknown>;
  fix: string | undefined;
  /** Always `true`: the code comes from fin-core/fin-store (not Node or SQLite). */
  finNode: true;
}

/** The optional inputs of `Engine.prepareMediaJob`. */
export interface PrepareMediaOptions {
  /** The server's font cache root, recorded in the manifest. */
  fontCache?: string | null;
  /** The job's `LookParams` as JSON: `params.json` is these with `planSamples` filled in. */
  paramsJson?: string | null;
}

/**
 * One project's op log. Short calls run on the libuv thread pool; long calls (`create`,
 * `open`, `takeover`, `verify`, `rebuild`, `compact`, `close`) run one at a time, in call
 * order, on the engine's own worker thread. Automatic maintenance never holds the store that
 * way: once the engine has been quiet for 30 s, the daily backup runs off the store (calls go
 * on; a long call cancels it) and compaction one short step at a time. While a
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
  /**
   * Creates a project; `rate` is a rational string such as `"30"` or `"30000/1001"`.
   * `idemKey`: a retry with the same key returns the project this create made (opened again)
   * instead of `PROJECT_EXISTS`; with another name or rate it is `IDEM_KEY_REUSED`. An empty or
   * whitespace-only name is `INVALID_NAME`.
   */
  static create(serverData: string, projectDir: string, name: string, rate: string, options?: { idemKey?: string | undefined }): Promise<Engine>;
  /**
   * Opens a project: acquires the lease or follows, imports, rebuilds a missing or damaged DB.
   * `relocate`: the project now lives in `projectDir` although the folder it was recorded at
   * cannot be reached (otherwise `PROJECT_ORIGINAL_UNREACHABLE`). Never set it automatically.
   */
  /** `expectId`: the project the folder must hold, checked before the lease (`PROJECT_NOT_FOUND {reason: changed}`). */
  static open(serverData: string, projectDir: string, options?: { relocate?: boolean; expectId?: string }): Promise<Engine>;
  readonly projectId: string;
  /** Head revision of `branch` (0 when it does not exist). */
  head(branch: string): Promise<number>;
  /**
   * Commits a `Tx` (JSON); resolves to the `CommittedTx` JSON. With `generationJson`
   * (`{"incarnation": string, "n": number}`, from `opsSinceAt` or `status`), rejects with
   * `HISTORY_REWOUND` when the branch was rewound or the database rebuilt since.
   * `metaJson` (`{actor?: {kind, session?, call?}, guard?: WriteGuard}`): `actor` (finisherd's
   * `callMeta`) is the actor of the `log.commit` diagnostics entry; `guard` (branch ids:
   * `{denyBranches?, allowBranches?}`) is checked by the store inside the write transaction
   * against the branch the call changes (`BRANCH_PROTECTED`, `BRANCH_NOT_ALLOWED`). A
   * `metaJson` or `guard` that does not decode is `INVALID_OP`. Every writing branch and
   * history call (and `recover`) takes it too.
   */
  commit(txJson: string, generationJson?: string, metaJson?: string): Promise<string>;
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
   * `{ status, seq }` JSON: `status()`'s answer and the number of the last event (`onEvent`'s
   * `seq`) emitted before it was read. Events numbered up to `seq` are older than it (their
   * changes are in it), later ones never older: a subscriber starting from it drops the former.
   */
  statusAt(): Promise<string>;
  /**
   * What became of `txId`: the `CommittedTx` JSON; or, when it was set aside,
   * `{ "setAside": { txId, branch, reason, origin } }` JSON (`reason`: `SUPERSEDED`,
   * `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`, `MALFORMED`,
   * `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`); or `null` when unknown.
   */
  txGet(txId: string): Promise<string | null>;
  /**
   * Stops status reporting the rewind of `branch` that started `generation`; `false` if none.
   * `metaJson`'s guard names the caller's credential, refused once ended (`CREDENTIAL_REVOKED`).
   */
  acknowledgeRewind(branch: string, generation: number, metaJson?: string): Promise<boolean>;
  /** Acknowledges the incomplete salvage of kept copy `copy`; `false` if none is listed. `metaJson` as above. */
  acknowledgeSalvage(copy: string, metaJson?: string): Promise<boolean>;
  /** Makes a branch: `BranchCreate` JSON (camelCase); resolves to the `BranchCreated` JSON. */
  branchCreate(reqJson: string, metaJson?: string): Promise<string>;
  /** Resolves to the `BranchView[]` JSON (`main` first; with `all`, archived ones too). */
  branches(all: boolean): Promise<string>;
  /** Renames, archives or restores a branch: `BranchSet` JSON; resolves to the `CommittedTx` JSON. */
  branchSet(reqJson: string, metaJson?: string): Promise<string>;
  /** Resolves to the JSON string of the id a branch reference (id, name, `<name>#<id>`) names. */
  resolveBranch(name: string): Promise<string>;
  /** `HistoryReq` JSON (`to`: undo back to a checkpoint); resolves to the `HistoryResult` JSON. */
  undo(reqJson: string, metaJson?: string): Promise<string>;
  /** `HistoryReq` JSON; resolves to the `HistoryResult` JSON. */
  redo(reqJson: string, metaJson?: string): Promise<string>;
  /** `CheckpointReq` JSON; resolves to the `CommittedTx` JSON. */
  checkpoint(reqJson: string, metaJson?: string): Promise<string>;
  /** `MergeReq` JSON; resolves to the `MergeReport` JSON. */
  merge(reqJson: string, metaJson?: string): Promise<string>;
  /** `CherryPickReq` JSON; resolves to the `HistoryResult` JSON. */
  cherryPick(reqJson: string, metaJson?: string): Promise<string>;
  /** `RevertReq` JSON; resolves to the `HistoryResult` JSON. */
  revert(reqJson: string, metaJson?: string): Promise<string>;
  /** Long call: lists recoverable groups and, with `create`, makes their branches (`RecoverReq` JSON); resolves to the `RecoverReport` JSON. */
  recover(reqJson: string, metaJson?: string): Promise<string>;
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
   *
   * `seq` numbers the events as they are emitted, from 1 (`lagged`: 0); see `statusAt`.
   */
  onEvent(cb: (json: string, seq: number) => void): void;
  /**
   * Resolves to the `FramePlan` JSON of `timeline` (`null`: the main timeline) on `branch` at
   * `at` (a rational time string). Captions are measured with the server's shared measurer;
   * a caption whose font bytes are missing is unmeasured (`layout: null`, `unmeasured`),
   * never an error, and the missing files are queued for fetching.
   */
  evalFrame(branch: string, timeline: string | null | undefined, at: string): Promise<string>;
  /**
   * Prepares a `media.look` job's inputs (plan 6A Task 12): evaluates the plan of `timeline` on
   * `branch`'s head at every time of `at` (rational seconds), and writes `snapshot.json`
   * (`JobSnapshot`), `manifest.json` (`RenderManifest`: the assets the plans reference, once
   * each, with the facts the worker's opener checks, or `original: null` for a file that is
   * missing; the font cache root `options.fontCache` and the project's font asset files) and,
   * given `options.paramsJson` (the job's `LookParams`), `params.json` (the same with `planSamples` filled)
   * into `inputDir` (the job's `in/` directory, which must exist). Resolves to
   * `{ rev, planSamples: [{ at, blake3 }] }` as JSON: the rev snapshotted and the BLAKE3 of the
   * plan's compact JSON (what `evalFrame` returns) at each time. Rejects as `evalFrame` does
   * (`INVALID_RANGE` past the timeline's end, `NOT_FOUND` for an unknown timeline).
   */
  prepareMediaJob(
    branch: string,
    timeline: string,
    at: string[],
    inputDir: string,
    options?: PrepareMediaOptions | null,
  ): Promise<string>;
  /** Resolves to the `AudioPlan` JSON of `timeline` on `branch` over `[from, to)`. */
  evalAudio(branch: string, timeline: string | null | undefined, from: string, to: string): Promise<string>;
  /**
   * Resolves to the `FontsPlan` JSON (`{ ops, notes }`): the `font.pin` ops to prepend to a tx
   * of `opsJson` (a JSON `Op[]`) on `branch`. The ops are applied to a copy of the head first,
   * so a tx that cannot apply rejects here as at commit. The tx itself is never changed.
   */
  fontsPlan(branch: string, opsJson: string): Promise<string>;
  /**
   * Sets the project's collection flag in `finisher.project.json` (needs the writer lease:
   * `PROJECT_READ_ONLY` otherwise) and in the server's journal; `never` purges the project's
   * diagnostics entries. Resolves to `{ project, collect }` JSON. `metaJson` as for
   * `acknowledgeRewind`.
   */
  setCollect(collect: "default" | "never", metaJson?: string): Promise<string>;
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
  /** @internal Test builds only: automatic work starts once the engine has been quiet for `ms` (30 s otherwise). */
  setAutoIdleForTest(ms: number): void;
  /** @internal Test builds only: each automatic backup waits `ms` between its copy and its checks. */
  pauseAutoBackupForTest(ms: number): void;
  /** @internal Test builds only: whether an automatic backup is running. */
  autoBackupRunningForTest(): boolean;
  /** @internal Test builds only: emits a synthetic `rewound` event. */
  emitRewoundForTest(branch: string, fromRev: number): void;
  /** @internal Test builds only: whether the maintenance thread runs. */
  maintenanceRunningForTest(): boolean;
  /**
   * @internal Test builds only: a status refresh computes the summary of the pins now and
   * pauses before publishing it while `txJson` is committed (rescanning the pins); the newer
   * summary must be the last one published.
   */
  fontPublishRaceForTest(txJson: string): void;
}

// The diagnostics journal, the agent-trace store and their helpers (`journal.d.ts`).
export * from "./journal.js";

/**
 * The font manager of a server data folder: the content-addressed cache
 * `<serverData>/fonts/`, its fetcher and the shared measurer. Engines opened on the same
 * folder share it. Their `status()` gains `fonts { missing, fetching, failed }` and the
 * warnings `FONT_MISSING`, `FONT_FETCH_FAILED` and `FONTS_DAMAGED`; a change is a `status`
 * event. Projects' missing pins are fetched in the background on open, after a commit or
 * import, when a measurement finds bytes missing and when `fontsPlan` pins them, unless
 * `setAutoFetch(false)` or `FIN_FONTS_OFFLINE=1`.
 */
export declare class FontManager {
  private constructor();
  static open(serverData: string): FontManager;
  /** Whether background fetching is on now. */
  readonly autoFetch: boolean;
  /** `fonts.autoFetch`, at run time (`FIN_FONTS_OFFLINE=1` keeps it off). */
  setAutoFetch(on: boolean): void;
  /**
   * Replaces the enabled plugin packs' font catalogs: `[{ id, catalogs: [<catalog.json
   * text>] }]` JSON, in merge order; returns `{ rejected: [{ id, code, message, details }] }`
   * JSON. A pack whose catalogs break the built-in catalog's rules, or clash with it or an
   * earlier pack (`FONT_CATALOG_INVALID`, rule `catalogIdTaken` or `unique`), is left out
   * whole; the others merge. Pins are untouched (a pinned font keeps its cached file).
   */
  setExtraCatalogs(packsJson: string): string;
  /** The `FontCatalog` JSON: the built-in catalog and the enabled packs' entries. */
  catalog(): string;
  /** Resolves to the `FontsStatus` JSON: every file's state. */
  status(): Promise<string>;
  /**
   * Fetches now (`{ sha256?, entries?, keys?, pinned? }` JSON; `pinned`: an open project's
   * id), even with background fetching off; resolves to the `FontsStatus` JSON or rejects
   * with `FONT_FETCH_FAILED`, `FONT_NOT_IN_CATALOG`, `PROJECT_NOT_FOUND` or `FONTS_DAMAGED`.
   */
  fetch(reqJson: string): Promise<string>;
}

/**
 * Validates the unpacked plugin in `dir` offline (`source`: `registry`, the default,
 * `bundled` or `dev`); resolves to the `ValidationReport` JSON.
 */
export declare function validateManifest(dir: string, source?: "registry" | "bundled" | "dev"): Promise<string>;

/**
 * Each permission's risk within the whole set (`fin_plugin_api::risks`: combinations count), in
 * order (sp8b Task 11: the developer-mode approval screen).
 */
export declare function permissionRisks(permissions: ReadonlyArray<unknown>): Array<"low" | "medium" | "high">;

/**
 * A plugin host call's params decoded by its `fin-plugin-api` type (unknown fields refused):
 * `{ok: true, params}` (canonical), `{ok: false, reserved}` (the batch a later method arrives
 * in), `{ok: false, unknown: true}` or `{ok: false, error}` (sp8b Task 7).
 */
export declare function decodePluginCall(
  method: string,
  params: unknown,
): { ok: true; params: unknown } | { ok: false; reserved: string } | { ok: false; unknown: true } | { ok: false; error: string };

/**
 * `fin view`'s screenplay of `projectJson` (a snapshot's project) without font file states:
 * every timeline, limited to `range` (`2s..5s`), within `budget` tokens. Throws
 * `INVALID_RANGE`/`INVALID_TIME` for a bad range.
 */
export declare function pluginProjectView(projectJson: string, range?: string, budget?: number): { lines: string[]; elided: unknown[]; tokens: number };

/**
 * A command's argument schema (a manifest's `args`), compiled once (sp8b Task 9). The
 * constructor throws `PLUGIN_MANIFEST_INVALID {at, problem}` for a schema outside the subset.
 */
export declare class PluginCommandArgs {
  constructor(schema: unknown);
  validate(args: unknown): { ok: true } | { ok: false; problems: Array<{ path: string; reason: string; message: string }> };
}

/**
 * A `plugin.command` job's result (`CommandOut`) decoded; a transaction is built on `txFields`
 * and applied to `projectJson` (the project the plugin saw, at `baseRev`).
 */
export declare function pluginCommandOutcome(
  projectJson: string,
  out: unknown,
  txFields: { txId: string; author: string; branch: string; baseRev: number; via: { plugin: string; command: string } },
):
  | { ok: true; kind: "data"; value: unknown }
  | { ok: true; kind: "tx"; tx: import("@finisher/core-types").Tx; bindings: Record<string, string>; diff: unknown }
  | { ok: false; protocol: string }
  | { ok: false; invalid: { code: string; message: string; details?: Record<string, unknown>; fix?: string } };

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

/**
 * This server data's current `server-id`, a lowercase ULID: created on first use, renewed when
 * the server data turns out to be a copy (`fin-store` `server_identity`). Throws `IO_ERROR`.
 */
export declare function serverId(serverData: string): string;

/**
 * `serverId` with why it was just renewed (5B): `otherMachine` (a copy from another machine, a
 * cloned machine) or `otherDirectory` (copied or moved to another place); absent otherwise.
 */
export declare function serverIdentity(serverData: string): { id: string; renewed?: "otherMachine" | "otherDirectory" };

/**
 * Review #4: throws `SERVER_DATA_NOT_LOCAL` (details `{path, resolved, filesystem, override}`)
 * unless `path`, or its nearest existing ancestor with symlinks resolved, is on a known local
 * filesystem: not a network share, FUSE, sync folder or overlayfs. `FIN_ALLOW_NONLOCAL_SERVER_DATA=1`
 * accepts any.
 */
export declare function checkServerDataLocal(path: string): void;

/** What `checkPrivateDir` found. */
export interface PrivateDirReport {
  ok: boolean;
  /** The owner's SID (Windows only). */
  owner?: string | undefined;
  /** Why it is not private: `owner <sid>`, `allows <sid>`, `null dacl`, `unsupported ace <type>`. */
  problems: string[];
}

/**
 * Windows: the directory's owner is this user (or Administrators) and its DACL allows nobody
 * but this user, SYSTEM, Administrators and CREATOR OWNER. Elsewhere `{ ok: true }` (POSIX
 * modes are checked by the caller). Throws `SERVER_DATA_UNSAFE` when the path cannot be read.
 */
export declare function checkPrivateDir(path: string): PrivateDirReport;

/**
 * Creates `path` (and parents) private to this user: on Windows a protected DACL
 * `D:P(A;OICI;FA;;;<user>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` owned by the user; elsewhere
 * mode 0700. Throws `SERVER_DATA_UNSAFE`.
 */
export declare function makePrivateDir(path: string): void;

/** A file made by `createPrivateFile`, open for its one write. */
export declare class PrivateFile {
  private constructor();
  readonly path: string;
  /** Writes all of `text`, fsyncs and closes; removes the file when a step fails (`IO_ERROR`). */
  commit(text: string): void;
  /** Closes and removes the file (nothing once it was committed). Idempotent. */
  discard(): void;
}

/**
 * Creates the new file `path` private to this user from creation (mode 0600 with `O_EXCL`; on
 * Windows `CREATE_NEW` with a protected owner-only DACL) and checks the open file before
 * anything is written: on Unix `fstat` (a regular file with one link, owned by the effective
 * user, no group or other bit), on Windows its owner and DACL. Throws `ALREADY_EXISTS`,
 * `IO_ERROR`, or `PRIVATE_FILE_UNSAFE` (the file is removed again) with `details.problems`.
 */
export declare function createPrivateFile(path: string): PrivateFile;

/**
 * Ends an `agent`/`mcp` credential (`"<kind>:<id>"`) for every store write of this process
 * (sub-project 5A Task 9): a write whose guard names it and has not committed yet is refused
 * with `CREDENTIAL_REVOKED` inside its transaction. Returns once every write already past that
 * check has committed, so none lands afterwards.
 */
export declare function endCredential(credential: string): void;

/**
 * Moves the finished file `from` (inside a job directory, absolute) to `to` (absolute; its
 * directory must exist), replacing it, with `maxBytes` as the cap. By default
 * `fin_shm::commit_output` (a rename: the destination is the writer's own inode, so call it
 * only once the worker has exited); with `copy`, `fin_shm::commit_output_copy`: the bytes of
 * the checked handle go into a temporary file the host creates beside `to` (`O_EXCL`, synced)
 * and that is renamed over it, so a worker that is still alive (warm) never holds the
 * destination. Rejects with `SHM_REFUSED` (`details.file` is absolute, `details.reason`:
 * `link`, `notRegular`, `hardLinked`, `overCap`, `changed`, `busy`, …) when `from` is not a
 * plain file of the job or the arrival is not the file that was checked.
 */
export declare function commitJobOutput(from: string, to: string, maxBytes: number, copy?: boolean | undefined | null): Promise<void>;

/**
 * Probes a media file (MP4/MOV/M4A/WAV), image (PNG/JPEG/WebP), SVG, dotLottie, font
 * (TTF/OTF/TTC) or `.cube` LUT; resolves to `Probe` JSON (`{ kind, duration?, video?, audio?,
 * image?, lottie?, font?, lut?, fingerprint, size }`). Never blocks: anything but a regular file
 * (a folder, FIFO, socket or device) rejects with `ASSET_UNREADABLE`, `details.reason: "NotAFile"`.
 */
export declare function probe(path: string): Promise<string>;

/**
 * `probe` of a file that must be inside `root` (a canonical folder): `path` is `root` joined
 * with plain components, none of them a symlink, decided on the open file (Linux `openat2`
 * `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`; elsewhere one no-follow open per component, or on
 * Windows no reparse point and a final path inside the root). Otherwise rejects with
 * `PATH_NOT_ALLOWED` (`details.reason`: `outsideRoot` or `symlink`).
 */
export declare function probeBeneath(root: string, path: string): Promise<string>;

/**
 * Resolves a library root for `library.add` (plan 5D Decision 3), on a thread of its own:
 * resolves to `{ root, rootKey, identity, storage }` JSON: the canonical absolute path (links
 * resolved), its lookup key (NFC, `/` separators, lower-cased on a case-insensitive volume),
 * the `VolumeIdentity` (`@finisher/core-types`) and the detected storage kind (`local`,
 * `network`, `sync:<provider>`). Rejects `LIBRARY_ROOT_INVALID` (`details.reason`: `relative`,
 * `notFound`, `notDirectory`, `notUtf8`, `serverData`: the root holds or lies inside
 * `serverData`) or `LIBRARY_ROOT_UNREADABLE` (`details.code`; `TIMEOUT` past `timeoutMs`,
 * default 60 s). Opens no file under the root.
 */
export declare function libraryCanonicalRoot(path: string, serverData: string, timeoutMs?: number): Promise<string>;

/**
 * A library root's state now against its stored `VolumeIdentity` JSON: resolves to the
 * `RootState` JSON of `@finisher/core-types`. Never runs on the libuv pool or the JS thread.
 * One stat per root is in flight (`fin_library::root_state` is single-flight): a call while one
 * is pending waits on it. Past `timeoutMs` (default 60 s) it resolves `{ state: "timeout" }`
 * and the stat thread is left to finish (that root counts in `libraryProbeStatus().stuckProbes`
 * until a later probe of it answers anything else).
 */
export declare function libraryProbeRoot(root: string, identityJson: string, timeoutMs?: number): Promise<string>;

/** What the root probes are doing: `stuckProbes` roots whose latest probe timed out. */
export declare function libraryProbeStatus(): { stuckProbes: number };

/** `fin_probe::PROBE_VERSION`: the probe version the catalog stores per file (a file probed at an older one is probed again). */
export declare function libraryProbeVersion(): number;

/** A running watch of one library root (`libraryWatchStart`). */
export interface LibraryWatch {
  /** Stops the watch (idempotent): no message is delivered after it returns. */
  close(): void;
}

/**
 * Linux only (other systems throw `WATCH_UNSUPPORTED`): watches a local library root with one
 * inotify watch per directory, on the root's mount only (nested mounts and automount points
 * are not descended into, nothing is triggered), never a dot-directory, at most `maxWatches`
 * directories, on a thread of its own. `callback` gets JSON strings on the calling thread:
 * `{ ready: number }` after the first walk, `{ dirs: RawRel[] }` (directories whose entries
 * changed), `{ all: true }` (events were lost: poll everything), `{ failed: code }` (the watch
 * stopped: `WATCH_LIMIT`, `ENOSPC`, `EACCES`, `WATCH_ROOT_GONE`, …). Opens no file.
 */
export declare function libraryWatchStart(root: string, maxWatches: number, callback: (json: string) => void): LibraryWatch;
// The job host and the job params helpers (`jobhost.js`).
export * from "./jobhost.js";
// The sandbox status (`sandbox.js`).
export * from "./sandbox.js";
// `asset.probe` of a catalogued library file (`library.js`).
export * from "./library.js";
// The native Windows pipe listener (`pipe.js`).
export * from "./pipe.js";
// The vault and the identity key (`vault.js`, 5B).
export * from "./vault.js";
// Self-signed certificates and QR codes (`tls.js`, 5B).
export * from "./tls.js";
// The plugin store and registries (`registry.js`).
export { Registry } from "./registry.js";

/** @internal Present only in binaries built with the `test-hooks` feature (`build:test`). */
export declare const testHooks:
  | {
      /** A generated font: `sans`, `rtl`, `han`, `latinBold`, `big:<bytes>[:<seed>]`. */
      testFont(name: string): Buffer;
      /** Measures a `TextRequest` JSON with the shared measurer; returns the `Measured` JSON. */
      measure(serverData: string, requestJson: string): string;
      writeFixtureMp4(
        path: string,
        opts?: { seconds?: number; fps?: number; audio?: boolean; sampleRate?: number },
      ): void;
      /** macOS: the paths process `pid` holds open or mapped (`proc_pidinfo`); throws `IO_ERROR` when they cannot be read, and elsewhere. */
      heldPaths(pid: number): string[];
      /** While `hang` is true the stat of library root `root` blocks, as on a dead hard mount. */
      libraryProbeHang(root: string, hang: boolean): void;
      /** How many stats of library root `root` have started. */
      libraryProbeStarted(root: string): number;
      /** Writes a small `png`/`jpeg`/`webp` (64x48), `svg` (640x360), `dotlottie`, `cube`
       * (4³), `wav` (1 s, 48 kHz stereo; `wav:<seconds>`, 1–600, for longer) or `m4a` (2 s,
       * 48 kHz stereo) fixture. */
      writeFixture(
        kind: "png" | "jpeg" | "webp" | "svg" | "dotlottie" | "cube" | "wav" | `wav:${number}` | "m4a",
        path: string,
      ): void;
      /** Packs an unpacked plugin into a `.finpkg`; returns the archive's SHA-256. */
      packPlugin(src: string, out: string): string;
      /**
       * Builds a signed local-folder registry in `dir` (keys in `keysDir`, outside it) from
       * `{ packages?, advisories?, yanks?, maxStaleDays?, ageDays? }`: a package is
       * `"@pub/name@1.0.0"` (a content pack) or `{ id, version, kind?: "content" | "code",
       * permissions?, license?, fonts?: string[] }`; `ageDays`: signed as of that many days
       * ago. Returns `{ rootSha256, rootFile }` JSON.
       */
      /** The tree hash of the files under `dir`: what `build.rs` puts in `BUNDLED_PLUGINS`. */
      treeHash(dir: string): string;
      buildRegistry(dir: string, keysDir: string, specJson: string): string;
      /** Publishes more packages, yanks and advisories (the same spec) to a built registry. */
      publishToRegistry(dir: string, keysDir: string, specJson: string): void;
    }
  | undefined;
