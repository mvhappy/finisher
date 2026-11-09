// Typecheck gate: `tsc --noEmit -p` for every workspace package (the directories
// `pnpm-workspace.yaml` lists that have a package.json). A package without a tsconfig.json, or
// a workspace that lists none, fails the gate; so does any diagnostic (TYPECHECK_FAILED). Uses
// the repository's own TypeScript.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { report, rootArg } from "./walk.mjs";

/**
 * The workspace packages, as repository-relative directories, sorted. Reads the `packages:`
 * list of `pnpm-workspace.yaml`: plain directories and `dir/*` globs (the only forms the
 * workspace uses; anything else throws so a new form cannot silently drop a package).
 */
function workspacePackages(root) {
  const lines = readFileSync(join(root, "pnpm-workspace.yaml"), "utf8").split(/\r?\n/);
  const start = lines.findIndex((l) => /^packages:\s*$/.test(l));
  const dirs = new Set();
  for (const line of start === -1 ? [] : lines.slice(start + 1)) {
    if (/^\S/.test(line)) break; // the next top-level key
    const item = line.match(/^\s+-\s+["']?([^"'#]+?)["']?\s*(#.*)?$/);
    if (!item) continue;
    const pattern = item[1];
    if (pattern.startsWith("!")) throw new Error(`pnpm-workspace.yaml: negated pattern ${pattern} is not supported`);
    if (pattern.endsWith("/*") && !pattern.slice(0, -2).match(/[*?[{]/)) {
      const parent = pattern.slice(0, -2);
      if (!existsSync(join(root, parent))) continue;
      for (const e of readdirSync(join(root, parent), { withFileTypes: true })) {
        if (e.isDirectory() && existsSync(join(root, parent, e.name, "package.json"))) dirs.add(`${parent}/${e.name}`);
      }
    } else if (!pattern.match(/[*?[{]/)) {
      if (existsSync(join(root, pattern, "package.json"))) dirs.add(pattern.replace(/\/$/, ""));
    } else {
      throw new Error(`pnpm-workspace.yaml: pattern ${pattern} is not supported (use dir or dir/*)`);
    }
  }
  return [...dirs].sort();
}

const root = rootArg();
const tsc = join(createRequire(import.meta.url).resolve("typescript/package.json"), "..", "bin", "tsc");
const projects = workspacePackages(root);

const violations = [];
if (projects.length === 0) violations.push("TYPECHECK_FAILED: pnpm-workspace.yaml lists no package");
for (const dir of projects) {
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
