// `media.proxy`'s plan (6B-1 Task 14) over crates/fin-node/src/media/proxy.rs.
import { native, sync } from "./native.js";

/** A proxy's cache key before its job is submitted; see `media-proxy.d.ts`. */
export function mediaProxyPlan(request) {
  return JSON.parse(sync(() => native.mediaProxyPlan(JSON.stringify(request))));
}
