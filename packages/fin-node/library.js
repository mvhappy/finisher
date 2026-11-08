// The library calls beyond the root probes (5D Task 13): `asset.probe` of a catalogued file.
import { call, native } from "./native.js";

export function libraryProbeFile(root, rel, expect, hydrate, display) {
  return call(native.libraryProbeFile(root, JSON.stringify(rel), JSON.stringify(expect), hydrate === true, display));
}
