// The library RPCs (sub-project 5D Task 13; plan "RPCs" and Decisions 46–51): Effect Schemas
// for the payloads and answers of `library.*`, and the types the server shows (a `LibraryView`,
// a file, a search hit). Scopes are in `scopes.ts`: `library.read` for everything that reads,
// `library.scan` (and `jobs.submit` to scan) for scans and the ordinary `library.set` fields,
// `library.admin` (human-only) for adding, removing and the fields only a person decides
// (`hydrate`, `bandwidthMiBs`, `acceptVolume`, `aliases`).
//
// A path the RPCs take below a library is relative to its root and uses `/`: no `..`, no
// leading `/` or drive, or `INVALID_PATH`. Absolute paths are the root's own business
// (`library.add`) and `asset.probe`'s.
import { Schema } from "effect";
import { ChallengeField } from "./auth.ts";
import { JobViewSchema, ProgressSchema } from "./jobs.ts";

export const LIBRARY_STORAGES = ["local", "network", "sync"] as const;
export const LibraryStorageSchema = Schema.Literals(LIBRARY_STORAGES);
export type LibraryStorage = typeof LibraryStorageSchema.Type;

export const LIBRARY_STATES = ["online", "offline", "scanning", "paused"] as const;
export const LibraryStateSchema = Schema.Literals(LIBRARY_STATES);

/** A catalogued file's state (Decision 23). */
export const FILE_STATES = ["present", "missing", "cloudOnly", "unreadable", "probeFailed", "skipped", "offline"] as const;
export const FileStateSchema = Schema.Literals(FILE_STATES);
export type FileStateName = typeof FileStateSchema.Type;

const PollSchema = Schema.Union([Schema.Literal("demand"), Schema.Number]);
const FullSchema = Schema.Literals(["daily", "weekly", "manual"]);
export const ScanSettingsSchema = Schema.Struct({
  /** `demand`: Decision 28's cadence; a number: a fixed interval in seconds. */
  poll: PollSchema,
  full: FullSchema,
});
/** `library.add|set`: either half may be left out. */
const ScanPatchSchema = Schema.Struct({ poll: Schema.optional(PollSchema), full: Schema.optional(FullSchema) });

const KindsSchema = Schema.Union([Schema.Literal("all"), Schema.Array(Schema.String)]);

/**
 * What a library shows: its settings, its state and counts, its warnings (`ROOT_UNMOUNTED
 * {dir}`, `DIR_EMPTY_SUSPECT {dir}`, `NAME_COLLISION {count}`, `WATCH_DEGRADED`, …). `counts`:
 * files per state. `root` and `aliases` are paths on this server and its other machines.
 */
export const LibraryViewSchema = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  root: Schema.String,
  aliases: Schema.Array(Schema.String),
  storage: LibraryStorageSchema,
  provider: Schema.optionalKey(Schema.String),
  state: LibraryStateSchema,
  offlineReason: Schema.optionalKey(Schema.String),
  include: Schema.optionalKey(Schema.Array(Schema.String)),
  exclude: Schema.Array(Schema.String),
  kinds: KindsSchema,
  scan: ScanSettingsSchema,
  hydrate: Schema.Boolean,
  bandwidthMiBs: Schema.optionalKey(Schema.Number),
  stages: Schema.Array(Schema.String),
  counts: Schema.Record(Schema.String, Schema.Number),
  lastScan: Schema.optionalKey(
    Schema.Struct({
      at: Schema.Number,
      mode: Schema.Literals(["full", "dirs", "poll"]),
      outcome: Schema.String,
      code: Schema.optionalKey(Schema.String),
    }),
  ),
  lastPollAt: Schema.optionalKey(Schema.Number),
  warnings: Schema.Array(Schema.Struct({ code: Schema.String, dir: Schema.optionalKey(Schema.String), count: Schema.optionalKey(Schema.Number) })),
  createdAt: Schema.Number,
});
export type LibraryViewResult = typeof LibraryViewSchema.Type;

/** A library by id or name (a name is matched case-insensitively). */
const LibraryRef = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128));

/** `library.add` (`library.admin`, human-only): a root the owner names. Starts the first full scan. */
export const LibraryAddPayload = Schema.Struct({
  root: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(4096)),
  name: Schema.optional(Schema.String),
  storage: Schema.optional(LibraryStorageSchema),
  include: Schema.optional(Schema.Array(Schema.String)),
  exclude: Schema.optional(Schema.Array(Schema.String)),
  kinds: Schema.optional(KindsSchema),
  scan: Schema.optional(ScanPatchSchema),
  hydrate: Schema.optional(Schema.Boolean),
  bandwidthMiBs: Schema.optional(Schema.Number),
  aliases: Schema.optional(Schema.Array(Schema.String)),
  /** The console's typed confirmation (5A Decision 30). */
  challenge: ChallengeField,
});
export type LibraryAddRequest = typeof LibraryAddPayload.Type;
/** `job`: the first full scan (absent when it could not be queued). */
export const LibraryAddSuccess = Schema.Struct({ library: LibraryViewSchema, job: Schema.optionalKey(JobViewSchema) });
export type LibraryAddResult = typeof LibraryAddSuccess.Type;

/** `library.remove` (`library.admin`): cancels its jobs and deletes its catalog; the files under the root are never touched. */
export const LibraryRemovePayload = Schema.Struct({ id: LibraryRef, challenge: ChallengeField });
export const LibraryRemoveSuccess = Schema.Struct({});

/**
 * `library.set`: `name`, `include`, `exclude`, `kinds`, `scan`, `paused` and `stages` need
 * `library.scan`; `aliases`, `hydrate`, `bandwidthMiBs` and `acceptVolume` need `library.admin`
 * (human-only). `include` and `bandwidthMiBs` take `null` to clear them; an empty `stages`
 * runs every registered stage.
 */
export const LibrarySetPayload = Schema.Struct({
  id: LibraryRef,
  name: Schema.optional(Schema.String),
  include: Schema.optional(Schema.NullOr(Schema.Array(Schema.String))),
  exclude: Schema.optional(Schema.Array(Schema.String)),
  kinds: Schema.optional(KindsSchema),
  scan: Schema.optional(ScanPatchSchema),
  paused: Schema.optional(Schema.Boolean),
  stages: Schema.optional(Schema.Array(Schema.String)),
  aliases: Schema.optional(Schema.Array(Schema.String)),
  hydrate: Schema.optional(Schema.Boolean),
  bandwidthMiBs: Schema.optional(Schema.NullOr(Schema.Number)),
  /** The owner accepts the volume now under the root (a changed volume, a root that is no longer a mount root). */
  acceptVolume: Schema.optional(Schema.Boolean),
  challenge: ChallengeField,
});
export type LibrarySetRequest = typeof LibrarySetPayload.Type;
export const LibrarySetSuccess = Schema.Struct({ library: LibraryViewSchema });

export const LibraryListSuccess = Schema.Struct({ libraries: Schema.Array(LibraryViewSchema) });

export const LibraryStatusPayload = Schema.Struct({ id: LibraryRef });
export const StageCountSchema = Schema.Struct({
  stage: Schema.String,
  version: Schema.String,
  done: Schema.Number,
  queued: Schema.Number,
  failed: Schema.Number,
  stale: Schema.Number,
  skipped: Schema.Number,
});
/**
 * `jobs`: the ids of the library's scans queued or running; `stages`: rows per stage and
 * version (`skipped`: files a stage does not apply to or that are metadata-only); `watch`:
 * `ok | degraded | none` with `watchReason`; `noChangePolls`: polls that found nothing (never
 * journaled); `inUse`: held by an open project, a watch stream or a recent request.
 */
export const LibraryStatusSuccess = Schema.Struct({
  library: LibraryViewSchema,
  jobs: Schema.Array(Schema.String),
  stages: Schema.Array(StageCountSchema),
  watch: Schema.optionalKey(Schema.Literals(["ok", "degraded", "none"])),
  watchReason: Schema.optionalKey(Schema.String),
  noChangePolls: Schema.Number,
  inUse: Schema.Boolean,
});
export type LibraryStatusResult = typeof LibraryStatusSuccess.Type;

/** A catalogued file as `library.files|file` show it (no raw names, no file ids of the volume). */
export const LibraryFileSchema = Schema.Struct({
  id: Schema.Number,
  dirId: Schema.Number,
  /** The name in NFC. */
  name: Schema.String,
  /** The path below the root in NFC, `/`-separated. */
  display: Schema.String,
  ext: Schema.NullOr(Schema.String),
  kind: Schema.NullOr(Schema.String),
  size: Schema.Number,
  /** Nanoseconds since the epoch, as text. */
  mtimeNs: Schema.String,
  state: FileStateSchema,
  code: Schema.NullOr(Schema.String),
  placeholder: Schema.NullOr(Schema.String),
  /** `fp1:…`, once probed. */
  fp: Schema.NullOr(Schema.String),
  /** `blake3:…`, once hashed on demand. */
  fullHash: Schema.NullOr(Schema.String),
  probe: Schema.Unknown,
  probeVersion: Schema.NullOr(Schema.Number),
  durationMs: Schema.NullOr(Schema.Number),
  width: Schema.NullOr(Schema.Number),
  height: Schema.NullOr(Schema.Number),
  /** Another file's name folds to the same key here (`NAME_COLLISION`). */
  collision: Schema.Boolean,
  hardlink: Schema.Boolean,
  firstSeen: Schema.Number,
  lastSeen: Schema.Number,
  lastChange: Schema.Number,
  missingSince: Schema.NullOr(Schema.Number),
});
export type LibraryFileResult = typeof LibraryFileSchema.Type;

export const LibraryDirSchema = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
  display: Schema.String,
  state: Schema.String,
  code: Schema.NullOr(Schema.String),
  placeholder: Schema.NullOr(Schema.String),
  collision: Schema.Boolean,
});

/**
 * `library.files` (`library.read`): one directory's subdirectories and files, keyset on
 * `(name, id)`; `dir` is a path below the root or a directory id (default the root). The
 * first page of a network or sync library whose last poll is older than 5 minutes lists the
 * directory first and waits up to 5 s (`stale: true` past it).
 */
export const LibraryFilesPayload = Schema.Struct({
  id: LibraryRef,
  dir: Schema.optional(Schema.Union([Schema.String, Schema.Number])),
  state: Schema.optional(FileStateSchema),
  kind: Schema.optional(Schema.String),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
});
export const LibraryFilesSuccess = Schema.Struct({
  dir: LibraryDirSchema,
  dirs: Schema.Array(LibraryDirSchema),
  files: Schema.Array(LibraryFileSchema),
  next: Schema.optionalKey(Schema.String),
  stale: Schema.Boolean,
  offline: Schema.Boolean,
  lastPollAt: Schema.optionalKey(Schema.Number),
});
export type LibraryFilesResult = typeof LibraryFilesSuccess.Type;

/** `library.file` (`library.read`): by path below the root or by file id; `stages`: its stage rows. */
export const LibraryFilePayload = Schema.Struct({
  id: LibraryRef,
  path: Schema.optional(Schema.String),
  fileId: Schema.optional(Schema.Number),
});
export const LibraryFileSuccess = Schema.Struct({ file: LibraryFileSchema, stages: Schema.Array(Schema.Unknown) });

const RangeSchema = Schema.Struct({ min: Schema.optional(Schema.Number), max: Schema.optional(Schema.Number) });

/**
 * `library.search` (`library.read`): names and folders, ranked; `libraries` (ids or names)
 * defaults to every library the caller reads; pages share a snapshot behind `cursor` (5
 * minutes, up to 2,000 hits, `truncated` beyond); `since` is a catalog time in ms.
 */
export const LibrarySearchPayload = Schema.Struct({
  query: Schema.optional(Schema.String),
  libraries: Schema.optional(Schema.Array(LibraryRef)),
  kind: Schema.optional(Schema.String),
  duration: Schema.optional(RangeSchema),
  width: Schema.optional(RangeSchema),
  height: Schema.optional(RangeSchema),
  since: Schema.optional(Schema.Number),
  state: Schema.optional(FileStateSchema),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
});
export type LibrarySearchRequest = typeof LibrarySearchPayload.Type;
export const SearchHitSchema = Schema.Struct({
  library: Schema.String,
  fileId: Schema.Number,
  relKey: Schema.String,
  display: Schema.String,
  kind: Schema.optionalKey(Schema.String),
  size: Schema.Number,
  durationMs: Schema.optionalKey(Schema.Number),
  width: Schema.optionalKey(Schema.Number),
  height: Schema.optionalKey(Schema.Number),
  fp: Schema.optionalKey(Schema.String),
  state: FileStateSchema,
  collision: Schema.Boolean,
  score: Schema.Number,
  offline: Schema.Boolean,
});
export const LibrarySearchSuccess = Schema.Struct({
  hits: Schema.Array(SearchHitSchema),
  next: Schema.optionalKey(Schema.String),
  truncated: Schema.Boolean,
  stale: Schema.Boolean,
  lastPollAt: Schema.optionalKey(Schema.Number),
});
export type LibrarySearchResult = typeof LibrarySearchSuccess.Type;

/**
 * `library.lookup` (`library.read`; `hashCandidates` also `library.scan` and `jobs.submit`):
 * exact matches by fingerprint or whole-file hash in the libraries the caller reads. With
 * `hashCandidates`, present files of a matched size without a hash are hashed by one job per
 * library: `pending.job` is the first, `pending.jobs` all of them, `pending.more` says the cap
 * of 1,000 files cut a list.
 */
export const LibraryLookupPayload = Schema.Struct({
  fingerprints: Schema.optional(Schema.Array(Schema.String)),
  hashes: Schema.optional(Schema.Array(Schema.String)),
  hashCandidates: Schema.optional(Schema.Boolean),
  libraries: Schema.optional(Schema.Array(LibraryRef)),
});
export const LibraryLookupSuccess = Schema.Struct({
  matches: Schema.Array(
    Schema.Struct({
      key: Schema.String,
      library: Schema.String,
      fileId: Schema.Number,
      relKey: Schema.String,
      display: Schema.String,
      state: FileStateSchema,
    }),
  ),
  pending: Schema.optionalKey(
    Schema.Struct({
      job: Schema.String,
      jobs: Schema.Array(Schema.Struct({ library: Schema.String, job: Schema.String })),
      more: Schema.Boolean,
    }),
  ),
});
export type LibraryLookupResult = typeof LibraryLookupSuccess.Type;

/**
 * `library.scan` (`library.scan` and `jobs.submit`): a full scan, a targeted scan of `dirs`
 * (paths below the root), or else a poll; the library counts as in use for 10 minutes.
 */
export const LibraryScanPayload = Schema.Struct({
  id: LibraryRef,
  full: Schema.optional(Schema.Boolean),
  dirs: Schema.optional(Schema.Array(Schema.String)),
});
export const LibraryScanSuccess = Schema.Struct({ job: JobViewSchema });

/** `library.watch` (stream, `library.read`): `ids` (ids or names) defaults to every library the caller reads. */
export const LibraryWatchPayload = Schema.Struct({ ids: Schema.optional(Schema.Array(LibraryRef)) });

/** A file event of the library service (`Libraries.onFile`). */
export const FileEventSchema = Schema.Struct({
  type: Schema.Literals(["added", "changed", "missing", "renamed", "moved", "probed", "stage"]),
  library: Schema.String,
  fileId: Schema.Number,
  fp: Schema.optionalKey(Schema.String),
  stage: Schema.optionalKey(Schema.String),
});

/**
 * What `library.watch` sends: a `snapshot` of the views first, then `library` (a view that
 * changed), `scan` (a scan job changed state), `progress` (its progress) and `files` (file
 * events, coalesced: at most 5 batches a second; `dropped` counts events a batch had no room
 * for). Holding the stream marks its libraries in use (Decision 28).
 */
export const LibraryWatchEventSchema = Schema.Union([
  Schema.Struct({ type: Schema.Literal("snapshot"), libraries: Schema.Array(LibraryViewSchema) }),
  Schema.Struct({ type: Schema.Literal("library"), library: LibraryViewSchema }),
  Schema.Struct({ type: Schema.Literal("scan"), library: Schema.String, job: JobViewSchema }),
  Schema.Struct({ type: Schema.Literal("progress"), library: Schema.String, job: Schema.String, progress: ProgressSchema }),
  Schema.Struct({ type: Schema.Literal("files"), events: Schema.Array(FileEventSchema), dropped: Schema.Number }),
]);
export type LibraryWatchEvent = typeof LibraryWatchEventSchema.Type;
