// The diagnostics journal (`<serverData>/events/`), the agent-trace store
// (`<serverData>/agent-traces/`) and their helpers: the native `Journal` of `index.js`, split
// from `index.d.ts`, which re-exports this file.

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
  /**
   * The agent-trace store (`<serverData>/agent-traces`; sp7 §12.1): training only, its own caps
   * (`privacy.tracesMaxMiB`, `tracesRetentionDays`), whole-session eviction; follows the
   * project policies and the key of the journal open on the folder (open that first).
   */
  static openTraces(
    serverData: string,
    privacyJson: string | null,
    hostname: string,
    testOptions?: { statsEveryMs?: number; flushEveryMs?: number; maxTotalBytes?: number; maxFileBytes?: number },
  ): Journal;
  /** Whether training is in force (content may be recorded): one atomic load. */
  readonly training: boolean;
  /** Whether an entry of the project would be recorded now (`default`; not `never`, not unknown). */
  recordsProject(projectId: string): boolean;
  /**
   * Queues `bytes` as a blob, written before the first entry that references it: the `Body`
   * for a content field, or `undefined` (training off, over 4 MiB, a type not in `MEDIA_TYPES`).
   */
  attachBlob(bytes: Uint8Array, mediaType: string): { blob: { hash: string; bytes: number; mediaType: string } } | undefined;
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
  /**
   * Queues a plugin runtime entry (sp8b Task 15) whose plugin is named by its raw id
   * (`@publisher/name`, no version): `worker.exit` kv 2 (and the `plugin.exit` of the same
   * exit), `plugin.exit`, `plugin.stats` or `plugin.command` kv 1, `payloadJson` without
   * `plugin`. fin-node names the plugin (a bundled name or the journal's keyed hash); for
   * `plugin.command`, `command` is the raw name, `txId` a raw id and `args` the raw arguments
   * (kept only with training on). Source `finisherd`. Throws `EVENT_INVALID`; builds nothing
   * while off.
   */
  recordPlugin(kind: "worker.exit" | "plugin.exit" | "plugin.stats" | "plugin.command", kv: number, pluginId: string, payloadJson: string): void;
  /** One timing: `rpc` keyed by an RPC method, `eval` by an eval key; `code` for a failure. */
  observe(metric: "rpc" | "eval", key: string, micros: number, code?: string): void;
  /** Resolves to the `DiagnosticsReport` JSON (after a flush; reads every file). */
  status(): Promise<string>;
  /** The `JournalStatus` JSON as last published, without waiting. */
  statusNow(): string;
  /** Resolves to one `ExportPage` JSON (`limit` 1–1000; `cursor`: a page's `next`; `redact`: content removed). */
  export(filterJson: string, cursor: string | null | undefined, limit: number, asUpload?: "diagnostics" | "training" | null, redact?: boolean): Promise<string>;
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
 * The newest payload version of kind `name` that `source` (`agent`, `finisherd`, …) may emit,
 * or `null` for an unknown kind, a reserved one (version 0) or one that source may not send
 * (7A Task 11: `review.verdict` is emitted once 5E gives it a version).
 */
export declare function kindVersion(name: string, source: string): number | null;

/**
 * Why a `privacy` section (JSON; `"null"` for none) is not one the journal accepts, naming the
 * first bad key (`privacy.retentionDays: …`); `undefined` when it is.
 */
export declare function validatePrivacy(privacyJson: string): string | undefined;
