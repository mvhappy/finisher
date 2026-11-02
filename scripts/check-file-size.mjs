import { readFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const SOURCE = /\.(rs|ts|tsx|mts|cts|js|mjs|cjs|py)$/;
const TEST = /(\.test\.|_test\.|(^|\/)tests\/)/;
const SOURCE_LIMIT = 600;
const TEST_LIMIT = 1500;

const root = rootArg();
const violations = [];
for (const path of walk(root)) {
  if (!SOURCE.test(path) || path.split("/").includes("generated")) continue;
  const count = readFileSync(join(root, path), "utf8").split("\n").length - 1;
  const limit = TEST.test(path) ? TEST_LIMIT : SOURCE_LIMIT;
  if (count > limit) violations.push(`${path}: ${count} lines > ${limit}`);
}
report(violations);
