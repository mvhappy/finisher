// The job host binding (5C-1 plan, Task 6): `JobHost` over crates/fin-node/src/jobhost, params
// validation and the worker binary's path. Values cross as JSON; this file parses them.
import { binaryDir, call, native, sync } from "./native.js";
import { workersBinaryPath as pathIn } from "./loader.js";

function json(value) {
  return JSON.stringify(value ?? null);
}

/** `fin-workers[.exe]` in `dir` (default: the directory the fin-node binary was loaded from). */
export function workersBinaryPath(dir) {
  return pathIn(dir ?? binaryDir, process.platform);
}

/** Decodes `params` with the kind's Rust params type. */
export function validateJobParams(kind, version, params) {
  return JSON.parse(sync(() => native.validateJobParams(kind, version, json(params))));
}

/** The kinds and versions `validateJobParams` knows. */
export function jobKindVersions() {
  return native.jobKindVersions();
}

/** A group memory rule in bytes for `totalBytes` of RAM; `undefined` without a bound. */
export function resolveMemoryRule(rule, totalBytes) {
  return sync(() => native.resolveMemoryRule(json(rule), totalBytes)) ?? undefined;
}

export class JobHost {
  #native;

  /** @internal Use `JobHost.start`. */
  constructor(nativeHost) {
    if (!(nativeHost instanceof native.JobHost)) throw new TypeError("use JobHost.start");
    this.#native = nativeHost;
  }

  static async start(opts, onEvent) {
    if (typeof onEvent !== "function") throw new TypeError("JobHost.start needs an onEvent function");
    // A throw here would reach the threadsafe-function trampoline and become an
    // uncaughtException; it is reported as a process warning instead.
    const deliver = (text) => {
      try {
        onEvent(JSON.parse(text));
      } catch (e) {
        const err = e instanceof Error ? e : new Error(String(e));
        process.emitWarning(`a JobHost onEvent callback threw: ${err.message}`, {
          code: "FIN_NODE_EVENT_CALLBACK",
          detail: err.stack ?? err.message,
        });
      }
    };
    const started = sync(() => native.JobHost.start(json(opts), deliver));
    return new JobHost(await call(started));
  }

  caps() {
    return sync(() => this.#native.caps());
  }

  openGroup(spec) {
    return sync(() => this.#native.openGroup(json(spec)));
  }

  spawn(group, req) {
    let promise;
    try {
      promise = this.#native.spawn(group, json(req));
    } catch (e) {
      return call(Promise.reject(e));
    }
    return call(promise);
  }

  send(proc, msg) {
    return sync(() => this.#native.send(proc, json(msg)));
  }

  watch(proc, w) {
    sync(() => this.#native.watch(proc, w == null ? null : json(w)));
  }

  closeStdin(proc) {
    sync(() => this.#native.closeStdin(proc));
  }

  killGroup(group, reason) {
    let promise;
    try {
      promise = this.#native.killGroup(group, reason);
    } catch (e) {
      return call(Promise.reject(e));
    }
    return call(promise);
  }

  closeGroup(group) {
    sync(() => this.#native.closeGroup(group));
  }

  shutdown(graceMs) {
    return call(this.#native.shutdown(graceMs));
  }

  /** @internal Test builds only. */
  get killProcForTest() {
    return this.#native.killProcForTest ? (proc) => sync(() => this.#native.killProcForTest(proc)) : undefined;
  }

  /** @internal Test builds only. `holdMs` defaults to 60 s. */
  get abandonForTest() {
    return this.#native.abandonForTest
      ? (proc, holdMs = 60_000) => sync(() => this.#native.abandonForTest(proc, holdMs))
      : undefined;
  }

  /** @internal Test builds only. */
  get pauseEventsForTest() {
    return this.#native.pauseEventsForTest ? (ms) => this.#native.pauseEventsForTest(ms) : undefined;
  }
}
