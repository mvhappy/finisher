// finisherd's own code reaches `node:fs` and `node:child_process` through ordinary imports only
// (plan 7D Task 11): the store spy (`agent-claude-account.test.ts`
// `finisherd_never_opens_sign_in_store`) wraps those imports, and a module loaded by hand would
// get past it. No `process.getBuiltinModule` and no `createRequire` under `apps/finisherd/src/`.
// Each finding is `MODULE_ESCAPE <path>:<line>: <text>`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg, walk } from "./walk.mjs";

export const ROOT = "apps/finisherd/src/";
export const FORBIDDEN = [/\bgetBuiltinModule\b/, /\bcreateRequire\b/, /\bmodule\s*\.\s*require\b/];

/** Whether `path` (repo-relative, "/") is checked. */
export function checked(path) {
  return path.startsWith(ROOT) && /\.(?:[cm]?[jt]s|tsx)$/.test(path);
}

/** The findings in one file's text (comment lines are skipped). */
export function findings(path, text) {
  const out = [];
  text.split("\n").forEach((line, n) => {
    if (/^\s*(?:\/\/|\*)/.test(line)) return;
    if (FORBIDDEN.some((re) => re.test(line))) out.push(`MODULE_ESCAPE ${path}:${n + 1}: ${line.trim().slice(0, 120)}`);
  });
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const violations = [];
  for (const path of walk(root)) if (checked(path)) violations.push(...findings(path, readFileSync(join(root, path), "utf8")));
  report(violations);
}
