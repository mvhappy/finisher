// `media.pcm`'s plan (6C Task 18) over crates/fin-node/src/media/pcm.rs.
import { native, sync } from "./native.js";

/** A PCM feed's cache key before its job is submitted; see `media-pcm.d.ts`. */
export function mediaPcmPlan(request) {
  // Absent optional fields are left out (the request is parsed strictly).
  const req = Object.fromEntries(Object.entries(request).filter(([, v]) => v !== undefined));
  return JSON.parse(sync(() => native.mediaPcmPlan(JSON.stringify(req))));
}

/** A waveform's cache key before its job is submitted; see `media-pcm.d.ts`. */
export function mediaWavePlan(request) {
  return JSON.parse(sync(() => native.mediaWavePlan(JSON.stringify(request))));
}
