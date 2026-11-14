// Types of `media-proxy.js` (re-exported by `index.d.ts`): the plan of `media.proxy` (6B-1 Task 14,
// Decision 11).
/** What `mediaProxyPlan` is asked. */
export interface MediaProxyPlanRequest {
  readonly fingerprint: string;
  readonly tier: "540" | "1080";
  readonly codec: "h264" | "av1";
  /** The source's video codec (`av1`, `h264`, `hevc`, `vp9`, `proRes`, …). */
  readonly sourceCodec: string;
  /** The capability report's backends for that codec, in the order they are tried. */
  readonly decoders: ReadonlyArray<string>;
  /** The source's audio track (the probe's `media.container`, `media.audio.codec`); absent: none (6C Task 20). */
  readonly sourceAudio?: { readonly container: string; readonly codec: string } | undefined;
}

/**
 * 6B-1 Task 14 (Decision 11): the cache key a `media.proxy` job of the source will commit
 * under, with the decoder the report names first for its codec and that decoder's vendor
 * class, and what this build reads of the source's audio (6C Task 20). Throws
 * `CODEC_UNAVAILABLE` (`details.direction: "decode"`) when no listed backend decodes the codec,
 * `INVALID_REQUEST` for a request that does not parse.
 */
export declare function mediaProxyPlan(request: MediaProxyPlanRequest): {
  key: string;
  decoder: string;
  decoderClass: "os" | "gpu" | "software" | "plugin";
  /** What the key says of the source's audio (`ProxySourceAudio`): `media.proxy`'s `audio` param. */
  audio: "none" | "unavailable" | { decoded: "os" | "gpu" | "software" | "plugin" };
};
