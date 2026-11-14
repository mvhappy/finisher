// The params of the library kinds, built from a library's row (plan 5D Decisions 9, 14, 26,
// 27, 32, 36–38). Rust decodes and checks them (`fin_library::params`, `validateJobParams`);
// these builders only fill them from the registry, so the service (Task 10) and the stage
// framework (Task 11) submit the same shapes.
import type { LibraryHashParams, LibraryProbeParams, LibraryScanParams, RawRel, Timing } from "@finisher/core-types";
import { readBpsPerSlot, readSlots, scanThreads } from "./pools.ts";
import { KIND_GROUPS, type LibraryRow, metadataOnly } from "./registry.ts";

/** Probe and hash batches (Decisions 34, 38). */
export const PROBE_BATCH = 256;
export const HASH_BATCH = 64;

/**
 * File extensions (lower case, no dot) per probe group of a library's `kinds` (Decision 37).
 * The extension only filters: the recorded kind comes from the content.
 */
export const KIND_EXTENSIONS: Readonly<Record<(typeof KIND_GROUPS)[number], ReadonlyArray<string>>> = {
  media: ["mp4", "mov", "m4v", "m4a", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "wav", "aif", "aiff", "flac", "mp3", "aac", "ogg", "opus"],
  image: ["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic"],
  svg: ["svg"],
  lottieTemplate: ["lottie", "json"],
  font: ["ttf", "otf", "ttc", "woff", "woff2"],
  lut: ["cube"],
};

/** The extensions a library probes, or `undefined` for `kinds: all`. Sorted, without duplicates. */
export function probeExtensions(kinds: LibraryRow["kinds"]): string[] | undefined {
  if (kinds === "all") return undefined;
  return [...new Set(kinds.flatMap((k) => KIND_EXTENSIONS[k as (typeof KIND_GROUPS)[number]] ?? []))].sort();
}

export interface ScanRequest {
  readonly mode: "full" | "dirs" | "poll";
  /** The generation a full scan writes; polls and targeted scans pass the library's current one. */
  readonly gen: number;
  /** `mode: dirs`: the directories to list. */
  readonly dirs?: ReadonlyArray<RawRel>;
  /** Scaled deadlines (test builds of `fin-workers` only). */
  readonly timing?: Timing;
}

/** `library.scan` params for `lib`. */
export function scanParams(lib: LibraryRow, req: ScanRequest): LibraryScanParams {
  return {
    library: lib.id,
    root: lib.root,
    mode: req.mode,
    gen: req.gen,
    ...(req.mode === "dirs" ? { dirs: [...(req.dirs ?? [])] } : {}),
    excludes: {
      caseSensitive: lib.volume.caseSensitive,
      exclude: [...lib.exclude],
      ...(lib.include === undefined ? {} : { include: [...lib.include] }),
    },
    threads: scanThreads(lib.storage),
    volume: lib.volume,
    ...(req.timing === undefined ? {} : { timing: req.timing }),
  };
}

/**
 * One read slot's byte rate for `lib` (Decision 36): its `bandwidthMiBs` split evenly over the
 * volume's read slots (`slots`, default the library's own `readSlots`).
 */
export function readBps(lib: LibraryRow, slots: number = readSlots(lib.storage)): number | undefined {
  return readBpsPerSlot(lib.bandwidthMiBs, slots);
}

const items = (fileIds: ReadonlyArray<number>, max: number): string[] => {
  if (fileIds.length === 0 || fileIds.length > max) throw new Error(`library kinds: 1 to ${max} files per job`);
  if (!fileIds.every((id) => Number.isSafeInteger(id) && id > 0)) throw new Error("library kinds: file ids are positive integers");
  return [...new Set(fileIds)].map(String);
};

/**
 * `library.probe` params for up to 256 catalogued files of `lib`, or `undefined` when the
 * library is metadata-only (Decision 14: Linux FUSE sync roots without `hydrate`): nothing
 * under it is opened, and the caller records the files `skipped` with `METADATA_ONLY`.
 */
export function probeParams(lib: LibraryRow, fileIds: ReadonlyArray<number>, slots?: number): LibraryProbeParams | undefined {
  if (metadataOnly(lib)) return undefined;
  const bps = readBps(lib, slots);
  const extensions = probeExtensions(lib.kinds);
  return {
    library: lib.id,
    root: lib.root,
    hydrate: lib.hydrate,
    ...(bps === undefined ? {} : { readBps: bps }),
    volume: lib.volume,
    ...(extensions === undefined ? {} : { extensions }),
    items: items(fileIds, PROBE_BATCH),
  };
}

/** `library.hash` params for up to 64 catalogued files of `lib` (`undefined` when metadata-only). */
export function hashParams(lib: LibraryRow, fileIds: ReadonlyArray<number>, slots?: number): LibraryHashParams | undefined {
  if (metadataOnly(lib)) return undefined;
  const bps = readBps(lib, slots);
  return {
    library: lib.id,
    root: lib.root,
    hydrate: lib.hydrate,
    ...(bps === undefined ? {} : { readBps: bps }),
    volume: lib.volume,
    items: items(fileIds, HASH_BATCH),
  };
}
