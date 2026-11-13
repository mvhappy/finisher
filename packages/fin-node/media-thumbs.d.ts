// Types of `media-thumbs.js` (re-exported by `index.d.ts`): the plan of `media.thumbs` (6B-1 plan
// Task 15, Decision 11).

/** What `mediaThumbsPlan` is asked. */
export interface MediaThumbsPlanRequest {
  readonly fingerprint: string;
  /** `{thumbs: {times, maxSide}}` or `{strip: {tileHeight, sprites?}}` (`sprites` 0 or absent: from `duration`). */
  readonly mode: { readonly thumbs: { readonly times: ReadonlyArray<string>; readonly maxSide: number } } | { readonly strip: { readonly tileHeight: number; readonly sprites?: number } };
  readonly sourceCodec: string;
  readonly decoders: ReadonlyArray<string>;
  /** The source's duration (`MediaInfo.duration`, a rational string); a filmstrip needs it. */
  readonly duration?: string | undefined;
}

/**
 * 6B-1 Task 15 (Decision 11): the cache key a `media.thumbs` job of the source will commit under,
 * the decoder (and its vendor class) the report names first, the mode with a filmstrip's sprite
 * count filled in, and the job's items. Throws `CODEC_UNAVAILABLE` when no listed backend decodes
 * the codec, `INVALID_REQUEST` for a request that does not parse.
 */
export declare function mediaThumbsPlan(request: MediaThumbsPlanRequest): {
  key: string;
  decoder: string;
  decoderClass: "os" | "gpu" | "software" | "plugin";
  mode: MediaThumbsPlanRequest["mode"];
  items: string[];
};
