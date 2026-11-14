// The audio `media.*` payloads of `FinRpcs` (6C), in `media-rpcs.ts`'s rows; `rpc.ts` stays
// untouched (it is at its size limit).
import { Schema } from "effect";

/**
 * 6C Task 18: an asset's audio as a PCM feed in the media cache: `pcm48` (default; 48 kHz `f32`
 * in the source's canonical layout, a mono source staying one channel) or `pcm16m` (16 kHz mono,
 * the analysis feed). A cache hit answers at once; otherwise a `media.pcm` job is submitted and,
 * unless `wait: false`, waited for (up to 10 minutes). `branch` defaults to `main`.
 */
export const MediaPcmPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
  recipe: Schema.optional(Schema.Literals(["pcm48", "pcm16m"])),
  wait: Schema.optional(Schema.Boolean),
});
/** `{asset, recipe, key, state: ready | making, cached?, job?, result?}` (`result` is the generated `PcmResult`). */
export const MediaPcmSuccess = Schema.Unknown;

/**
 * 6C Task 19: an asset's waveform peaks (`peaks.bin`: per channel `(min, max)` pairs at 256, 1,024,
 * 4,096, 16,384 and 65,536 samples per peak) in the media cache. A cache hit answers at once;
 * otherwise a `media.waveform` job is submitted (read from the asset's `pcm48` entry when the cache
 * has one) and, unless `wait: false`, waited for (up to 10 minutes). `branch` defaults to `main`.
 */
export const MediaWaveformPayload = Schema.Struct({
  projectId: Schema.String,
  asset: Schema.String,
  branch: Schema.optional(Schema.String),
  wait: Schema.optional(Schema.Boolean),
});
/** `{asset, key, levels, state: ready | making, cached?, job?, result?}` (`levels`: samples per peak; `result` is the generated `WaveResult`). */
export const MediaWaveformSuccess = Schema.Unknown;
