// The media cache plans of `media.proxy` (6B-1 Task 14) and this build's versions, over
// crates/fin-node/src/media/proxy.rs; moved out of `index.js` to keep it small.
import { native, sync } from "./native.js";

/** A proxy's cache key before its job is submitted (6B-1 Task 14); see `media-proxy.d.ts`. */
export function mediaProxyPlan(request) {
  // Absent optional fields are left out (the request is parsed strictly).
  const req = Object.fromEntries(Object.entries(request).filter(([, v]) => v !== undefined));
  return JSON.parse(sync(() => native.mediaProxyPlan(JSON.stringify(req))));
}

/** This build's `RENDER_VERSION`, `AUDIO_VERSION` and cache recipe versions; see `media-proxy.d.ts`. */
export function mediaVersions() {
  return JSON.parse(sync(() => native.mediaVersions()));
}
