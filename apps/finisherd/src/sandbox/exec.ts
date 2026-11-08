// Where the 7C launcher `fin-sandbox-exec` is (plan 7C Task 5): beside the bundled binaries
// (fin-node's directory, with `fin-workers` and `fin`), else a checkout's
// `target/release` or `target/debug`. Session wiring (Task 11) starts agents and hooks through it.
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { sandboxExecPath } from "@finisher/fin-node";

const exeIn = (dir: string) => join(dir, process.platform === "win32" ? "fin-sandbox-exec.exe" : "fin-sandbox-exec");

/** The launcher's path; the last candidate is answered even when missing (the start then fails). */
export function resolveSandboxExecPath(): string {
  const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
  const candidates = [sandboxExecPath(), exeIn(join(repo, "target", "release")), exeIn(join(repo, "target", "debug"))];
  return candidates.find((p) => existsSync(p)) ?? candidates[candidates.length - 1]!;
}
