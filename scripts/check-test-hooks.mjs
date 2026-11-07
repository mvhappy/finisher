import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { rootArg, walk } from "./walk.mjs";

// Test hooks must never reach a shipped binary.
// - `test-hooks` features (`fin-store/test-hooks`: fault injection, the passive follower;
//   `fin/test-hooks`: `FIN_OFFLINE_SNAPSHOT`, `FIN_TEST_COMMIT_GATE`): the normal (non-dev)
//   feature graph of the workspace's default members must not enable one (TEST_HOOKS_IN_BUILD).
//   Only dev-dependencies (fin's self dev-dependency) and `fin-store-xhost`, built alone, do.
// - fin's hook variables are read only through `backend::test_hook`, which is off without the
//   feature: a direct `env::var`/`var_os` of one in `crates/fin/src` is TEST_HOOK_UNGUARDED.
const root = rootArg();
let failed = false;
const r = spawnSync(
  "cargo",
  ["tree", "-e", "features,normal", "--prefix", "none", "--manifest-path", join(root, "Cargo.toml")],
  { encoding: "utf8" },
);
if (r.error || r.status !== 0) {
  process.stdout.write(r.stdout ?? "");
  process.stderr.write(r.stderr ?? "");
  console.log(`TEST_HOOKS_IN_BUILD: cargo tree failed${r.error ? ` (${r.error.message})` : ""}`);
  failed = true;
} else if (/ feature "test-hooks"/.test(r.stdout)) {
  const lines = r.stdout.split("\n").filter((l) => l.includes('feature "test-hooks"'));
  console.log(`TEST_HOOKS_IN_BUILD: the default members' build enables ${[...new Set(lines)].join(", ")}; keep crates that enable it out of default-members`);
  failed = true;
}

// `cargo tree` does not show a root package's own features: no manifest may turn
// `test-hooks` on by default.
const DEFAULT_HOOKS = /^\s*default\s*=\s*\[[^\]]*"test-hooks"/m;
for (const file of walk(root)) {
  if (!/^crates\/[^/]+\/Cargo\.toml$/.test(file)) continue;
  if (DEFAULT_HOOKS.test(readFileSync(join(root, file), "utf8"))) {
    console.log(`TEST_HOOKS_IN_BUILD: ${file} enables "test-hooks" by default`);
    failed = true;
  }
}

const HOOKS = /env::var(?:_os)?\(\s*(?:SNAPSHOT_ENV|COMMIT_GATE_ENV|"FIN_OFFLINE_SNAPSHOT"|"FIN_TEST_COMMIT_GATE")/;
for (const file of walk(root)) {
  if (!file.startsWith("crates/fin/src/") || !file.endsWith(".rs")) continue;
  readFileSync(join(root, file), "utf8")
    .split("\n")
    .forEach((line, i) => {
      if (HOOKS.test(line)) {
        console.log(`TEST_HOOK_UNGUARDED: ${file}:${i + 1} reads a test hook directly; use backend::test_hook`);
        failed = true;
      }
    });
}
if (failed) process.exitCode = 1;
