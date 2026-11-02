import { readFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const USES = /^\s*-?\s*uses:\s*([^\s#]+)/;
const PINNED = /@[0-9a-f]{40}$/;

const root = rootArg();
const violations = [];
for (const path of walk(root)) {
  if (!/^\.github\/(workflows|actions)\/.*\.ya?ml$/.test(path)) continue;
  for (const line of readFileSync(join(root, path), "utf8").split("\n")) {
    const m = line.match(USES);
    if (!m) continue;
    const ref = m[1];
    if (ref.startsWith("./") || ref.startsWith("docker://")) continue;
    if (!PINNED.test(ref)) violations.push(`${path}: ${ref} is not pinned to a commit SHA`);
  }
}
report(violations);
