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
  static open(serverData: string, projectDir: string, options?: { relocate?: boolean }): Promise<Engine>;
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
   */
  onEvent(cb: (json: string) => void): void;
  /**
   * Resolves to the `FramePlan` JSON of `timeline` (`null`: the main timeline) on `branch` at
   * `at` (a rational time string). Captions are measured with the server's shared measurer;
   * a caption whose font bytes are missing is unmeasured (`layout: null`, `unmeasured`),
   * never an error, and the missing files are queued for fetching.
   */
  evalFrame(branch: string, timeline: string | null | undefined, at: string): Promise<string>;
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
   * diagnostics entries. Resolves to `{ project, collect }` JSON.
   */
  setCollect(collect: "default" | "never"): Promise<string>;
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
  /** @internal Test builds only: lets the maintenance thread queue the daily backup and compaction (off at open). */
  setAutoWorkForTest(enabled: boolean): void;
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

/**
 * The diagnostics journal of a server data folder (`<serverData>/events/`, spec §21.1), which
 * the engines opened on it record into. Calls that only queue or read memory return at once;
 * those that flush or read the files return promises and run off the JS thread. JSON values
 * follow the generated types of `@finisher/core-types` (`ConsentState`, `DiagnosticsReport`,
 * `JournalStatus`, `ExportPage`, `PurgeReport`, `Filter`, `PurgeSelector`, `SessionStart`).
 * Rejects and throws `FinNodeError`s.
 */
export declare class Journal {
  private constructor();
  /**
   * Opens the journal under the consent resolved from `privacyJson` (the raw `privacy`
   * section of `settings.json`; `null` when absent; text that is not JSON when the file is
   * unusable: then off) and this process's `DO_NOT_TRACK`, `FIN_DIAGNOSTICS`, `FIN_TRAINING`.
   * `hostname` is `os.hostname()`, scrubbed from received text. Replaces a journal already
   * open on the folder; installs the chained panic hook. `testOptions` apply only to
   * `test-hooks` builds.
   */
  static open(
    serverData: string,
    privacyJson: string | null,
    hostname: string,
    testOptions?: { statsEveryMs?: number; flushEveryMs?: number },
  ): Journal;
  /** Why the `privacy` section given to `open` was unusable (recording is then off). */
  readonly openWarning: string | undefined;
  /** This process run's id (a ULID). */
  readonly session: string | undefined;
  /** Whether any purpose is on. */
  readonly enabled: boolean;
  /** Applies a new raw `privacy` section; returns why it was unusable (the last good one stays). */
  applySettings(privacyJson: string | null): string | undefined;
  /** The `ConsentState` JSON in force. */
  consent(): string;
  /**
   * Announces this run (`SessionStart` JSON): `session.start` (after `session.unclean`) now or
   * once on. Waits at most `timeoutMs` (default 100, at most 1000) for the writer: true when
   * they are on disk (or nothing was due), false on a timeout (the start still happens).
   */
  startSession(startJson: string, timeoutMs?: number): boolean;
  /** A clean shutdown: `session.end`, `openSession` cleared; resolves to whether it is on disk. */
  endSession(uptimeS: number): Promise<boolean>;
  /**
   * Queues an entry (a wire-form payload for `cli.command`/`error.panic`). `ctxJson`:
   * `{ project?, branch?, txId?, author?, actor?: { kind, session?, call? } }` with raw ids.
   * `session`: the sender's run (a ULID), this journal's own when omitted. Throws
   * `EVENT_KIND_UNKNOWN`, `EVENT_INVALID`, `EVENT_TOO_LARGE`; builds nothing while off.
   */
  record(kind: string, kv: number, ctxJson: string | null | undefined, payloadJson: string, source: string, session?: string): void;
  /** `record`, written and flushed before it returns (at most `timeoutMs`); true when on disk. */
  recordSync(kind: string, kv: number, ctxJson: string | null | undefined, payloadJson: string, source: string, timeoutMs: number): boolean;
  /** One timing: `rpc` keyed by an RPC method, `eval` by an eval key; `code` for a failure. */
  observe(metric: "rpc" | "eval", key: string, micros: number, code?: string): void;
  /** Resolves to the `DiagnosticsReport` JSON (after a flush; reads every file). */
  status(): Promise<string>;
  /** The `JournalStatus` JSON as last published, without waiting. */
  statusNow(): string;
  /** Resolves to one `ExportPage` JSON (`limit` 1–1000; `cursor`: a page's `next`). */
  export(filterJson: string, cursor: string | null | undefined, limit: number, asUpload?: "diagnostics" | "training"): Promise<string>;
  /** Purges (`PurgeSelector` JSON); resolves to the `PurgeReport` JSON. */
  purge(selectorJson: string): Promise<string>;
  /** What this server knows of a project's flag (`null`: unknown). Engines set it themselves. */
  setProjectPolicy(projectId: string, policy: "default" | "never" | null): void;
  /** `{ [projectId]: "default" | "never" | "unknown" }` JSON. */
  projectPolicies(): string;
  /** Drains, flushes and stops the writer. */
  close(): Promise<void>;
  /** @internal Test builds only: panics on a thread named `thread`; true when caught. */
  panicOnThreadForTest(thread: string): boolean;
  /** @internal Test builds only: the writer panics at its next message. */
  panicWriterForTest(): void;
  /** @internal Test builds only: `raw` as entries store an id (a ULID kept, else keyed); undefined while off. */
  idRefForTest(raw: string): string | undefined;
  /** @internal Test builds only: stalls the writer until `resumeWriterForTest` (false when off). */
  pauseWriterForTest(): boolean;
  /** @internal Test builds only: lets a stalled writer go on. */
  resumeWriterForTest(): void;
  /** @internal Test builds only: how often the previous panic hook ran. */
  previousHookCallsForTest(): number;
}

/** fin-core's `RPC_METHODS` table: the keys `rpc.stats` accepts (`other` last). */
export declare function rpcMethods(): string[];

/**
 * The BLAKE3 of `data` (unkeyed), 64 lower-case hex digits: the agent runtime's `bodyHash`,
 * `systemHash` and `toolsHash` as it sends them (`blake3:<hex>`); the journal keys them when it
 * records a turn.
 */
export declare function blake3Hex(data: Uint8Array): string;

/** The BLAKE3 of `data` keyed with a 32-byte `key`, 64 lower-case hex digits. */
export declare function blake3KeyedHex(key: Uint8Array, data: Uint8Array): string;

/** Every kind name of fin-core's event registry. */
export declare function kindNames(): string[];

/**
 * Why a `privacy` section (JSON; `"null"` for none) is not one the journal accepts, naming the
 * first bad key (`privacy.retentionDays: …`); `undefined` when it is.
 */
export declare function validatePrivacy(privacyJson: string): string | undefined;

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
 * A server's plugin store (`<serverData>/plugins`): install plans, commits, removals and the
 * staging sweep. Every call runs on the registry's worker thread, one at a time. Values are
 * JSON strings of the generated `@finisher/core-types` types. Nothing a package ships runs.
 */
export declare class Registry {
  private constructor();
  static open(serverData: string): Registry;
  /**
   * Makes a plan from `{ id, source: { kind: "registry", registry, version? }, update?,
   * allowDowngrade?, installed?: InstalledPlugin, knownIds? }` JSON; resolves to the
   * `InstallPlan` JSON. A registry source is resolved in the registry's trusted metadata and
   * fetched with its signed length and SHA-256 checked (`REGISTRY_NOT_FOUND`,
   * `PLUGIN_NOT_FOUND`, `TUF_TARGET_MISMATCH`); `{ kind: "bundled", root }` plans the
   * compiled-in plugin `id` from `<root>/<name>` (the app's resources) against the hash built
   * into this binary (`PLUGIN_BUNDLED_MISMATCH`, `PLUGIN_NOT_FOUND`) and installs only the
   * compiled-in version; test builds also take `{ kind: "package",
   * archive, registryId, staleDays?, yanked? }` and `testAgentFacing`.
   */
  plan(reqJson: string): Promise<string>;
  /**
   * Installs the plan with its digest (`Accept` JSON: `{ copyleft?, thirdPartyCode? }`;
   * `checksJson`: `{ installedDirs? }`, the installed plugins' package directories, whose font
   * catalog ids must still be free); resolves to the `InstalledPlugin` JSON or rejects with
   * `PLAN_EXPIRED`, `PLAN_MISMATCH` (`reason: installed` when an id was taken since),
   * `COPYLEFT_NOT_ACCEPTED` or `THIRD_PARTY_CODE_NOT_ACCEPTED`.
   */
  commit(planId: string, digest: string, acceptJson?: string, checksJson?: string): Promise<string>;
  /** The staged `InstallPlan` JSON; rejects with `PLAN_EXPIRED` once it is gone. */
  stagedPlan(planId: string): Promise<string>;
  /** Deletes a plan and its staged files; resolves to whether there was one. */
  discardPlan(planId: string): Promise<boolean>;
  /**
   * The installed versions (`[{ id, version }]` JSON) a `disable` advisory of registry `id`
   * applies to: `[{ id, version, advisory }]` JSON.
   */
  revocations(id: string, installedJson: string): Promise<string>;
  /** Deletes a version from the store (and the plugin's data with `purgeData`). */
  removePlugin(id: string, version: string, purgeData: boolean): Promise<void>;
  /** Deletes expired and broken plans; resolves to how many. */
  sweep(): Promise<number>;
  /**
   * Every version directory in the store, readable or not: `[{ id, version, dir }]` JSON, with
   * `dir` as `control.db` records it (`store/@<publisher>/<name>/<version>-<hash12>`).
   */
  installedDirs(): Promise<string>;
  /**
   * The contributions of the installed content pack in `dir` (as `control.db` records it):
   * `{ id, version, fontCatalogs: [text], captionStyles: [{ name, title?, style }] }` JSON,
   * every file validated again as at install (`PLUGIN_MANIFEST_INVALID {reason:
   * contentInvalid}` when one no longer is).
   */
  packContent(dir: string): Promise<string>;
  /** The bundled plugins compiled into this binary: `[{ id, version, treeHash }]` JSON. */
  bundledList(): string;
  /**
   * Re-hashes each bundled plugin's tree under `root` and compares it with the compiled-in
   * hash: `[{ id, version, treeHash, ok, code?, reason? }]` JSON (`code` is
   * `PLUGIN_BUNDLED_MISMATCH`; `reason` is `missing`, `unreadable` or `treeHash`).
   */
  bundledVerify(root: string): Promise<string>;
  /** Test builds only: replaces the compiled-in list for this `Registry`; `null` restores it. */
  setBundledForTest(listJson: string | null): void;
  /**
   * Every added registry's `RegistryStatus` as a `RegistryList` JSON (`{ registries,
   * problems }`): a registry that cannot be opened is a problem and hides no other.
   */
  registryList(): Promise<string>;
  /** One registry's `RegistryStatus` JSON; rejects with `REGISTRY_NOT_FOUND`. */
  registryStatus(id: string): Promise<string>;
  /**
   * Adds the local-folder registry `{ id, path }` (JSON) trusted through `rootJson`:
   * `{ kind: "file", path }` (a root file of the folder) or `{ kind: "sha256", sha256 }`.
   * Resolves to its `RegistryStatus` JSON; rejects with `REGISTRY_INVALID`,
   * `REGISTRY_EXISTS` or `REGISTRY_UNTRUSTED` (the root is none of the folder's; nothing is
   * written). Nothing is fetched: refresh next.
   */
  registryAdd(cfgJson: string, rootJson: string): Promise<string>;
  /** Deletes the registry's config; its trusted metadata stays (Decision 14). */
  registryRemove(id: string): Promise<void>;
  /** Deletes the trusted metadata of a root no registry uses. */
  registryForget(rootSha256: string): Promise<void>;
  /**
   * Refreshes the registry from its folder; resolves to `{ status, report }` JSON
   * (`RegistryStatus`, `RefreshReport`) or rejects with `TUF_*`, `CLOCK_SUSPECT`,
   * `REGISTRY_INVALID`. Journals `registry.refresh` with `byHuman`.
   */
  registryRefresh(id: string, byHuman: boolean): Promise<string>;
  /**
   * Plugins the registries list: `{ text?, kind?, registry? }` JSON; resolves to a `Listing`
   * array JSON.
   */
  search(queryJson: string): Promise<string>;
  /** What each registry signs about plugin `id`: a `PluginInfo` array JSON (empty when none). */
  info(id: string): Promise<string>;
}

/**
 * Validates the unpacked plugin in `dir` offline (`source`: `registry`, the default,
 * `bundled` or `dev`); resolves to the `ValidationReport` JSON.
 */
export declare function validateManifest(dir: string, source?: "registry" | "bundled" | "dev"): Promise<string>;

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

/**
 * Probes a media file (MP4/MOV/M4A/WAV), image (PNG/JPEG/WebP), SVG, dotLottie, font
 * (TTF/OTF/TTC) or `.cube` LUT; resolves to `Probe` JSON (`{ kind, duration?, video?, audio?,
 * image?, lottie?, font?, lut?, fingerprint, size }`).
 */
export declare function probe(path: string): Promise<string>;

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
