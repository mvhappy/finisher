import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { rootArg } from "./walk.mjs";

// `fin-store/test-hooks` (fault injection, the passive follower) and `fin-registry/test-http`
// (the HTTP transport for `tuf-conformance`) must never reach a shipped binary: the normal
// (non-dev) feature graph of the workspace's default members must not enable either
// (TEST_HOOKS_IN_BUILD). Only `fin-store-xhost`, built alone, enables `test-hooks`; only
// `fin-registry`'s own tests and its conformance example enable `test-http`.
const root = rootArg();
const r = spawnSync(
  "cargo",
  ["tree", "-e", "features,normal", "--prefix", "none", "--manifest-path", join(root, "Cargo.toml")],
  { encoding: "utf8" },
);
if (r.error || r.status !== 0) {
  process.stdout.write(r.stdout ?? "");
  process.stderr.write(r.stderr ?? "");
  console.log(`TEST_HOOKS_IN_BUILD: cargo tree failed${r.error ? ` (${r.error.message})` : ""}`);
  process.exitCode = 1;
} else if (/ feature "(test-hooks|test-http)"/.test(r.stdout)) {
  const lines = r.stdout
    .split("\n")
    .filter((l) => l.includes('feature "test-hooks"') || l.includes('feature "test-http"'));
  console.log(`TEST_HOOKS_IN_BUILD: the default members' build enables ${[...new Set(lines)].join(", ")}; keep crates that enable them out of default-members`);
  process.exitCode = 1;
}
