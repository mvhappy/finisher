import { spawnSync } from "node:child_process";
import { listedCrates } from "./check-clippy-config.mjs";

const steps = [
  ["node", ["scripts/check-file-size.mjs"]],
  ["node", ["scripts/check-cloud-imports.mjs"]],
  ["node", ["scripts/check-action-pins.mjs"]],
  ["node", ["scripts/check-js-licenses.mjs"]],
  ["node", ["scripts/check-font-catalog.mjs"]],
  ["node", ["scripts/check-eval-floats.mjs"]],
  ["node", ["scripts/check-wasm-deps.mjs"]],
  ["node", ["scripts/check-generated-types.mjs"]],
  // The exported command registry (7B Decision 1): regenerated with `fin schema --registry`.
  ["node", ["scripts/check-registry.mjs"]],
  ["node", ["scripts/check-typecheck.mjs"]],
  // The contracts' runtime envelope schemas against the generated Rust types.
  ["node", ["scripts/check-envelopes.mjs"]],
  // The fin-node tarball carries the runtime files, not its tests or the test-hooks binaries.
  ["node", ["scripts/check-pack-files.mjs"]],
  ["node", ["scripts/check-strip-types.mjs"]],
  ["node", ["scripts/check-rust-fmt.mjs"]],
  ["node", ["scripts/check-test-hooks.mjs"]],
  // Workers never start processes (5C-1 plan Decision 24).
  ["node", ["scripts/check-worker-spawn.mjs"]],
  ["node", ["scripts/check-agent-spawn.mjs"]],
  // No hand-loaded `fs`/`child_process` under apps/finisherd/src (the store spy's blind spot, 7D Task 11).
  ["node", ["scripts/check-module-escapes.mjs"]],
  // One function decides which encoder a purpose may use; encoders open only through
  // fin_codecs::open_encoder (6B-1 plan Decision 2).
  ["node", ["scripts/check-encoder-call-sites.mjs"]],
  ["node", ["scripts/check-native-deps.mjs"]],
  ["node", ["scripts/check-notices.mjs"]],
  ["node", ["scripts/check-runtime-libs.mjs"]],
  ["node", ["scripts/check-crate-deps.mjs"]],
  ["node", ["scripts/check-clippy-config.mjs"]],
  // The library-reader gate (plan 5D Decision 21): the crates that handle library paths, with
  // `disallowed-methods` denied (the unlisted `allow`s are check-clippy-config's).
  ["cargo", ["clippy", "-q", ...listedCrates(".").flatMap((c) => ["-p", c]), "--all-targets", "--", "-D", "clippy::disallowed_methods"]],
  ["cargo", ["build", "-q", "-p", "fin-core", "--target", "wasm32-unknown-unknown"]],
  ["cargo", ["build", "-q", "-p", "fin-text", "--target", "wasm32-unknown-unknown"]],
  ["cargo", ["build", "-q", "-p", "fin-plugin-api", "--target", "wasm32-unknown-unknown"]],
  ["cargo", ["build", "-q", "-p", "fin-media", "-p", "fin-container", "--target", "wasm32-unknown-unknown"]],
  // The audio crate: mixer, resampler, effects and limiter build for the browser too (6C).
  ["cargo", ["build", "-q", "-p", "fin-audio", "--target", "wasm32-unknown-unknown"]],
  // The compositor builds for the browser too (spec §5.3); 6A checks it, fin-wasm (6F) links it.
  ["cargo", ["check", "-q", "-p", "fin-compositor", "--features", "reference", "--target", "wasm32-unknown-unknown"]],
];
const hasCargoDeny = spawnSync("cargo", ["deny", "--version"], { stdio: "ignore" }).status === 0;
// Advisories (review #46) need the RustSec database: fetched (or updated) into
// ~/.cargo/advisory-dbs, or with FIN_DENY_OFFLINE=1 (`--offline`) only the cached copy is used.
const denyOffline = process.env.FIN_DENY_OFFLINE === "1" ? ["--offline"] : [];
if (process.platform !== "win32" && hasCargoDeny)
  steps.push(["cargo", ["deny", ...denyOffline, "check", "advisories", "licenses", "sources", "bans"]]);
else console.log("gates: skipping cargo deny (not installed or Windows)");

let failed = 0;
for (const [cmd, args] of steps) {
  const { status } = spawnSync(cmd, args, { stdio: "inherit" });
  if (status !== 0) {
    console.log(`gates: ${cmd} ${args.join(" ")} failed`);
    failed++;
  }
}
process.exitCode = failed ? 1 : 0;
