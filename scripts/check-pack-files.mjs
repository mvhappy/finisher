// Pack gate: what `pnpm pack` would put in the fin-node tarball. The package lists its `files`
// (package.json), so its tests, build scripts and the test-hooks binaries under test/native
// (24 MB each) stay out; this runs `pnpm pack --dry-run` and fails (PACK_CONTENTS) when the
// tarball holds any of them, or lacks the entry points.
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { report, rootArg } from "./walk.mjs";

const PACKAGE = "packages/fin-node";
const MUST_HAVE = ["index.js", "index.d.ts", "package.json"];
const MUST_NOT = [/^test\//, /^scripts\//, /\.test\.[cm]?[jt]s$/, /^tsconfig[^/]*\.json$/];

const dir = join(rootArg(), PACKAGE);
// `pnpm` is a .cmd shim on Windows, which only a shell can run.
const run = spawnSync("pnpm", ["pack", "--dry-run", "--json"], { cwd: dir, encoding: "utf8", shell: process.platform === "win32" });
const violations = [];
let files = [];
try {
  // Lifecycle output may precede the JSON document.
  const text = run.stdout.slice(run.stdout.indexOf("{"));
  files = JSON.parse(text).files.map((f) => f.path.split("\\").join("/"));
} catch {
  violations.push(`PACK_CONTENTS: \`pnpm pack --dry-run\` in ${PACKAGE} gave no file list\n${(run.stdout + run.stderr).trim()}`);
}
for (const path of files) {
  if (MUST_NOT.some((re) => re.test(path))) violations.push(`PACK_CONTENTS: ${PACKAGE} would pack ${path}; list the shipped files in package.json "files"`);
}
if (files.length === 0 && violations.length === 0) violations.push(`PACK_CONTENTS: \`pnpm pack --dry-run\` in ${PACKAGE} listed no file`);
if (files.length) {
  for (const path of MUST_HAVE) {
    if (!files.includes(path)) violations.push(`PACK_CONTENTS: ${PACKAGE} would not pack ${path}`);
  }
}
report(violations);
