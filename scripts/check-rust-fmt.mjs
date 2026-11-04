import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { rootArg } from "./walk.mjs";

// `cargo fmt --all --check` over the workspace at --root; the diff goes to stdout.
const root = rootArg();
const r = spawnSync("cargo", ["fmt", "--all", "--check", "--manifest-path", join(root, "Cargo.toml")], {
  encoding: "utf8",
});
if (r.error || r.status !== 0) {
  process.stdout.write(r.stdout ?? "");
  process.stderr.write(r.stderr ?? "");
  console.log(`RUST_FMT_DRIFT: run \`cargo fmt --all\`${r.error ? ` (${r.error.message})` : ""}`);
  process.exitCode = 1;
}
