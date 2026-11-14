import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { rootArg } from "./walk.mjs";

// `cargo fmt --all --check` over the workspace at --root; the diff goes to stdout. On Windows one
// `cargo fmt -p <package>` at a time: `--all` hands one rustfmt every target's file, longer than a
// Windows command line (32 767 characters: "The filename or extension is too long").
const root = rootArg();
const manifest = join(root, "Cargo.toml");

function packages() {
  const m = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (m.error || m.status !== 0) throw new Error(`cargo metadata failed: ${m.error?.message ?? m.stderr}`);
  return JSON.parse(m.stdout).packages.map((p) => p.name);
}

// `--per-package` takes the Windows path on any OS (its test).
const perPackage = process.platform === "win32" || process.argv.includes("--per-package");
const runs = perPackage ? packages().map((p) => ["-p", p]) : [["--all"]];
for (const which of runs) {
  const r = spawnSync("cargo", ["fmt", ...which, "--check", "--manifest-path", manifest], { encoding: "utf8" });
  if (r.error || r.status !== 0) {
    process.stdout.write(r.stdout ?? "");
    process.stderr.write(r.stderr ?? "");
    console.log(`RUST_FMT_DRIFT: run \`cargo fmt --all\`${r.error ? ` (${r.error.message})` : ""}`);
    process.exitCode = 1;
  }
}
