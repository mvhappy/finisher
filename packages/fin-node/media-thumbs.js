// `media.thumbs`' plan (6B-1 Task 15) over crates/fin-node/src/media/thumbs.rs.
import { native, sync } from "./native.js";

/** Thumbnails' or a filmstrip's cache key before its job is submitted; see `media-thumbs.d.ts`. */
export function mediaThumbsPlan(request) {
  return JSON.parse(sync(() => native.mediaThumbsPlan(JSON.stringify(request))));
}
