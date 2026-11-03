// Loader and error adapter for the fin-node napi binding (crates/fin-node).
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const here = fileURLToPath(new URL(".", import.meta.url));

/** Must match `ERROR_PREFIX` in crates/fin-node/src/lib.rs. */
const ERROR_PREFIX = "FIN_CORE_ERROR:";

function loadNative() {
  const base = `${process.platform}-${process.arch}`;
  // `napi build --platform` appends an ABI on Linux (gnu/musl) and Windows (msvc).
  const names = [base, `${base}-gnu`, `${base}-musl`, `${base}-msvc`].map((s) => `fin-node.${s}.node`);
  const file = names.map((n) => here + n).find((p) => existsSync(p));
  if (!file) {
    throw new Error(
      `@finisher/fin-node: no native binary for ${base} in ${here} (looked for ${names.join(", ")}); ` +
        "run `pnpm --filter @finisher/fin-node build`",
    );
  }
  return require(file);
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

  static async create(serverData, projectDir, name, rate) {
    return new Engine(await call(native.Engine.create(serverData, projectDir, name, rate)));
  }

  static async open(serverData, projectDir) {
    return new Engine(await call(native.Engine.open(serverData, projectDir)));
  }

  get projectId() {
    return this.#native.projectId;
  }

  head(branch) {
    return call(this.#native.head(branch));
  }

  commit(txJson) {
    return call(this.#native.commit(txJson));
  }

  snapshot(branch) {
    return call(this.#native.snapshot(branch));
  }

  opsSince(branch, rev) {
    return call(this.#native.opsSince(branch, rev));
  }

  async close() {
    await call(this.#native.close());
  }
}

export function probe(path) {
  return call(native.probe(path));
}
