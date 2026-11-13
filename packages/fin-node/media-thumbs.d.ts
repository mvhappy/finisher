// Types of `media-thumbs.js` (re-exported by `index.d.ts`): the plan of `media.thumbs` (6B-1 plan
// Task 15, Decision 11). The request and the answer are fin-media's `ThumbsPlanRequest` and
// `ThumbsPlan`, generated into `@finisher/core-types`.
import type { ThumbsPlan, ThumbsPlanRequest } from "@finisher/core-types";

/** What `mediaThumbsPlan` is asked (the generated `ThumbsPlanRequest`). */
export type MediaThumbsPlanRequest = ThumbsPlanRequest;

/**
 * 6B-1 Task 15 (Decision 11): the cache key a `media.thumbs` job of the source will commit under,
 * from the original's codec (the decoder the report names first, and its vendor class) or from the
 * decoder class a `1080` proxy entry records; the mode with a filmstrip's sprite count filled in,
 * and the job's items. Throws `CODEC_UNAVAILABLE` when no listed backend decodes the codec,
 * `INVALID_REQUEST` for a request that does not parse or gives both or neither sources.
 */
export declare function mediaThumbsPlan(request: ThumbsPlanRequest): ThumbsPlan;
