import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { rootArg, walk } from "./walk.mjs";

// Test hooks must never reach a shipped binary.
// - `test-hooks` features (`fin-store/test-hooks`: fault injection, the passive follower;
//   `fin/test-hooks`: `FIN_OFFLINE_SNAPSHOT`, `FIN_TEST_COMMIT_GATE`) and
//   `fin-registry/test-http` (the HTTP transport for `tuf-conformance`): the normal (non-dev)
//   feature graph of the workspace's default members must not enable one (TEST_HOOKS_IN_BUILD).
//   Only dev-dependencies (fin's self dev-dependency), `fin-store-xhost`, built alone, and
//   `fin-registry`'s own tests and conformance example do.
// - fin's hook variables (`FIN_OFFLINE_SNAPSHOT`, any `FIN_TEST_*`) are read only through
//   `backend::test_hook`, which is off without the feature: a direct `env::var`/`var_os` of
//   one in `crates/fin/src`, by name or through its `*_ENV` constant, is TEST_HOOK_UNGUARDED.
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
} else if (/ feature "(test-hooks|test-http)"/.test(r.stdout)) {
  const lines = r.stdout
    .split("\n")
    .filter((l) => l.includes('feature "test-hooks"') || l.includes('feature "test-http"'));
  console.log(`TEST_HOOKS_IN_BUILD: the default members' build enables ${[...new Set(lines)].join(", ")}; keep crates that enable them out of default-members`);
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

// Hook names: `FIN_OFFLINE_SNAPSHOT` and every `FIN_TEST_*`, as literals or through the
// `*_ENV` constants fin defines for them.
const HOOK_NAME = /^(FIN_OFFLINE_SNAPSHOT|FIN_TEST_[A-Z0-9_]+)$/;
const finSources = [...walk(root)].filter((f) => f.startsWith("crates/fin/src/") && f.endsWith(".rs"));
const texts = new Map(finSources.map((f) => [f, readFileSync(join(root, f), "utf8")]));
const hookConsts = new Set();
for (const text of texts.values()) {
  for (const m of text.matchAll(/const\s+([A-Z0-9_]+_ENV)\s*:\s*&str\s*=\s*"([A-Z0-9_]+)"/g)) {
    if (HOOK_NAME.test(m[2])) hookConsts.add(m[1]);
  }
}
const READ = /env::var(?:_os)?\(\s*(?:"([A-Z0-9_]+)"|(?:[a-z_][a-z0-9_]*::)*([A-Z0-9_]+_ENV)\b)/g;
for (const [file, text] of texts) {
  text.split("\n").forEach((line, i) => {
    for (const m of line.matchAll(READ)) {
      if ((m[1] !== undefined && HOOK_NAME.test(m[1])) || (m[2] !== undefined && hookConsts.has(m[2]))) {
        console.log(`TEST_HOOK_UNGUARDED: ${file}:${i + 1} reads a test hook directly; use backend::test_hook`);
        failed = true;
      }
    }
  });
}
if (failed) process.exitCode = 1;
