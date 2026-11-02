import { readdirSync } from "node:fs";
import { join, relative } from "node:path";

const SKIP_DIRS = new Set(["target", "node_modules", "dist", "out", ".git", ".superpowers"]);

/** Yields repository-relative paths of files under root, skipping build and vendor dirs. */
export function* walk(root, dir = root) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      yield* walk(root, join(dir, entry.name));
    } else if (entry.isFile()) {
      yield relative(root, join(dir, entry.name));
    }
  }
}

export function rootArg(argv = process.argv) {
  const i = argv.indexOf("--root");
  return i === -1 ? process.cwd() : argv[i + 1];
}

export function report(violations) {
  for (const v of violations) console.log(v);
  process.exitCode = violations.length ? 1 : 0;
}
