// Strip-types gate: every TypeScript file of the repository must load under Node's type
// stripping, which is how `node src/main.ts` runs finisherd (no build step). Node's own
// `module.stripTypeScriptTypes` (strip mode) is run over each `.ts`/`.mts`/`.cts` file except
// declaration files; syntax that is not erasable (a parameter property, an `enum`, a
// `namespace` with values, `import =`) fails it (STRIP_TYPES_FAILED). `tsc` catches the same
// with `erasableSyntaxOnly` (tsconfig.base.json); this gate also covers files outside the
// typechecked projects.
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const root = rootArg();
const violations = [];
const noWarning = process.emitWarning;
// The API warns that it is experimental once per process; that is noise here.
process.emitWarning = (w, ...rest) => (String(w).includes("stripTypeScriptTypes") ? undefined : noWarning.call(process, w, ...rest));
for (const path of walk(root)) {
  if (!/\.(ts|mts|cts)$/.test(path) || /\.d\.(ts|mts|cts)$/.test(path)) continue;
  try {
    stripTypeScriptTypes(readFileSync(join(root, path), "utf8"), { mode: "strip", sourceUrl: path });
  } catch (e) {
    violations.push(`STRIP_TYPES_FAILED: ${path}: ${e?.code ?? ""} ${String(e?.message ?? e).split("\n")[0]}`);
  }
}
report(violations);
