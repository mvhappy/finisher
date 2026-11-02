import { readdirSync, realpathSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join, relative } from "node:path";

const SKIP_DIRS = new Set(["target", "node_modules", "dist", "out", ".git", ".superpowers"]);

/** Yields repository-relative paths of files under root, skipping build and vendor dirs. */
/** Repository files: tracked plus untracked-but-not-ignored when root is a git checkout,
 * otherwise a plain directory walk. Gitignored build output is never checked. */
export function* walk(root) {
  const git = spawnSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { cwd: root, encoding: "utf8" });
  if (git.status === 0 && realpath(spawnSync("git", ["rev-parse", "--show-toplevel"], { cwd: root, encoding: "utf8" }).stdout.trim() || ".") === realpath(root)) {
    yield* git.stdout.split("\0").filter(Boolean);
    return;
  }
  yield* walkDir(root);
}

function realpath(p) {
  return realpathSync.native(p);
}

function* walkDir(root, dir = root) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      yield* walkDir(root, join(dir, entry.name));
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
