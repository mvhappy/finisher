// The evaluation, `media.*` and `fonts.*` payloads of `FinRpcs` (`rpc.ts`).
import type { AudioPlan, FontCatalog, FontsPlan, FontsStatus, FramePlan } from "@finisher/core-types";
import { Schema } from "effect";
import { ChallengeField } from "./auth.ts";

/**
 * Plan 4C Decisions 40–43: evaluation runs on the server. `branch` defaults to `main` (an id,
 * a name or `<name>#<id>`), `timeline` to the main (first) timeline; times are rational
 * strings (`"12"`, `"1001/30000"`). Captions are measured with the server's shared measurer
 * from the project's pinned font bytes; a caption whose bytes are missing is unmeasured
 * (`layout: null`, `unmeasured`), never an error. `rev`: the branch rev the caller resolved
 * `timeline` at (`snapshot.get`'s); a named timeline missing from a head at another rev (moved
 * on, or rewound) is `CONFLICT` (`details.reason: TIMELINE_GONE`; read the project again and retry), while
 * one missing at that rev (or with no `rev`) is `NOT_FOUND`.
 */
export const EvalFramePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  rev: Schema.optional(Schema.Number),
  at: Schema.String,
});
export const EvalAudioPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  rev: Schema.optional(Schema.Number),
  from: Schema.String,
  to: Schema.String,
});
/**
 * Plan 6A Task 13: frames of a timeline as PNGs, rendered by a `media.look` job on the media
 * worker. `at`: 1 to 64 rational times (`"12"`, `"1001/30000"`, canonical; past the timeline's
 * end is `INVALID_RANGE`). `branch` defaults to `main`, `timeline` to the first. `out`: an
 * absolute folder for the files (`workspace/shared/look/` of the project when absent; never
 * inside `.finisher/`, `assets/` or the server's data, `EXPORT_DESTINATION`); naming it needs
 * `project.write` on top of `project.read` and `jobs.submit`; an agent or MCP principal writes
 * only inside its project's folder, a device inside the server's allowed roots (5B Decision 32). `size`: each side 1 to 8192 (the timeline's resolution when
 * absent). `wait` (default true) answers the `LookResult` (frames with their final paths, what
 * was `skipped`, the adapter) once the job finished, or its error; `wait: false` answers
 * `{job}` at once.
 */
export const MediaLookPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  at: Schema.Array(Schema.String),
  out: Schema.optional(Schema.String),
  size: Schema.optional(Schema.Struct({ width: Schema.Number, height: Schema.Number })),
  wait: Schema.optional(Schema.Boolean),
});
/** The generated `LookResult` (`wait`), or `{job}` (the job's view: `wait: false`). */
export const MediaLookSuccess = Schema.Unknown;
/** The generated `MediaCapabilities`: `{codecs, gpu, worker}`. */
export const MediaCapabilitiesSuccess = Schema.Unknown;
/**
 * 6B-1 Task 13: the media cache (spec §7.2). `media.cache.status` (`server.read`) answers the
 * generated `CacheStatus` (`{bytes, entries, capBytes, pinned, byRecipe}`); `media.cache.gc`
 * (`settings.admin`, the owner's) evicts unpinned entries, least recently used first, down to
 * the cap (`media.cacheMaxGiB` in `settings.json`, default `min(50 GiB, 15 % of the volume)`)
 * or to `toGiB` when lower, and answers the generated `CacheGcResult` (`{evicted, freedBytes,
 * bytes}`). Pinned entries (a running job's, an open project's) are never evicted.
 */
export const MediaCacheStatusSuccess = Schema.Unknown;
/** `challenge`: the console's confirmation code (the owner's change, as `settings.write`). */
export const MediaCacheGcPayload = Schema.Struct({ toGiB: Schema.optional(Schema.Number), challenge: ChallengeField });
export const MediaCacheGcSuccess = Schema.Unknown;
/**
 * Plan 6B-1 Task 11: the decode-level probe of one `media` asset by a `media.probeDeep` job:
 * the first frame decoded with the decoder this machine picks, and in-band HDR SEI read when
 * the container has no HDR boxes. `branch` defaults to `main`. Waits for the job (up to 120 s)
 * and commits nothing.
 */
export const MediaProbeDeepPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
});
/** The generated `ProbeDeepResult`: `{asset, fingerprint, codec, decodable, backend?, class?, hw?, hdr?, hdrSource?, firstFrame?, failure?}`. */
export const MediaProbeDeepSuccess = Schema.Unknown;
/**
 * 6B-1 Task 11: `asset.reprobe` probes a `media` asset's file again and commits one
 * `asset.reprobe` op (schema 5) with what it found: `video` (display, rotation and pixel aspect
 * included) and `audio`. `deep`: run `media.probeDeep` first and refuse an asset this machine
 * cannot decode (`CODEC_UNAVAILABLE`). A write like the history writes: `txId` is the
 * idempotency key, `author` the record's, `argsHash` stored; `dryRun` probes and commits nothing.
 * `branch` is required, as on every guarded write: the protected-branches stage checks it.
 */
export const AssetReprobePayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.String,
  deep: Schema.optional(Schema.Boolean),
  txId: Schema.String,
  author: Schema.String,
  dryRun: Schema.optional(Schema.Boolean),
  argsHash: Schema.optional(Schema.String),
});
/**
 * `{asset, video?, audio?, deep?, rev?, txId?, dryRun?}`: the streams committed (fin-core's
 * `VideoInfo` and `AudioInfo`), the `media.probeDeep` answer with `deep`, and the record's `rev`
 * and `txId` (none for a dry run).
 */
export const AssetReprobeSuccess = Schema.Unknown;
/** fin-core's `FramePlan` / `AudioPlan` (the generated types). */
export const FramePlanSchema = Schema.Unknown;
export const AudioPlanSchema = Schema.Unknown;
export type FramePlanPayload = FramePlan;
export type AudioPlanPayload = AudioPlan;

/** fin-fonts' `FontCatalog`: the catalog this server fetches from. */
export const FontCatalogSchema = Schema.Unknown;
export type FontCatalogPayload = FontCatalog;
/**
 * fin-fonts' `FontsStatus`: every font file's state in this server (`cached | missing |
 * queued | fetching {bytes, total} | failed {reason, retryAtMs}`), `autoFetch` and
 * `damaged`. A project's own font summary is in `project.status` (`fonts {missing, fetching,
 * failed}`, warnings `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONTS_DAMAGED`).
 */
export const FontStatusSchema = Schema.Unknown;
export type FontStatusPayload = FontsStatus;
/**
 * `fonts.fetch`: fetch now and wait (background fetching being off does not stop it).
 * `sha256`: catalog files; `entries`: catalog entry ids (each of their styles); `keys`: font
 * keys `<family>|<weight>|<normal|italic>` (the catalog's face); `pinned`: a project id
 * (what its branches pin). Fails with `FONT_FETCH_FAILED` (`details {key, entry, style,
 * sha256, reason, retryAt}`), `FONT_NOT_IN_CATALOG`, `PROJECT_NOT_FOUND`, `FONTS_DAMAGED`.
 */
export const FontsFetchPayload = Schema.Struct({
  sha256: Schema.optional(Schema.Array(Schema.String)),
  entries: Schema.optional(Schema.Array(Schema.String)),
  keys: Schema.optional(Schema.Array(Schema.String)),
  pinned: Schema.optional(Schema.String),
});
/**
 * `fonts.plan` (Decision 35): the `font.pin` ops a client prepends to its own tx of `ops`
 * before committing it; `notes` list faces or scripts the catalog cannot pin. The server
 * applies `ops` to a copy of the head (a tx that cannot apply fails here as at commit) and
 * never alters a client's tx. Planned files are fetched.
 */
export const FontsPlanPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  ops: Schema.Array(Schema.Unknown),
});
export const FontsPlanSchema = Schema.Unknown;
export type FontsPlanResult = FontsPlan;
/**
 * 6B-1 Task 14: a proxy of a video asset (`tier` `540` by default, `codec` H.264 where a hardware
 * encoder makes it, else AV1). A cache hit answers at once; otherwise a `media.proxy` job is
 * submitted and, unless `wait: false`, waited for (up to 10 minutes). `branch` defaults to `main`.
 */
export const MediaProxyPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
  tier: Schema.optional(Schema.Literals(["540", "1080"])),
  codec: Schema.optional(Schema.Literals(["h264", "av1"])),
  wait: Schema.optional(Schema.Boolean),
});
/** `{asset, tier, codec, key, state: ready | making, cached?, job?, result?}` (`result` is the generated `ProxyResult`). */
export const MediaProxySuccess = Schema.Unknown;
/** 6B-1 Task 14: each tier's proxy of an asset: `ready`, `making` (with its job), `failed` or `none`. */
export const MediaProxyStatusPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
});
/** The generated `ProxyStatus`: `{asset, fingerprint, tiers: [{tier, state, key?, codec?, job?, code?}]}`. */
export const MediaProxyStatusSuccess = Schema.Unknown;

/**
 * 6B-1 Task 15: thumbnails of a video asset at source times (`at`: rational seconds, at most 64,
 * each `maxSide` px at most, 256 by default) or its filmstrip (`strip`: a sprite per 10 s and a
 * tile per second `tileHeight` px tall, 90 by default), as one media cache entry. Exactly one of
 * `at` and `strip`. A cache hit answers at once; otherwise a `media.thumbs` job is submitted and,
 * unless `wait: false`, waited for (up to 10 minutes). `branch` defaults to `main`.
 */
export const MediaThumbsPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
  at: Schema.optional(Schema.Array(Schema.String)),
  strip: Schema.optional(Schema.Boolean),
  maxSide: Schema.optional(Schema.Number),
  tileHeight: Schema.optional(Schema.Number),
  wait: Schema.optional(Schema.Boolean),
});
/** `{asset, mode: thumbs | strip, key, state: ready | making, cached?, job?, files?}`: `files` are the entry's (`media.url` makes a URL of one). */
export const MediaThumbsSuccess = Schema.Unknown;

/**
 * 6B-1 Task 17: `media.url {key, file}` mints a capability URL for one file of a media cache
 * entry (spec 6 §7.3, plan Decisions 13 and 14). It needs `project.read` and reach over one of
 * the projects the entry serves (`"*"` reaches all); a key the caller cannot reach, a key that
 * is not there and a file the entry's `meta.json` does not list are all `NOT_FOUND`. `url` is
 * origin-relative, `/media/cap/<token>/<file>`: fetch it with `GET` or `HEAD` (single `bytes=`
 * ranges) on the HTTP origin the connection reached the server on; it works for that file only,
 * until `expiresAt` (ms since the epoch, at most 10 minutes) or until the principal is revoked.
 */
export const MediaUrlPayload = Schema.Struct({ key: Schema.String, file: Schema.String });
export const MediaUrlSuccess = Schema.Struct({ url: Schema.String, expiresAt: Schema.Number });
