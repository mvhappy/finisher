// Agent processes start only through the session sandbox (plan 7C Task 11, spec §8): no
// process-starting module under `apps/finisherd/src/agent/` except `sandbox.ts`, the built-in
// `fin` tool (`builtin/fin-tool.ts`, our own `fin` with the session's credential, outside the OS
// sandbox as spec §8.9 says) and the built-in environment's `fin` command map
// (`builtin/environment.ts`: our own `fin` with no credential and only `PATH`). Each finding is
// `AGENT_SPAWN <path>:<line>: <module>`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg, walk } from "./walk.mjs";

export const ROOT = "apps/finisherd/src/agent/";
export const ALLOWED = new Set([`${ROOT}sandbox.ts`, `${ROOT}builtin/fin-tool.ts`, `${ROOT}builtin/environment.ts`]);

/** A static or dynamic import, or a `require`, of a module that starts processes. */
const MODULE = String.raw`(?:node:)?(?:child_process|cluster|worker_threads)`;
export const FORBIDDEN = [
  new RegExp(String.raw`\bfrom\s*["']${MODULE}["']`),
  new RegExp(String.raw`\bimport\s*\(\s*["']${MODULE}["']\s*\)`),
  new RegExp(String.raw`\brequire\s*\(\s*["']${MODULE}["']\s*\)`),
  new RegExp(String.raw`^\s*import\s*["']${MODULE}["']`),
  // Bun's and Deno's own process APIs, `process.binding("spawn_sync")` and a `require` made
  // by hand. A lint, not a boundary: a helper module outside `agent/` still gets past it.
  /\bBun\.spawn(?:Sync)?\b|\bDeno\.(?:Command|run)\b|process\.binding\s*\(|\bcreateRequire\b/,
];

/** Whether `path` (repo-relative, "/") is checked. */
export function checked(path) {
  return path.startsWith(ROOT) && /\.(?:[cm]?[jt]s|tsx)$/.test(path) && !ALLOWED.has(path);
}

/** The findings in one file's text. */
export function findings(path, text) {
  const out = [];
  text.split("\n").forEach((line, n) => {
    if (/^\s*(?:\/\/|\*)/.test(line)) return;
    if (FORBIDDEN.some((re) => re.test(line))) out.push(`AGENT_SPAWN ${path}:${n + 1}: ${line.trim().slice(0, 120)}`);
  });
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const violations = [];
  for (const path of walk(root)) if (checked(path)) violations.push(...findings(path, readFileSync(join(root, path), "utf8")));
  report(violations);
}
