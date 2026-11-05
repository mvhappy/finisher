// Drift gate: regenerates @finisher/core-types into a temp dir and fails if the committed
// schema or generated TypeScript differ (GENERATED_TYPES_DRIFT). Needs no network.
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { report, rootArg } from "./walk.mjs";

const FILES = ["packages/core-types/schema/fin-core.schema.json", "packages/core-types/src/generated/index.ts"];

const root = rootArg();
const tmp = mkdtempSync(join(tmpdir(), "fin-types-"));
try {
  const run = spawnSync(process.execPath, [join(root, "packages/core-types/scripts/generate.mjs"), "--out", join(tmp, "packages/core-types")], {
    cwd: root,
    encoding: "utf8",
  });
  if (run.status !== 0) {
    console.log(`GENERATED_TYPES_DRIFT: generation failed\n${run.stdout}${run.stderr}`);
    process.exitCode = 1;
  } else {
    const violations = [];
    for (const file of FILES) {
      const committed = existsSync(join(root, file)) ? readFileSync(join(root, file), "utf8") : null;
      if (committed !== readFileSync(join(tmp, file), "utf8")) {
        violations.push(`GENERATED_TYPES_DRIFT: ${file} differs from the Rust types' output; run \`pnpm --filter @finisher/core-types generate\``);
      }
    }
    report(violations);
  }
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
