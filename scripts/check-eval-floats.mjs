// Evaluation determinism (plan 4C Global Constraints, spec §5.4): every fin-core source file
// (eval, params, layout, time, model, ops, …) may use only f64 + − × ÷, sqrt, comparisons, abs,
// min/max, floor and conversions from integers. Transcendental and fused functions are refused
// as method calls (`x.sin()`, even split across lines) and as paths under any qualifier
// (`f64::sin`, `F::sin` through an alias, `T::powf`, `<f64 as Tr>::powf`, `Self::ln`). A macro
// that names the function by metavariable (`f64::$f`, `x.$f()`) is refused too, since the name
// can't be checked; so are libm and the float constants (which only serve them), so plans hash the same on every OS and on wasm32. Comments and string
// literals are not checked.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { report, rootArg, walk } from "./walk.mjs";

const SCOPE = /^crates\/fin-core\/src\/.*\.rs$/;
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

function violations(path, src) {
  const text = code(src);
  const lineOf = (i) => text.slice(0, i).split("\n").length;
  const hits = [];
  for (const [re, what] of RULES)
    for (const m of text.matchAll(re)) hits.push([lineOf(m.index), what(m)]);
  hits.sort((a, b) => a[0] - b[0]);
  return hits.map(([n, what]) => `${path}:${n}: ${what} (eval floats: + − × ÷ sqrt only)`);
}

const root = rootArg();
const found = [];
for (const path of walk(root)) {
  if (SCOPE.test(path)) found.push(...violations(path, readFileSync(join(root, path), "utf8")));
}
report(found);
