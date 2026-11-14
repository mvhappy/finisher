// `media.proxy`'s plan (6B-1 Task 14) over crates/fin-node/src/media/proxy.rs.
import { native, sync } from "./native.js";

/** A proxy's cache key before its job is submitted; see `media-proxy.d.ts`. */
export function mediaProxyPlan(request) {
  // Absent optional fields are left out (the request is parsed strictly).
  const req = Object.fromEntries(Object.entries(request).filter(([, v]) => v !== undefined));
  return JSON.parse(sync(() => native.mediaProxyPlan(JSON.stringify(req))));
}
