// Loader and error adapter for the fin-node napi binding (crates/fin-node).
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { nativeFileNames } from "./loader.js";

const require = createRequire(import.meta.url);
const here = fileURLToPath(new URL(".", import.meta.url));
/** Tests point this at the `build:test` output (binary with the `test-hooks` feature). */
const binaryDir = process.env.FIN_NODE_BINARY_DIR ? `${process.env.FIN_NODE_BINARY_DIR.replace(/[\\/]$/, "")}/` : here;

/** Must match `ERROR_PREFIX` in crates/fin-node/src/lib.rs. */
const ERROR_PREFIX = "FIN_CORE_ERROR:";

function loadNative() {
  const base = `${process.platform}-${process.arch}`;
  const names = nativeFileNames(process.platform, process.arch);
  const files = names.map((n) => binaryDir + n).filter((p) => existsSync(p));
  if (files.length === 0) {
    throw new Error(
      `@finisher/fin-node: no native binary for ${base} in ${binaryDir} (looked for ${names.join(", ")}); ` +
        "run `pnpm --filter @finisher/fin-node build`",
    );
  }
  // The libc-matching binary is first; one that is present but does not load (the other libc) is skipped.
  let failure;
  for (const file of files) {
    try {
      return require(file);
    } catch (e) {
      failure ??= e;
    }
  }
  throw failure;
}

const native = loadNative();

/** Turns a rejected native call into an Error carrying the core `code`, `details` and `fix`. */
function toFinError(e) {
  if (!(e instanceof Error) || !e.message.startsWith(ERROR_PREFIX)) return e;
  let core;
  try {
    core = JSON.parse(e.message.slice(ERROR_PREFIX.length));
  } catch {
    return e;
  }
  const err = new Error(core.message);
  err.code = core.code;
  err.details = core.details ?? {};
  err.fix = core.fix ?? undefined;
  // Marks codes that come from fin-core/fin-store, so servers can pass them through verbatim.
  err.finNode = true;
  return err;
}

async function call(promise) {
  try {
    return await promise;
  } catch (e) {
    throw toFinError(e);
  }
}

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
    return new Engine(await call(native.Engine.open(serverData, projectDir, options.relocate === true)));
  }

  get projectId() {
    return this.#native.projectId;
  }

  head(branch) {
    return call(this.#native.head(branch));
  }

  commit(txJson, generationJson) {
    return call(generationJson === undefined ? this.#native.commit(txJson) : this.#native.commit(txJson, generationJson));
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

  acknowledgeRewind(branch, generation) {
    return call(this.#native.acknowledgeRewind(branch, generation));
  }

  acknowledgeSalvage(copy) {
    return call(this.#native.acknowledgeSalvage(copy));
  }

  branchCreate(reqJson) {
    return call(this.#native.branchCreate(reqJson));
  }

  branches(all) {
    return call(this.#native.branches(all === true));
  }

  branchSet(reqJson) {
    return call(this.#native.branchSet(reqJson));
  }

  resolveBranch(name) {
    return call(this.#native.resolveBranch(name));
  }

  undo(reqJson) {
    return call(this.#native.undo(reqJson));
  }

  redo(reqJson) {
    return call(this.#native.redo(reqJson));
  }

  checkpoint(reqJson) {
    return call(this.#native.checkpoint(reqJson));
  }

  merge(reqJson) {
    return call(this.#native.merge(reqJson));
  }

  cherryPick(reqJson) {
    return call(this.#native.cherryPick(reqJson));
  }

  revert(reqJson) {
    return call(this.#native.revert(reqJson));
  }

  recover(reqJson) {
    return call(this.#native.recover(reqJson));
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

  /** @internal Test builds only: emits a synthetic `rewound` event. */
  emitRewoundForTest(branch, fromRev) {
    this.#hook("emitRewoundForTest")(branch, fromRev);
  }

  /** @internal Test builds only: whether the maintenance thread runs. */
  maintenanceRunningForTest() {
    return this.#hook("maintenanceRunningForTest")();
  }

  /** @internal Test builds only: pins `Fonts` JSON as if main's head held them. */
  injectFontPinsForTest(fontsJson) {
    try {
      this.#hook("injectFontPinsForTest")(fontsJson);
    } catch (e) {
      throw toFinError(e);
    }
  }

  /** @internal Test builds only: forces two font status refreshes to interleave. */
  fontPublishRaceForTest(fontsJson) {
    try {
      this.#hook("fontPublishRaceForTest")(fontsJson);
    } catch (e) {
      throw toFinError(e);
    }
  }
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
  try {
    return native.lockExclusive(path);
  } catch (e) {
    throw toFinError(e);
  }
}

export function probe(path) {
  return call(native.probe(path));
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
    }
  : undefined;
