import { spawnSync } from "node:child_process";

const steps = [
  ["node", ["scripts/check-file-size.mjs"]],
  ["node", ["scripts/check-cloud-imports.mjs"]],
  ["node", ["scripts/check-action-pins.mjs"]],
  ["node", ["scripts/check-js-licenses.mjs"]],
  ["node", ["scripts/check-font-catalog.mjs"]],
  ["node", ["scripts/check-eval-floats.mjs"]],
  ["node", ["scripts/check-wasm-deps.mjs"]],
  ["node", ["scripts/check-generated-types.mjs"]],
  ["node", ["scripts/check-typecheck.mjs"]],
  ["node", ["scripts/check-strip-types.mjs"]],
  ["node", ["scripts/check-rust-fmt.mjs"]],
  ["node", ["scripts/check-test-hooks.mjs"]],
  ["node", ["scripts/check-native-deps.mjs"]],
  ["node", ["scripts/check-runtime-libs.mjs"]],
  ["node", ["scripts/check-crate-deps.mjs"]],
  ["cargo", ["build", "-q", "-p", "fin-core", "--target", "wasm32-unknown-unknown"]],
  ["cargo", ["build", "-q", "-p", "fin-text", "--target", "wasm32-unknown-unknown"]],
  ["cargo", ["build", "-q", "-p", "fin-media", "-p", "fin-container", "--target", "wasm32-unknown-unknown"]],
  // The compositor builds for the browser too (spec §5.3); 6A checks it, fin-wasm (6F) links it.
  ["cargo", ["check", "-q", "-p", "fin-compositor", "--features", "reference", "--target", "wasm32-unknown-unknown"]],
];
const hasCargoDeny = spawnSync("cargo", ["deny", "--version"], { stdio: "ignore" }).status === 0;
if (process.platform !== "win32" && hasCargoDeny) steps.push(["cargo", ["deny", "check", "licenses", "sources", "bans"]]);
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
