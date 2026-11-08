// The sandbox status (plan 7C Task 3).
import { sandboxExecPath as pathIn } from "./loader.js";
import { binaryDir, native, sync } from "./native.js";

/** `fin-sandbox-exec[.exe]` in `dir` (default: the directory the fin-node binary was loaded from). */
export function sandboxExecPath(dir) {
  return pathIn(dir ?? binaryDir, process.platform);
}

export function sandboxStatus() {
  return JSON.parse(sync(() => native.sandboxStatus()));
}
