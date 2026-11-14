// The in-process library API for sub-projects 6 and 8 (plan 5D "Frozen interfaces", Tasks
// 10–12). Task 10 provides `list`, `get`, `pools`, `online`, `use`, `hash` and `onFile`; Task 11
// `locate`, `registerStage`, `prioritize` and `stageStates`; Task 13 adds `reach` and the
// contracts' `LibraryView` schema (this interface is its shape until then).
import type { FinError, JobView, Priority } from "@finisher/contracts";
import { Context, type Effect } from "effect";
import type { LibraryPools } from "./pools.ts";
import type { LibraryState, LibraryStorage } from "./registry.ts";

export type { LibraryPools, LibraryStorage };

/** A catalogued file's state (Decision 23). */
export type FileState = "present" | "missing" | "cloudOnly" | "unreadable" | "probeFailed" | "skipped" | "offline";

/** Base64 of each raw component (the generated `RawRel`). */
export interface RawRelWire {
  readonly components: ReadonlyArray<string>;
}

export interface ExpectWire {
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileId?: string;
  /** The catalogued entry's device (`Expect.dev`): the opener checks a nested mount's file against it. */
  readonly dev?: string;
}

/**
 * One file a stage job reads (Decision 39): its worker opens it **only** through
 * `fin_library::open_for_read(root, rel, expect, hydrate)`. `display` is the NFC path below the
 * root (for messages, never for opening).
 */
export interface StageFile {
  readonly library: string;
  readonly fileId: number;
  readonly root: string;
  readonly rel: RawRelWire;
  readonly display: string;
  readonly expect: ExpectWire;
  readonly hydrate: boolean;
  readonly fp: string;
  readonly size: number;
  readonly kind?: string;
  /** The stored `fin_probe::Probe`. */
  readonly probe?: unknown;
  readonly storage: LibraryStorage;
}

/** A stage sub-projects 6 and 8 register (Decisions 39–42). */
export interface StageSpec {
  /** `/^[a-z][a-zA-Z0-9.:-]{0,63}$/`; `probe` is reserved (built in). */
  readonly name: string;
  /** Opaque; a change re-queues this stage only. */
  readonly version: string;
  readonly appliesTo: (f: StageFile) => boolean;
  readonly dependsOn?: ReadonlyArray<string>;
  /** A registered batch kind whose items are fingerprints and whose result keeps the default `{items, itemErrors}` shape. */
  readonly jobKind: string;
  /** Fingerprints per job, ≤ 256. */
  readonly batch: number;
  readonly params: (files: ReadonlyArray<StageFile>, ctx: { readonly library: string; readonly pools: LibraryPools }) => unknown;
  /** Default `background`. */
  readonly priority?: Priority;
  /** Jobs queued or running per library, default 2. */
  readonly maxInFlight?: number;
  /** A cache hit in the owner's index: the row is `done` without a job. `file` is the row's file. */
  readonly isDone?: (fp: string, file?: StageFile) => Effect.Effect<boolean>;
}

/** Where a file is (`Libraries.locate`, Decision 45). */
export interface Located {
  readonly library: string;
  readonly fileId: number;
  readonly root: string;
  readonly rel: RawRelWire;
  readonly display: string;
  readonly expect: ExpectWire;
  readonly hydrate: boolean;
  readonly state: FileState;
  readonly match: "path" | "alias" | "fingerprint" | "hash";
  readonly offline: boolean;
}

/** A stage row of one library (`Libraries.stageStates`). */
export interface StageState {
  readonly library: string;
  readonly stage: string;
  readonly version: string;
  readonly state: string;
  readonly code?: string;
}

export type FileEvent = {
  readonly type: "added" | "changed" | "missing" | "renamed" | "moved" | "probed" | "stage";
  readonly library: string;
  readonly fileId: number;
  readonly fp?: string;
  readonly stage?: string;
};

export interface LibraryWarning {
  readonly code: string;
  readonly dir?: string;
  readonly count?: number;
}

export interface LastScan {
  readonly at: number;
  readonly mode: "full" | "dirs" | "poll";
  readonly outcome: string;
  readonly code?: string;
}

/** A library as the RPCs show it (Task 13 turns this shape into a contracts schema). */
export interface LibraryView {
  readonly id: string;
  readonly name: string;
  readonly root: string;
  readonly aliases: ReadonlyArray<string>;
  readonly storage: LibraryStorage;
  readonly provider?: string;
  readonly state: LibraryState;
  readonly offlineReason?: string;
  readonly include?: ReadonlyArray<string>;
  readonly exclude: ReadonlyArray<string>;
  readonly kinds: ReadonlyArray<string> | "all";
  readonly scan: { readonly poll: "demand" | number; readonly full: "daily" | "weekly" | "manual" };
  readonly hydrate: boolean;
  readonly bandwidthMiBs?: number;
  readonly stages: ReadonlyArray<string>;
  readonly counts: Readonly<Record<string, number>>;
  readonly lastScan?: LastScan;
  readonly lastPollAt?: number;
  readonly warnings: ReadonlyArray<LibraryWarning>;
  readonly createdAt: number;
}

export interface Libraries {
  list(): ReadonlyArray<LibraryView>;
  /** `LIBRARY_NOT_FOUND`. */
  get(id: string): Effect.Effect<LibraryView, FinError>;
  /** The library's pools (`LIBRARY_NOT_FOUND` as a thrown `FinError` when it has none). */
  pools(id: string): LibraryPools;
  /** Whether the root is usable now (the cached root state, re-probed when older than 60 s). */
  online(id: string): Effect.Effect<boolean>;
  /** Holds the libraries in use (Decision 28) until `release()`. */
  use(ids: ReadonlyArray<string>): { release(): void };
  /** A `library.hash` job of up to 64 catalogued files (Decision 38). */
  hash(library: string, fileIds: ReadonlyArray<number>): Effect.Effect<JobView, FinError>;
  onFile(listener: (e: FileEvent) => void): () => void;
  /**
   * A path under a library root or one of its aliases (by `relKey`; several rows →
   * `PATH_AMBIGUOUS`), else a fingerprint, else a full hash; nothing when no library holds it.
   * `INVALID_PATH` for a relative path or one with `.`/`..` components. `reach`: only the
   * libraries it answers `true` for are searched (a caller's reach; aliases never grant any).
   */
  locate(
    ref: { readonly path?: string; readonly fingerprint?: string; readonly fullHash?: string },
    reach?: (library: string) => boolean,
  ): Effect.Effect<Located | undefined, FinError>;
  /** Registers a stage (throws on an invalid spec or a duplicate name). */
  registerStage(spec: StageSpec): void;
  /** Runs `stage` for `fp` now at `normal` priority, after its dependencies (Decision 41). */
  prioritize(fp: string, stage: string): Effect.Effect<void, FinError>;
  /** The stage rows of `fp` in every library holding it. */
  stageStates(fp: string): Effect.Effect<ReadonlyArray<StageState>>;
}

export class LibrariesTag extends Context.Service<LibrariesTag, Libraries>()("finisherd/Libraries") {}
