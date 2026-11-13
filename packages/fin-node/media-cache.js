// The media cache (6B-1 Task 13) over crates/fin-node/src/media/cache: every call runs on the
// cache's own thread; JSON answers are parsed here.
import { call, native, toFinError } from "./native.js";

/** The most bytes one `readFile` returns (`crates/fin-node/src/media/cache/read.rs`). */
export const MEDIA_READ_MAX = 1 << 20;

const json = async (p) => {
  const text = await call(p);
  return text == null ? null : JSON.parse(text);
};

export class MediaCache {
  #native;

  /** @internal Use `MediaCache.open`. */
  constructor(nativeCache) {
    if (!(nativeCache instanceof native.MediaCache)) throw new TypeError("use MediaCache.open");
    this.#native = nativeCache;
  }

  static async open(serverData, capBytes) {
    let opened;
    try {
      opened = await native.mediaCacheOpen(serverData, capBytes ?? null);
    } catch (e) {
      throw toFinError(e);
    }
    return new MediaCache(opened);
  }

  openReport() {
    return JSON.parse(this.#native.openReport());
  }

  entryDir(key) {
    return this.#native.entryDir(key) ?? null;
  }

  lookup(key) {
    return json(this.#native.lookup(key));
  }

  commit(jobOutDir, key, projects) {
    return json(this.#native.commit(jobOutDir, key, [...(projects ?? [])]));
  }

  serve(key, projects) {
    return call(this.#native.serve(key, [...(projects ?? [])]));
  }

  pin(key, holder) {
    return call(this.#native.pin(key, holder));
  }

  unpin(key, holder) {
    return call(this.#native.unpin(key, holder));
  }

  release(holder) {
    return call(this.#native.release(holder));
  }

  pinProject(projectId) {
    return call(this.#native.pinProject(projectId));
  }

  unpinProject(projectId) {
    return call(this.#native.unpinProject(projectId));
  }

  pinsOf(key) {
    return call(this.#native.pinsOf(key));
  }

  readFile(key, file, offset, len) {
    return call(this.#native.readFile(key, file, offset, len));
  }

  attachProxies(inDir, holder, projects) {
    return call(this.#native.attachProxies(inDir, holder, [...projects]));
  }

  entriesFor(asset) {
    return json(this.#native.entriesFor(asset.fingerprint));
  }

  projectsOf(key) {
    return call(this.#native.projectsOf(key));
  }

  status() {
    return json(this.#native.status());
  }

  gc(targetBytes) {
    return json(this.#native.gc(targetBytes ?? null));
  }

  setCap(capBytes) {
    return json(this.#native.setCap(capBytes ?? null));
  }

  /** @internal Test builds only. */
  setFaultsForTest(faults) {
    if (!this.#native.setFaultsForTest) throw new Error("fin-node was built without the test-hooks feature");
    return call(this.#native.setFaultsForTest(JSON.stringify(faults)));
  }
}
