// The in-process library API for sub-projects 6 and 8 (plan 5D "Frozen interfaces", Tasks
// 10–12). Task 10 provides the members below; Task 11 adds `locate`, `registerStage`,
// `prioritize` and `stageStates`, Task 13 `reach` and the contracts' `LibraryView` schema
// (this interface is its shape until then).
import type { FinError, JobView } from "@finisher/contracts";
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
}

export class LibrariesTag extends Context.Service<LibrariesTag, Libraries>()("finisherd/Libraries") {}
