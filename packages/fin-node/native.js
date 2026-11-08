// Loads the fin-node napi binding (crates/fin-node) and adapts its errors; shared by index.js
// and jobhost.js.
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { nativeFileNames } from "./loader.js";

const require = createRequire(import.meta.url);
const here = fileURLToPath(new URL(".", import.meta.url));
/** Tests point this at the `build:test` output (binary with the `test-hooks` feature). */
export const binaryDir = process.env.FIN_NODE_BINARY_DIR ? `${process.env.FIN_NODE_BINARY_DIR.replace(/[\\/]$/, "")}/` : here;

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

export const native = loadNative();

/** Turns a rejected native call into an Error carrying the core `code`, `details` and `fix`. */
export function toFinError(e) {
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

export async function call(promise) {
  try {
    return await promise;
  } catch (e) {
    throw toFinError(e);
  }
}

/** Runs a synchronous native call, turning a thrown core error into a `FinNodeError`. */
export function sync(f) {
  try {
    return f();
  } catch (e) {
    throw toFinError(e);
  }
}
