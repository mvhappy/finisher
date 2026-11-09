// The fin-node napi binding (crates/fin-node): JS wrappers over `native.js`.
import { call, native, sync, toFinError } from "./native.js";
import { defineRegistry } from "./registry.js";

export {
  JobHost,
  jobKindVersions,
  resolveMemoryRule,
  validateJobParams,
  workersBinaryPath,
} from "./jobhost.js";
export {
  libraryCanonicalRoot,
  libraryProbeFile,
  libraryProbeRoot,
  libraryProbeStatus,
  libraryProbeVersion,
  libraryWatchStart,
} from "./library.js";
export { PipeServer } from "./pipe.js";
export { checkServerDataLocal, serverId } from "./server-data.js";
export * from "./sandbox.js";
export * from "./vault.js"; // 5B: the vault, the identity key; certificates and QR codes (`tls.js`)
export class Engine {
  #native;

  /** @internal Use `Engine.create` or `Engine.open`. */
  constructor(nativeEngine) {
    if (!(nativeEngine instanceof native.Engine)) throw new TypeError("use Engine.create or Engine.open");
    this.#native = nativeEngine;
  }

  static async create(serverData, projectDir, name, rate, options = {}) {
    return new Engine(await call(native.Engine.create(serverData, projectDir, name, rate, options.idemKey ?? null)));
  }

  static async open(serverData, projectDir, options = {}) {
    return new Engine(await call(native.Engine.open(serverData, projectDir, options.relocate === true, options.expectId ?? null)));
  }

  get projectId() {
    return this.#native.projectId;
  }

  head(branch) {
    return call(this.#native.head(branch));
  }

  commit(txJson, generationJson, metaJson) {
    return call(this.#native.commit(txJson, generationJson ?? null, metaJson ?? null));
  }

  snapshot(branch) {
    return call(this.#native.snapshot(branch));
  }

  opsSince(branch, rev) {
    return call(this.#native.opsSince(branch, rev));
  }

  opsSinceAt(branch, rev) {
    return call(this.#native.opsSinceAt(branch, rev));
  }

  status() {
    return call(this.#native.status());
  }

  txGet(txId) {
    return call(this.#native.txGet(txId));
  }

  acknowledgeRewind(branch, generation, metaJson) {
    return call(this.#native.acknowledgeRewind(branch, generation, metaJson));
  }

  acknowledgeSalvage(copy, metaJson) {
    return call(this.#native.acknowledgeSalvage(copy, metaJson));
  }

  branchCreate(reqJson, metaJson) {
    return call(this.#native.branchCreate(reqJson, metaJson ?? null));
  }

  branches(all) {
    return call(this.#native.branches(all === true));
  }

  branchSet(reqJson, metaJson) {
    return call(this.#native.branchSet(reqJson, metaJson ?? null));
  }

  resolveBranch(name) {
    return call(this.#native.resolveBranch(name));
  }

  undo(reqJson, metaJson) {
    return call(this.#native.undo(reqJson, metaJson ?? null));
  }

  redo(reqJson, metaJson) {
    return call(this.#native.redo(reqJson, metaJson ?? null));
  }

  checkpoint(reqJson, metaJson) {
    return call(this.#native.checkpoint(reqJson, metaJson ?? null));
  }

  merge(reqJson, metaJson) {
    return call(this.#native.merge(reqJson, metaJson ?? null));
  }

  cherryPick(reqJson, metaJson) {
    return call(this.#native.cherryPick(reqJson, metaJson ?? null));
  }

  revert(reqJson, metaJson) {
    return call(this.#native.revert(reqJson, metaJson ?? null));
  }

  recover(reqJson, metaJson) {
    return call(this.#native.recover(reqJson, metaJson ?? null));
  }

  takeover(confirm) {
    return call(this.#native.takeover(confirm));
  }

  verify() {
    return call(this.#native.verify());
  }

  rebuild() {
    return call(this.#native.rebuild());
  }

  compact() {
    return call(this.#native.compact());
  }

  onEvent(cb) {
    if (typeof cb !== "function") throw new TypeError("onEvent needs a function");
    // A throw here would reach the threadsafe-function trampoline and become an
    // uncaughtException; it is reported as a process warning instead.
    this.#native.onEvent((json) => {
      try {
        cb(json);
      } catch (e) {
        const err = e instanceof Error ? e : new Error(String(e));
        process.emitWarning(`an Engine.onEvent callback threw: ${err.message}`, {
          code: "FIN_NODE_EVENT_CALLBACK",
          detail: err.stack ?? err.message,
        });
      }
    });
  }

  evalFrame(branch, timeline, at) {
    return call(this.#native.evalFrame(branch, timeline ?? null, at));
  }

  evalAudio(branch, timeline, from, to) {
    return call(this.#native.evalAudio(branch, timeline ?? null, from, to));
  }

  fontsPlan(branch, opsJson) {
    return call(this.#native.fontsPlan(branch, opsJson));
  }

  prepareMediaJob(branch, timeline, at, inputDir, options) {
    return call(this.#native.prepareMediaJob(branch, timeline, at, inputDir, options ?? null));
  }

  setCollect(collect, metaJson) {
    return call(this.#native.setCollect(collect, metaJson));
  }

  async close() {
    await call(this.#native.close());
  }

  #hook(name) {
    if (!this.#native[name]) throw new Error("fin-node was built without the test-hooks feature");
    return this.#native[name].bind(this.#native);
  }

  /** @internal Test builds only: panics inside the native store call. */
  panicForTest() {
    return call(this.#hook("panicForTest")());
  }

  /** @internal Test builds only: the next maintenance tick panics. */
  panicNextTickForTest() {
    this.#hook("panicNextTickForTest")();
  }

  /** @internal Test builds only: the next long call keeps the store `ms` longer. */
  delayNextLongCallForTest(ms) {
    this.#hook("delayNextLongCallForTest")(ms);
  }

  /** @internal Test builds only: automatic work starts once the engine has been quiet for `ms` (30 s otherwise). */
  setAutoIdleForTest(ms) {
    this.#hook("setAutoIdleForTest")(ms);
  }

  /** @internal Test builds only: each automatic backup waits `ms` between its copy and its checks. */
  pauseAutoBackupForTest(ms) {
    this.#hook("pauseAutoBackupForTest")(ms);
  }

  /** @internal Test builds only: whether an automatic backup is running. */
  autoBackupRunningForTest() {
    return this.#hook("autoBackupRunningForTest")();
  }

  /** @internal Test builds only: emits a synthetic `rewound` event. */
  emitRewoundForTest(branch, fromRev) {
    this.#hook("emitRewoundForTest")(branch, fromRev);
  }

  /** @internal Test builds only: whether the maintenance thread runs. */
  maintenanceRunningForTest() {
    return this.#hook("maintenanceRunningForTest")();
  }

  /** @internal Test builds only: forces two font status refreshes to interleave around a commit of `txJson`. */
  fontPublishRaceForTest(txJson) {
    try {
      this.#hook("fontPublishRaceForTest")(txJson);
    } catch (e) {
      throw toFinError(e);
    }
  }
}

/** The diagnostics journal of a server data folder (the engines opened on it record into it). */
export class Journal {
  #native;

  /** @internal Use `Journal.open`. */
  constructor(nativeJournal) {
    if (!(nativeJournal instanceof native.Journal)) throw new TypeError("use Journal.open");
    this.#native = nativeJournal;
  }

  static open(serverData, privacyJson, hostname, testOptions) {
    const test = testOptions === undefined ? null : JSON.stringify(testOptions);
    return new Journal(sync(() => native.Journal.open(serverData, privacyJson ?? null, hostname, test)));
  }

  /** The agent-trace store of the folder (`<serverData>/agent-traces`); open `Journal.open` first. */
  static openTraces(serverData, privacyJson, hostname, testOptions) {
    const test = testOptions === undefined ? null : JSON.stringify(testOptions);
    return new Journal(sync(() => native.Journal.openTraces(serverData, privacyJson ?? null, hostname, test)));
  }

  get openWarning() {
    return this.#native.openWarning ?? undefined;
  }

  get training() {
    return this.#native.training;
  }

  recordsProject(projectId) {
    return sync(() => this.#native.recordsProject(projectId));
  }

  attachBlob(bytes, mediaType) {
    const body = sync(() => this.#native.attachBlob(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength), mediaType));
    return body === null || body === undefined ? undefined : JSON.parse(body);
  }

  get session() {
    return this.#native.session ?? undefined;
  }

  get enabled() {
    return this.#native.enabled;
  }

  applySettings(privacyJson) {
    return sync(() => this.#native.applySettings(privacyJson ?? null)) ?? undefined;
  }

  consent() {
    return sync(() => this.#native.consent());
  }

  startSession(startJson, timeoutMs = 100) {
    return sync(() => this.#native.startSession(startJson, timeoutMs));
  }

  async endSession(uptimeS) {
    return JSON.parse(await call(sync(() => this.#native.endSession(uptimeS))));
  }

  record(kind, kv, ctxJson, payloadJson, source, session) {
    sync(() => this.#native.record(kind, kv, ctxJson ?? null, payloadJson, source, session ?? null));
  }

  recordSync(kind, kv, ctxJson, payloadJson, source, timeoutMs) {
    return sync(() => this.#native.recordSync(kind, kv, ctxJson ?? null, payloadJson, source, timeoutMs));
  }

  observe(metric, key, micros, code) {
    this.#native.observe(metric, key, micros, code ?? null);
  }

  async status() {
    return call(sync(() => this.#native.status()));
  }

  statusNow() {
    return sync(() => this.#native.statusNow());
  }

  async export(filterJson, cursor, limit, asUpload, redact) {
    return call(sync(() => this.#native.export(filterJson, cursor ?? null, limit, asUpload ?? null, redact === true)));
  }

  async purge(selectorJson) {
    return call(sync(() => this.#native.purge(selectorJson)));
  }

  setProjectPolicy(projectId, policy) {
    sync(() => this.#native.setProjectPolicy(projectId, policy ?? null));
  }

  projectPolicies() {
    return sync(() => this.#native.projectPolicies());
  }

  async close() {
    await call(this.#native.close());
  }

  #hook(name) {
    if (!this.#native[name]) throw new Error("fin-node was built without the test-hooks feature");
    return this.#native[name].bind(this.#native);
  }

  /** @internal Test builds only. */
  panicOnThreadForTest(thread) {
    return this.#hook("panicOnThreadForTest")(thread);
  }

  /** @internal Test builds only. */
  panicWriterForTest() {
    this.#hook("panicWriterForTest")();
  }

  /** @internal Test builds only. */
  idRefForTest(raw) {
    return this.#hook("idRefForTest")(raw) ?? undefined;
  }

  /** @internal Test builds only. */
  pauseWriterForTest() {
    return this.#hook("pauseWriterForTest")();
  }

  /** @internal Test builds only. */
  resumeWriterForTest() {
    this.#hook("resumeWriterForTest")();
  }

  /** @internal Test builds only. */
  previousHookCallsForTest() {
    return this.#hook("previousHookCallsForTest")();
  }
}

/** fin-core's `RPC_METHODS`: the keys `rpc.stats` accepts (`other` for the rest). */
export function rpcMethods() {
  return native.rpcMethods();
}

/** The BLAKE3 of `data`, 64 lower-case hex digits (the `blake3` crate). */
export function blake3Hex(data) {
  return native.blake3Hex(data);
}

/** The BLAKE3 of `data` keyed with a 32-byte `key`, 64 lower-case hex digits. */
export function blake3KeyedHex(key, data) {
  return sync(() => native.blake3KeyedHex(key, data));
}

/** Every kind name of fin-core's event registry. */
export function kindNames() {
  return native.kindNames();
}

/** The newest payload version of kind `name` that `source` may emit; `null` for none (reserved, unknown, not that source). */
export function kindVersion(name, source) {
  return native.kindVersion(name, source) ?? null;
}

/** Why a `privacy` section (JSON) is not one the journal accepts, naming the key; `undefined` when it is. */
export function validatePrivacy(privacyJson) {
  return native.validatePrivacy(privacyJson) ?? undefined;
}

/** The font manager of a server data folder (shared with the engines opened on it). */
export class FontManager {
  #native;

  constructor(nativeManager) {
    if (!(nativeManager instanceof native.FontManager)) throw new TypeError("use FontManager.open");
    this.#native = nativeManager;
  }

  static open(serverData) {
    try {
      return new FontManager(native.FontManager.open(serverData));
    } catch (e) {
      throw toFinError(e);
    }
  }

  get autoFetch() {
    return this.#native.autoFetch;
  }

  setAutoFetch(on) {
    this.#native.setAutoFetch(on === true);
  }

  /** `[{id, catalogs: [text]}]` JSON → `{rejected: [{id, code, message, details}]}` JSON. */
  setExtraCatalogs(packsJson) {
    try {
      return this.#native.setExtraCatalogs(packsJson);
    } catch (e) {
      throw toFinError(e);
    }
  }

  catalog() {
    try {
      return this.#native.catalog();
    } catch (e) {
      throw toFinError(e);
    }
  }

  status() {
    return call(this.#native.status());
  }

  fetch(reqJson) {
    let promise;
    try {
      promise = this.#native.fetch(reqJson);
    } catch (e) {
      return Promise.reject(toFinError(e));
    }
    return call(promise);
  }
}

/**
 * Kernel-held exclusive lock (flock / LockFileEx) on `path`; throws `SERVER_RUNNING` if held.
 * Keep the returned `ServerLock` until shutdown, then call `release()`.
 */
export function lockExclusive(path) {
  return sync(() => native.lockExclusive(path));
}

/** Windows: owner and DACL private to this user; elsewhere `{ ok: true }`. */
export function checkPrivateDir(path) {
  const r = sync(() => native.checkPrivateDir(path));
  return { ok: r.ok, ...(r.owner == null ? {} : { owner: r.owner }), problems: r.problems };
}

/** Creates `path` private to this user (protected owner-only DACL on Windows, 0700 elsewhere). */
export function makePrivateDir(path) {
  sync(() => native.makePrivateDir(path));
}

/** A new file private to this user, checked before anything is written (5A Task 9, review M2). */
class PrivateFile {
  #native;
  constructor(n) {
    this.#native = n;
  }
  get path() {
    return this.#native.path;
  }
  commit(text) {
    sync(() => this.#native.commit(text));
  }
  discard() {
    this.#native.discard();
  }
}

/** Creates `path` new and private to this user; see `index.d.ts`. */
export function createPrivateFile(path) {
  return new PrivateFile(sync(() => native.createPrivateFile(path)));
}

/** Ends an agent/MCP credential (`<kind>:<id>`) for every store write of this process (5A Task 9). */
export function endCredential(credential) {
  sync(() => native.endCredential(credential));
}

export function probe(path) {
  return call(native.probe(path));
}

export function probeBeneath(root, path) {
  return call(native.probeBeneath(root, path));
}

export function commitJobOutput(from, to, maxBytes, copy) {
  return call(native.commitJobOutput(from, to, maxBytes, copy ?? null));
}

/** The plugin store and registries of a server (`registry.js`). */
export const Registry = defineRegistry({ native, call, toFinError });

export function validateManifest(dir, source) {
  return call(native.validateManifest(dir, source ?? null));
}

/** @internal Present only in binaries built with the `test-hooks` feature (`build:test`). */
export const testHooks = native.writeFixtureMp4ForTest
  ? {
      testFont(name) {
        try {
          return native.testFontForTest(name);
        } catch (e) {
          throw toFinError(e);
        }
      },
      measure(serverData, requestJson) {
        try {
          return native.measureForTest(serverData, requestJson);
        } catch (e) {
          throw toFinError(e);
        }
      },
      writeFixtureMp4(path, { seconds = 2, fps = 25, audio = true, sampleRate = 48000 } = {}) {
        try {
          native.writeFixtureMp4ForTest(path, seconds, fps, audio, sampleRate);
        } catch (e) {
          throw toFinError(e);
        }
      },
      writeFixture(kind, path) {
        try {
          native.writeFixtureForTest(kind, path);
        } catch (e) {
          throw toFinError(e);
        }
      },
      heldPaths(pid) {
        try {
          return native.heldPathsForTest(pid);
        } catch (e) {
          throw toFinError(e);
        }
      },
      libraryProbeHang: (root, hang) => native.libraryProbeHangForTest(root, hang),
      libraryProbeStarted: (root) => native.libraryProbeStartedForTest(root),
      packPlugin(src, out) {
        try {
          return native.packPluginForTest(src, out);
        } catch (e) {
          throw toFinError(e);
        }
      },
      buildRegistry(dir, keysDir, specJson) {
        try {
          return native.buildRegistryForTest(dir, keysDir, specJson);
        } catch (e) {
          throw toFinError(e);
        }
      },
      treeHash(dir) {
        try {
          return native.treeHashForTest(dir);
        } catch (e) {
          throw toFinError(e);
        }
      },
      publishToRegistry(dir, keysDir, specJson) {
        try {
          native.publishToRegistryForTest(dir, keysDir, specJson);
        } catch (e) {
          throw toFinError(e);
        }
      },
    }
  : undefined;
