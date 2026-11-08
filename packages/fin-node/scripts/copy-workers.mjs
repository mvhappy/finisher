// Builds `fin-workers` and the 7C launcher `fin-sandbox-exec` (release) and copies them beside
// the fin-node `.node` file (5C-1 plan, Decision 2; plan 7C Task 5): `packages/fin-node/` for `build`, `packages/fin-node/test/native/` with the
// test families (`--test`, feature `test-hooks`) for `build:test`. The cargo target directory
// comes from `cargo metadata`, so `CARGO_TARGET_DIR` is honoured. Needs no network.
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, renameSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const pkg = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(pkg, "../..");
const test = process.argv.includes("--test");

function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: repo, stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (r.error || r.status !== 0) {
    console.error(`copy-workers: ${cmd} ${args.join(" ")} failed${r.error ? `: ${r.error.message}` : ""}`);
    process.exit(r.status ?? 1);
  }
  return r.stdout;
}

const features = test ? ["--features", "test-hooks"] : [];
run("cargo", ["build", "--release", "-p", "fin-workers", ...features]);
// The 7C launcher (plan 7C Task 5) ships beside the workers. Test builds honour the status stub.
run("cargo", ["build", "--release", "-p", "fin-sandbox", "--bin", "fin-sandbox-exec", ...features]);
const target = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--no-deps"])).target_directory;
const out = test ? join(pkg, "test", "native") : pkg;
mkdirSync(out, { recursive: true });
for (const name of ["fin-workers", "fin-sandbox-exec"]) {
  const exe = process.platform === "win32" ? `${name}.exe` : name;
  // Copy then rename, so a running test never sees a half-written binary.
  const tmp = join(out, `.${exe}.tmp`);
  copyFileSync(join(target, "release", exe), tmp);
  if (process.platform !== "win32") chmodSync(tmp, 0o755);
  renameSync(tmp, join(out, exe));
}
