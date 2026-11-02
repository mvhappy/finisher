import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { report } from "./walk.mjs";

const ALLOWED = new Set([
  "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "MPL-2.0",
  "CC0-1.0", "CC-BY-4.0", "Unlicense", "BlueOak-1.0.0", "Python-2.0", "Zlib",
]);

function arg(name) {
  const i = process.argv.indexOf(name);
  return i === -1 ? undefined : process.argv[i + 1];
}

/** True when every alternative in an SPDX "OR" expression set includes an allowed licence. */
function permitted(expr) {
  const clean = expr.replace(/[()]/g, "").trim();
  return clean.split(/\s+OR\s+/).some((alt) => alt.split(/\s+AND\s+/).every((id) => ALLOWED.has(id.trim())));
}

const input = arg("--input");
const json = input
  ? readFileSync(input, "utf8")
  : execFileSync("pnpm", ["licenses", "list", "--json", "--prod"], { encoding: "utf8" });
const allowlist = JSON.parse(readFileSync(arg("--allowlist") ?? new URL("./license-allowlist.json", import.meta.url), "utf8"));

const violations = [];
for (const [license, pkgs] of Object.entries(JSON.parse(json))) {
  if (permitted(license)) continue;
  for (const pkg of pkgs) {
    if (allowlist[pkg.name]) continue;
    for (const version of pkg.versions ?? [pkg.version]) {
      violations.push(`${pkg.name}@${version}: ${license} is not allowed`);
    }
  }
}
report(violations);
