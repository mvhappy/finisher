// The `media.*` RPCs of `FinRpcs`, spread into it (`rpc.ts`) so that file stays small; their
// rules are in `scopes.ts`, their payloads in `media.ts` (6C's audio ones in `audio.ts`).
import { Rpc } from "effect/rpc";
import { Schema } from "effect";
import { MediaPcmPayload, MediaPcmSuccess, MediaWaveformPayload, MediaWaveformSuccess } from "./audio.ts";
import { FinError } from "./errors.ts";
import { AssetReprobePayload, AssetReprobeSuccess, MediaCacheGcPayload, MediaCacheGcSuccess, MediaCacheStatusSuccess, MediaCapabilitiesSuccess, MediaLookPayload, MediaLookSuccess, MediaProbeDeepPayload, MediaProbeDeepSuccess, MediaProxyPayload, MediaProxyStatusPayload, MediaProxyStatusSuccess, MediaProxySuccess, MediaThumbsPayload, MediaThumbsSuccess, MediaUrlPayload, MediaUrlSuccess } from "./media.ts";

export const MEDIA_RPCS = [
  /** Plan 6A Task 13: looks at a timeline, and what this machine's media worker can do. */
  Rpc.make("media.look", { payload: MediaLookPayload, success: MediaLookSuccess, error: FinError }),
  Rpc.make("media.capabilities", { payload: Schema.Struct({}), success: MediaCapabilitiesSuccess, error: FinError }),
  /** 6B-1 Task 13: the media cache's status, and an eviction down to the cap or lower. */
  Rpc.make("media.cache.status", { payload: Schema.Struct({}), success: MediaCacheStatusSuccess, error: FinError }),
  Rpc.make("media.cache.gc", { payload: MediaCacheGcPayload, success: MediaCacheGcSuccess, error: FinError }),
  /** 6B-1 Task 11: the decode-level probe of one asset (commits nothing). */
  Rpc.make("media.probeDeep", { payload: MediaProbeDeepPayload, success: MediaProbeDeepSuccess, error: FinError }),
  /** 6B-1 Task 17: a capability URL for one file of a media cache entry (`project.read`, reach over the entry's projects). */
  Rpc.make("media.url", { payload: MediaUrlPayload, success: MediaUrlSuccess, error: FinError }),
  /** 6B-1 Task 11: probes a media asset's file again and commits one `asset.reprobe` op. */
  Rpc.make("asset.reprobe", { payload: AssetReprobePayload, success: AssetReprobeSuccess, error: FinError }),
  /** 6B-1 Task 14: a proxy of a video asset, and the state of its tiers. */
  Rpc.make("media.proxy", { payload: MediaProxyPayload, success: MediaProxySuccess, error: FinError }),
  Rpc.make("media.proxy.status", { payload: MediaProxyStatusPayload, success: MediaProxyStatusSuccess, error: FinError }),
  /** 6B-1 Task 15: thumbnails at source times, or the filmstrip, of a video asset. */
  Rpc.make("media.thumbs", { payload: MediaThumbsPayload, success: MediaThumbsSuccess, error: FinError }),
  /** 6C Task 18: an asset's `pcm48` or `pcm16m` feed (payloads in `audio.ts`). */
  Rpc.make("media.pcm", { payload: MediaPcmPayload, success: MediaPcmSuccess, error: FinError }),
  /** 6C Task 19: an asset's waveform peaks (payloads in `audio.ts`). */
  Rpc.make("media.waveform", { payload: MediaWaveformPayload, success: MediaWaveformSuccess, error: FinError }),
] as const;
