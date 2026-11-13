// Where the 7C launcher `fin-sandbox-exec` is (plan 7C Task 5): beside the bundled binaries
// (fin-node's directory, with `fin-workers` and `fin`), else a checkout's
// `release` or `debug` directory under `CARGO_TARGET_DIR` and then `target/`. Session wiring (Task 11) starts agents and hooks through it.
import { existsSync } from "node:fs";
import { join } from "node:path";
import { cargoProfileDirs } from "../cargo-target.ts";
import { sandboxExecPath } from "@finisher/fin-node";

const exeIn = (dir: string) => join(dir, process.platform === "win32" ? "fin-sandbox-exec.exe" : "fin-sandbox-exec");

/** The launcher's path; the last candidate is answered even when missing (the start then fails). */
export function resolveSandboxExecPath(env: Record<string, string | undefined> = process.env, repo?: string): string {
  const candidates = [sandboxExecPath(), ...cargoProfileDirs(repo, env).map(exeIn)];
  return candidates.find((p) => existsSync(p)) ?? candidates[candidates.length - 1]!;
}
