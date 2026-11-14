// Types of `media-pcm.js` (re-exported by `index.d.ts`): the plan of `media.pcm` (6C plan Task 18,
// Decision 7).
/** What `mediaPcmPlan` is asked. */
export interface MediaPcmPlanRequest {
  readonly fingerprint: string;
  readonly recipe: "pcm48" | "pcm16m";
  /** The source's audio codec (`media.audio.codec` of its probe), when the probe names one. */
  readonly sourceCodec?: string | undefined;
  /** The source's channel count (`audio.channels`). */
  readonly channels: number;
}

/**
 * 6C Task 18 (Decision 7): the cache key a `media.pcm` job of the source will commit under,
 * with the audio decoder this build uses for its codec (symphonia without one), that decoder's
 * vendor class and the feed's layout (`mono` for one channel or `pcm16m`, else `stereo`).
 * Throws `CODEC_UNAVAILABLE` (`details.direction: "decode"`) when nothing here decodes the codec,
 * `ASSET_TYPE_UNSUPPORTED` for no channel, `INVALID_REQUEST` for a request that does not parse.
 */
export declare function mediaPcmPlan(request: MediaPcmPlanRequest): { key: string; decoder: string; decoderClass: "os" | "gpu" | "software" | "plugin"; layout: "mono" | "stereo" };
