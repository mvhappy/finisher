import { spawnSync } from "node:child_process";

const steps = [
  ["node", ["scripts/check-file-size.mjs"]],
  ["node", ["scripts/check-cloud-imports.mjs"]],
  ["node", ["scripts/check-action-pins.mjs"]],
  ["node", ["scripts/check-js-licenses.mjs"]],
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
