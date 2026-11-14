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

/** A native call's synchronous throw, as the rejection of the promise it would return. */
function promised(f) {
  let promise;
  try {
    promise = f();
  } catch (e) {
    return call(Promise.reject(e));
  }
  return call(promise);
}

export class JobHost {
  #native;
  /** Pending spawns: their number → `{ resolve, reject }`. */
  #spawns;
  #nextSpawn = 0;

  /** @internal Use `JobHost.start`. */
  constructor(nativeHost, spawns) {
    if (!(nativeHost instanceof native.JobHost) || !(spawns instanceof Map)) throw new TypeError("use JobHost.start");
    this.#native = nativeHost;
    this.#spawns = spawns;
  }

  static async start(opts, onEvent) {
    if (typeof onEvent !== "function") throw new TypeError("JobHost.start needs an onEvent function");
    const spawns = new Map();
    // A throw here would reach the threadsafe-function trampoline and become an
    // uncaughtException; it is reported as a process warning instead.
    const deliver = (text) => {
      try {
        const e = JSON.parse(text);
        // A spawn's answer, handed over before any event of its process.
        if (e.type === "_spawn") {
          const pending = spawns.get(e.spawn);
          spawns.delete(e.spawn);
          pending?.resolve(e.proc);
          return;
        }
        onEvent(e);
      } catch (e) {
        const err = e instanceof Error ? e : new Error(String(e));
        process.emitWarning(`a JobHost onEvent callback threw: ${err.message}`, {
          code: "FIN_NODE_EVENT_CALLBACK",
          detail: err.stack ?? err.message,
        });
      }
    };
    return new JobHost(await promised(() => native.JobHost.start(json(opts), deliver)), spawns);
  }

  caps() {
    return JSON.parse(sync(() => this.#native.caps()));
  }

  aclGrants(req) {
    return JSON.parse(sync(() => this.#native.aclGrants(json(req))));
  }

  openGroup(spec) {
    return sync(() => this.#native.openGroup(json(spec)));
  }

  spawn(group, req) {
    const n = ++this.#nextSpawn;
    const answered = new Promise((resolve, reject) => this.#spawns.set(n, { resolve, reject }));
    // Resolves (with nothing) or rejects; the id comes with the `_spawn` answer.
    promised(() => this.#native.spawn(group, json(req), n)).catch((e) => {
      const pending = this.#spawns.get(n);
      this.#spawns.delete(n);
      pending?.reject(e);
    });
    return answered;
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
    return promised(() => this.#native.killGroup(group, reason));
  }

  closeGroup(group) {
    sync(() => this.#native.closeGroup(group));
  }

  shutdown(graceMs) {
    return promised(() => this.#native.shutdown(graceMs));
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
