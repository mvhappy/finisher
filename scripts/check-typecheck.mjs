// Typecheck gate: `tsc --noEmit -p` for every TypeScript project of the repository. Any
// diagnostic fails the gate (TYPECHECK_FAILED). Uses the repository's own TypeScript.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { report, rootArg } from "./walk.mjs";

const PROJECTS = ["apps/finisherd", "packages/contracts", "packages/core-types", "packages/fin-node", "packages/plugin-sdk"];

const root = rootArg();
const tsc = join(createRequire(import.meta.url).resolve("typescript/package.json"), "..", "bin", "tsc");

const violations = [];
for (const dir of PROJECTS) {
  if (!existsSync(join(root, dir, "tsconfig.json"))) {
    violations.push(`TYPECHECK_FAILED: ${dir}/tsconfig.json is missing`);
    continue;
  }
  const run = spawnSync(process.execPath, [tsc, "--noEmit", "-p", join(root, dir)], { cwd: root, encoding: "utf8" });
  if (run.status !== 0) {
    violations.push(`TYPECHECK_FAILED: ${dir}\n${(run.stdout + run.stderr).trim()}`);
  }
}
report(violations);
