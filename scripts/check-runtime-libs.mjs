// Run-time library gate (sub-project 6 spec §16.3 item 2). Every library Finisher loads by
// name at run time is listed in `third_party/runtime.json` with who provides it (`os`,
// `driver`, `bundled`) and its licence. Rust and TypeScript sources are searched (comments
// left out) for `Library::new(` (libloading), `dlopen(`, `LoadLibrary…(` and `process.dlopen(`;
// the library argument (the first; `process.dlopen`'s second) must be a string literal naming a listed library (RUNTIME_LIB_UNLISTED).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg, walk } from "./walk.mjs";

const SOURCES = /\.(rs|ts|tsx|mts|cts)$/;
const SKIP = /^(scripts\/|node_modules\/|archive\/)|\/node_modules\//;
const CALL = /\b(process\s*\.\s*)?(?:Library::new|dlopen|LoadLibrary(?:ExW|ExA|W|A|Ex)?)\s*\(\s*/g;

/** The source with comments (and, with `strings`, string contents) replaced by spaces; same
 * length and newlines, so an offset in one is the same place in the other. */
export function blank(src, strings = false) {
  const sp = (t) => t.replace(/[^\n]/g, " ");
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "/") {
      const j = src.indexOf("\n", i);
      const stop = j === -1 ? src.length : j;
      out += sp(src.slice(i, stop));
      i = stop;
    } else if (c === "/" && src[i + 1] === "*") {
      const j = src.indexOf("*/", i + 2);
      const stop = j === -1 ? src.length : j + 2;
      out += sp(src.slice(i, stop));
      i = stop;
    } else if (c === '"' || c === "`" || (c === "'" && /^'(?:\\.|[^\\'\n])*'/.test(src.slice(i, i + 200)))) {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === "\\" ? 2 : 1;
      const body = src.slice(i + 1, j);
      out += c + (strings ? sp(body) : body) + (src[j] ?? "");
      i = j + 1;
    } else out += src[i++];
  }
  return out;
}

/** Violations in one file's source against the listed names. */
export function violations(path, src, names) {
  const text = blank(src);
  const out = [];
  for (const m of blank(src, true).matchAll(CALL)) {
    const line = text.slice(0, m.index).split("\n").length;
    // `process.dlopen(module, filename)` names the library second.
    let rest = text.slice(m.index + m[0].length);
    if (m[1]) rest = rest.replace(/^[\w.]+\s*,\s*/, "");
    const lit = rest.match(/^(?:[a-zA-Z_]\w*!\s*\(\s*)?(?:b|c|L)?["'`]([^"'`]*)["'`]/);
    if (!lit) out.push(`RUNTIME_LIB_UNLISTED: ${path}:${line}: loads a library whose name is not a string literal; load listed names through a literal`);
    else if (!names.includes(lit[1])) out.push(`RUNTIME_LIB_UNLISTED: ${path}:${line}: loads "${lit[1]}", which third_party/runtime.json does not list`);
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const listed = JSON.parse(readFileSync(join(root, "third_party/runtime.json"), "utf8")).libraries ?? [];
  const names = listed.flatMap((l) => [l.name, ...(l.files ?? [])]);
  const found = [];
  for (const path of walk(root)) {
    if (!SOURCES.test(path) || SKIP.test(path)) continue;
    found.push(...violations(path, readFileSync(join(root, path), "utf8"), names));
  }
  report(found);
}
