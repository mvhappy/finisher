import { readFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const TS_IMPORT = /^\s*(?:import|export)\b[^;]*?from\s+["'](@finisher-cloud\/[^"']+)["']|^\s*import\s+["'](@finisher-cloud\/[^"']+)["']|\brequire\(\s*["'](@finisher-cloud\/[^"']+)["']\s*\)/;
const RUST_USE = /^\s*(?:pub\s+)?(?:use|extern\s+crate)\s+(finisher_cloud\w*)/;
const CARGO_DEP = /^\s*(finisher-cloud[\w-]*)\s*=/;

const root = rootArg();
const violations = [];
for (const path of walk(root)) {
  const kind = /\.(ts|tsx|mts|cts|js|mjs|cjs)$/.test(path) ? "ts" : path.endsWith(".rs") ? "rs" : path.endsWith("Cargo.toml") ? "cargo" : null;
  if (!kind) continue;
  for (const line of readFileSync(join(root, path), "utf8").split("\n")) {
    if (kind === "ts") {
      const m = line.match(TS_IMPORT);
      if (m) violations.push(`${path}: imports ${m[1] ?? m[2] ?? m[3]}`);
    } else if (kind === "rs") {
      const m = line.match(RUST_USE);
      if (m) violations.push(`${path}: uses ${m[1]}`);
    } else {
      const m = line.match(CARGO_DEP);
      if (m) violations.push(`${path}: depends on ${m[1]}`);
    }
  }
}
report(violations);
