// The sandbox status (plan 7C Task 3) and the session wiring's data (Task 11).
import { sandboxExecPath as pathIn } from "./loader.js";
import { binaryDir, native, sync } from "./native.js";

/** `fin-sandbox-exec[.exe]` in `dir` (default: the directory the fin-node binary was loaded from). */
export function sandboxExecPath(dir) {
  return pathIn(dir ?? binaryDir, process.platform);
}

export function sandboxStatus() {
  return JSON.parse(sync(() => native.sandboxStatus()));
}

export function sandboxSessionGitConfig() {
  return native.sandboxSessionGitConfig();
}

export function sandboxToolchainBase() {
  return native.sandboxToolchainBase();
}

export function sandboxAgentCheck(input) {
  return native.sandboxAgentCheck(JSON.stringify(input)) ?? null;
}
