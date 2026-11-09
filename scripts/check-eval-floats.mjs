// Evaluation determinism (plan 4C Global Constraints, spec §5.4): every fin-core source file
// (eval, params, layout, time, model, ops, …) may use only f64 + − × ÷, sqrt, comparisons, abs,
// min/max, floor and conversions from integers. Transcendental and fused functions are refused
// as method calls (`x.sin()`, even split across lines) and as paths under any qualifier
// (`f64::sin`, `F::sin` through an alias, `T::powf`, `<f64 as Tr>::powf`, `Self::ln`). A macro
// that names the function by metavariable (`f64::$f`, `x.$f()`) is refused too, since the name
// can't be checked; so are libm and the float constants (which only serve them), so plans hash the same on every OS and on wasm32. Comments and string
// literals are not checked.
//
// Render determinism (sub-project 6 spec §13.4, plan 6A Task 7): the hashed render modules
// (`fin-media`'s colour conversion, HDR tone mapping and soft clip (`colour/tonemap.rs`,
// `colour/softclip.rs`, plan 6B-1 Task 10) and sampling rule, `fin-compositor`'s geometry,
// coverage, mip levels and CPU reference) refuse the same platform transcendental and fused calls (methods
// and paths), the metavariable forms and SIMD intrinsics (`std::arch`/`core::arch`), but must
// use the pinned `libm::` for transcendentals, so `libm::sin` and the float constants pass there.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const SCOPE = /^crates\/fin-core\/src\/.*\.rs$/;
const RENDER =
  /^crates\/(fin-media\/src\/(colour\/.*|sample)|fin-compositor\/src\/(geometry|coverage|mips|reference\/.*))\.rs$/;
// Render modules that must exist: a rename out of the scope above fails instead of escaping it.
const REQUIRED = ["crates/fin-media/src/colour/tonemap.rs", "crates/fin-media/src/colour/softclip.rs"];
const FNS =
  "sin|cos|tan|sin_cos|asin|acos|atan|atan2|sinh|cosh|tanh|asinh|acosh|atanh|exp|exp2|exp_m1|ln|ln_1p|log|log2|log10|powf|powi|cbrt|hypot|mul_add|gamma|ln_gamma|erf|erfc";
const RULES = [
  // Whole-file regexes (\s spans newlines), so `x.sin\n()` is one call.
  [new RegExp(`\\.\\s*(${FNS})\\s*\\(`, "g"), (m) => `calls .${m[1]}()`],
  [new RegExp(`::\\s*(${FNS})\\b`, "g"), (m) => `calls ::${m[1]}`],
  [/\b(f32|f64)\s*(?:as\s+[\w:]+\s*)?>?\s*::\s*\$/g, (m) => `calls ${m[1]}::$metavariable`],
  [/\.\s*\$\w+\s*\(/g, () => "calls a metavariable method .$f()"],
  [/\blibm\s*::/g, () => "uses libm"],
  [/\bf(?:32|64)\s*::\s*consts\b/g, () => "uses the float constants"],
];
// The render rules: as above, but `libm::f` and the constants are allowed, SIMD is refused.
const RENDER_RULES = [
  RULES[0],
  [new RegExp(`(?<!\\blibm\\s*)::\\s*(${FNS})\\b`, "g"), (m) => `calls ::${m[1]}`],
  RULES[2],
  RULES[3],
  [/\b(?:std|core)\s*::\s*arch\b/g, () => "uses SIMD intrinsics"],
];

/** The source with comments and string/char literals blanked (newlines kept). */
function code(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const next = src[i + 1];
    if (c === "/" && next === "/") {
      while (i < src.length && src[i] !== "\n") i++;
    } else if (c === "/" && next === "*") {
      let depth = 1;
      i += 2;
      while (i < src.length && depth) {
        if (src[i] === "/" && src[i + 1] === "*") (depth++, (i += 2));
        else if (src[i] === "*" && src[i + 1] === "/") (depth--, (i += 2));
        else out += src[i++] === "\n" ? "\n" : "";
      }
    } else if (c === "r" && /^r#*"/.test(src.slice(i, i + 258)) && !/\w/.test(src[i - 1] ?? "")) {
      const hashes = src.slice(i + 1).match(/^#*/)[0];
      const end = src.indexOf(`"${hashes}`, i + 2 + hashes.length);
      const stop = end === -1 ? src.length : end + 1 + hashes.length;
      out += src.slice(i, stop).replace(/[^\n]/g, "") + '""';
      i = stop;
    } else if (c === '"') {
      i++;
      while (i < src.length && src[i] !== '"') {
        if (src[i] === "\\") i++;
        else if (src[i] === "\n") out += "\n";
        i++;
      }
      i++;
      out += '""';
    } else if (c === "'" && /^'(?:\\.|[^\\'\n])'/u.test(src.slice(i, i + 4))) {
      // A char literal ('"', '\''); a lifetime has no closing quote.
      i += src[i + 1] === "\\" ? 4 : 3;
      out += "' '";
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

function violations(path, src, rules = RULES, what = "eval floats: + − × ÷ sqrt only") {
  const text = code(src);
  const lineOf = (i) => text.slice(0, i).split("\n").length;
  const hits = [];
  for (const [re, name] of rules)
    for (const m of text.matchAll(re)) hits.push([lineOf(m.index), name(m)]);
  hits.sort((a, b) => a[0] - b[0]);
  return hits.map(([n, name]) => `${path}:${n}: ${name} (${what})`);
}

const root = rootArg();
const found = [];
const paths = [...walk(root)];
// Checked where `fin-media`'s colour module exists (fixture trees without it skip the check).
if (paths.includes("crates/fin-media/src/colour/mod.rs")) for (const r of REQUIRED) if (!paths.includes(r)) found.push(`${r}: missing (a hashed render module; render floats)`);
for (const path of paths) {
  if (SCOPE.test(path)) found.push(...violations(path, readFileSync(join(root, path), "utf8")));
  else if (RENDER.test(path)) found.push(...violations(path, readFileSync(join(root, path), "utf8"), RENDER_RULES, "render floats: libm:: for transcendentals"));
}
report(found);
