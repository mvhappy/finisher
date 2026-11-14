// Drift gate: regenerates the exported command registry with `fin schema --registry` (built by
// `cargo run --locked`) and fails if the committed packages/contracts/registry.json
// differs (REGISTRY_DRIFT). `--write` rewrites the file instead. `--generator <script.mjs>` runs
// that script for the registry instead of cargo (the gate's own tests). `--locked` refuses a
// Cargo.lock that needs updating (a fresh machine may still fetch the locked crates).
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg } from "./walk.mjs";

const FILE = "packages/contracts/registry.json";

const root = rootArg();
const gen = process.argv.indexOf("--generator");
const [cmd, args] =
  gen === -1
    ? ["cargo", ["run", "--locked", "-q", "-p", "fin", "--", "schema", "--registry"]]
    : [process.execPath, [process.argv[gen + 1]]];
const run = spawnSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
if (run.status !== 0) {
  console.log(`REGISTRY_DRIFT: generation failed\n${run.stdout}${run.stderr}`);
  process.exitCode = 1;
} else {
  let fresh;
  try {
    fresh = `${JSON.stringify(JSON.parse(run.stdout), null, 2)}\n`;
  } catch (e) {
    console.log(`REGISTRY_DRIFT: fin schema --registry printed no JSON: ${e.message}`);
    process.exit(1);
  }
  if (process.argv.includes("--write")) {
    writeFileSync(join(root, FILE), fresh);
  } else {
    const committed = existsSync(join(root, FILE)) ? readFileSync(join(root, FILE), "utf8") : null;
    report(committed === fresh ? [] : [`REGISTRY_DRIFT: ${FILE} differs from \`fin schema --registry\`; run \`node scripts/check-registry.mjs --write\``]);
  }
}
