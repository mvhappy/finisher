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
}

/**
 * 6B-1 Task 14 (Decision 11): the cache key a `media.proxy` job of the source will commit
 * under, with the decoder the report names first for its codec and that decoder's vendor
 * class. Throws `CODEC_UNAVAILABLE` (`details.direction: "decode"`) when no listed backend
 * decodes the codec, `INVALID_REQUEST` for a request that does not parse.
 */
export declare function mediaProxyPlan(request: MediaProxyPlanRequest): { key: string; decoder: string; decoderClass: "os" | "gpu" | "software" | "plugin" };
