// Where a checkout's cargo-built binaries are. `CARGO_TARGET_DIR` (empty counts as unset) is
// searched before `<repo>/target`, so a build sent elsewhere (a worktree's shared target dir) wins
// over a stale in-tree one; a relative value is taken against the repository, as scripts/e2e/lib.mjs does.
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** The checkout's root, from this file's location (apps/finisherd/src). */
export const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

/** `release` then `debug` directories of each cargo target dir to search, best first. */
export function cargoProfileDirs(repo: string = REPO_ROOT, env: Record<string, string | undefined> = process.env): string[] {
  const roots = [join(repo, "target")];
  const custom = env.CARGO_TARGET_DIR;
  if (custom) roots.unshift(resolve(repo, custom));
  return [...new Set(roots)].flatMap((r) => [join(r, "release"), join(r, "debug")]);
}
