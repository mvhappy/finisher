// The library calls (5D): root resolution and probes, the root watch, and `asset.probe` of a
// catalogued file (Task 13).
import { call, native, sync } from "./native.js";

export function libraryProbeFile(root, rel, expect, hydrate, display) {
  return call(native.libraryProbeFile(root, JSON.stringify(rel), JSON.stringify(expect), hydrate === true, display));
}

export function libraryCanonicalRoot(path, serverData, timeoutMs) {
  return call(native.libraryCanonicalRoot(path, serverData, timeoutMs ?? null));
}

export function libraryProbeRoot(root, identityJson, timeoutMs) {
  return call(native.libraryProbeRoot(root, identityJson, timeoutMs ?? null));
}

export function libraryProbeStatus() {
  return native.libraryProbeStatus();
}

export function libraryProbeVersion() {
  return native.libraryProbeVersion();
}

export function libraryWatchStart(root, maxWatches, callback) {
  return sync(() => native.libraryWatchStart(root, maxWatches, callback));
}
