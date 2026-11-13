// Types of `media-cache.js` (re-exported by `index.d.ts`): the server's media cache (6B-1 plan
// Task 13; sub-project 6 spec §7.2).
import type { CacheGcResult, CacheStatus, EntryMeta } from "@finisher/core-types";

export type { CacheGcResult, CacheStatus, EntryMeta };

/** The most bytes one `readFile` returns. */
export declare const MEDIA_READ_MAX: number;

/** What the adoption at open did. */
export interface MediaCacheOpenReport {
  /** Directories without a row whose `meta.json` validated, key recomputed and files matched. */
  readonly adopted: number;
  /** Directories (and other names under a shard) that did not, deleted. */
  readonly deleted: number;
  /** Rows whose directory was gone. */
  readonly dropped: number;
  /** The index did not open (another schema version, damaged) and was made anew. */
  readonly indexReplaced: boolean;
  /** What could not be done and was left (a leftover under `.staging/` or `.trash/` that would not go). */
  readonly warnings: string[];
}

/**
 * `<server-data>/cache/media/`: entries are directories `<key[0..2]>/<key>/` (the payload and
 * `meta.json`), indexed in `index.db` (SQLite, WAL) by fin-node only; workers never touch it.
 * Every call runs in order on the cache's own thread. Errors: `CACHE_IO {reason: full |
 * io | index, freeBytes?, neededBytes?}` (a full volume fails the job without a retry),
 * `PLAN_MISMATCH {reason: cacheKey | cacheEntry, file?}` (an output that is not the entry it
 * claims), `SHM_REFUSED` (fin-shm's checks), `INVALID_OP {field}` (a malformed holder,
 * project id or byte count).
 */
export declare class MediaCache {
  private constructor();
  /**
   * Opens (creating) the cache with a cap of `capBytes` (absent: `min(50 GiB, 15 % of the
   * volume)`). Pins never outlive a server run: the server pins again what it opens. A row
   * without its directory is dropped; a directory without a row is adopted only when its
   * `meta.json` validates, its key recomputes and its files' hashes match, else deleted.
   */
  static open(serverData: string, capBytes?: number): Promise<MediaCache>;
  openReport(): MediaCacheOpenReport;
  /** Where the entry `key` lives (whether or not it exists); `null` for a malformed key. */
  entryDir(key: string): string | null;
  /**
   * The entry, its last use set to now; `null` when there is none or its directory is gone.
   * A hit records nothing else: a consumer (6B-1 Tasks 14–17) then calls `serve(key, projects)`
   * so the entry serves (reach) and is pinned for its projects, and `pin(key, "job:<id>")`
   * before it reads the entry's files (an unpinned entry may be evicted at any time).
   */
  lookup(key: string): Promise<EntryMeta | null>;
  /**
   * Records that `key` serves `projects`, as a commit does, and pins it for those open here;
   * false when there is no such entry. Adopted entries serve no project until this or a commit.
   */
  serve(key: string, projects: ReadonlyArray<string>): Promise<boolean>;
  /**
   * Moves `<jobOutDir>/<key>/` into the cache (after the worker exited): fin-shm's checks,
   * `meta.json` and every file's size and BLAKE3 verified, renamed into place, then indexed
   * with the `projects` it serves (pinned for those open). An entry already there wins: its
   * meta is answered and the projects are added. Evicts down to the cap afterwards.
   */
  commit(jobOutDir: string, key: string, projects: ReadonlyArray<string>): Promise<EntryMeta>;
  /** Pins `key` for `holder` (`job:<id>`, `project:<id>`, `http:<id>`); false when there is no such entry. */
  pin(key: string, holder: string): Promise<boolean>;
  unpin(key: string, holder: string): Promise<boolean>;
  /** Removes every pin of `holder` (a job's end hook); how many. */
  release(holder: string): Promise<number>;
  /** The project is open here: every entry serving it is pinned until `unpinProject`. */
  pinProject(projectId: string): Promise<number>;
  unpinProject(projectId: string): Promise<number>;
  pinsOf(key: string): Promise<string[]>;
  /**
   * Up to `len` bytes (at most {@link MEDIA_READ_MAX}) of the entry file `file` from `offset`,
   * opened through fin-shm's checked opener (no link at any segment, one hard link, at most the
   * listed size): empty at or past the end, `null` when there is no such entry or the entry
   * lists no such file (`meta.json` is not listed). A link or a file larger than listed is
   * `SHM_REFUSED`. A shorter answer than asked, inside the file, means the file is shorter than
   * listed. Serving a file (spec §7.3) is a series of these reads, the entry pinned `http:<id>`.
   */
  readFile(key: string, file: string, offset: number, len: number): Promise<Buffer | null>;
  /**
   * Lists, in the job's `in/manifest.json` (`inDir`, written by `prepareMediaJob`), the `1080`
   * proxy of each asset's fingerprint the index holds (`ManifestAsset.proxies`: tier, key,
   * absolute directory, codec), pinned for `holder` (`job:<id>`; release it when the job
   * ends). Each listed entry is a hit: it now serves `projects` (the job's, as `serve`), so they
   * reach it and pin it while open. Resolves to the files a worker reads from those entries (absolute, for the job's
   * `reads`): none when no asset has a proxy, and then the manifest is left as it is.
   */
  attachProxies(inDir: string, holder: string, projects: ReadonlyArray<string>): Promise<string[]>;
  /** The entries made from the source with this fingerprint. */
  entriesFor(asset: { readonly fingerprint: string }): Promise<EntryMeta[]>;
  /** The projects `key` serves (reach for media bytes, spec §7.3). */
  projectsOf(key: string): Promise<string[]>;
  status(): Promise<CacheStatus>;
  /** Evicts unpinned entries, least recently used first, down to `targetBytes` (at most the cap). */
  gc(targetBytes?: number): Promise<CacheGcResult>;
  /** Sets the cap (absent: the default) and evicts down to it. */
  setCap(capBytes?: number): Promise<CacheGcResult>;
  /** @internal Test builds only: a commit stops after its rename; the pre-move free-space check reads `freeBytes`. */
  setFaultsForTest(faults: { readonly stopAfterRename?: boolean; readonly freeBytes?: number | null }): Promise<void>;
}
