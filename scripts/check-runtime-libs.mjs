// Run-time library gate (sub-project 6 spec §16.3 item 2). Every library Finisher loads by
// name at run time is listed in `third_party/runtime.json` with who provides it (`os`,
// `driver`, `bundled`) and its licence. Rust and TypeScript sources are searched (comments
// left out) for `Library::new(`/`::open(` (libloading, aliased imports too), `dlopen(`,
// `LoadLibrary…(` and `process.dlopen(`;
// the library argument (the first; `process.dlopen`'s second) must be a string literal naming a listed library (RUNTIME_LIB_UNLISTED).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Graph, documents, metadata } from "./cargo-graph.mjs";
import { report, rootArg, walk } from "./walk.mjs";

// Crates that load libraries at run time: a shipped package depending on one must be listed in
// runtime.json's `crates` (RUNTIME_LOADER_UNLISTED), so a dependency's own loading is reviewed.
export const LOADERS = ["libloading", "dlopen", "dlopen2", "shared_library", "dlib", "libloading-mini"];

const SOURCES = /\.(rs|ts|tsx|mts|cts)$/;
const SKIP = /^(scripts\/|node_modules\/|archive\/)|\/node_modules\//;
/** The call pattern, with the names `libloading::Library` is imported as in this file. */
function calls(text) {
  const names = ["Library"];
  for (const m of text.matchAll(/use\s+libloading(?:::[\w:]*)?::\{?([^;]*)\}?\s*;/g))
    for (const a of m[1].matchAll(/\bLibrary\s+as\s+(\w+)/g)) names.push(a[1]);
  const lib = names.map((n) => n.replace(/[^\w]/g, "")).join("|");
  return new RegExp(`\\b(process\\s*\\.\\s*)?(?:(?:${lib})\\s*::\\s*(?:new|open)|dlopen|LoadLibrary(?:ExW|ExA|W|A|Ex)?)\\s*\\(\\s*`, "g");
}

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

/** Shipped packages (all members' normal and build closure) that depend on a loader crate and
 * are not listed in `crates`. */
export function loaderUsers(doc, crates) {
  const g = new Graph(doc);
  const out = [];
  for (const id of g.closure([...g.members])) {
    const name = g.name(id);
    const uses = g.direct(id, null).filter((n) => LOADERS.includes(n));
    if (uses.length && !crates.includes(name))
      out.push(`RUNTIME_LOADER_UNLISTED: ${name} depends on ${uses.join(", ")}; list it in third_party/runtime.json "crates" with the libraries it loads`);
  }
  return [...new Set(out)];
}

/** Violations in one file's source against the listed names. */
export function violations(path, src, names) {
  const text = blank(src);
  const out = [];
  const blanked = blank(src, true);
  for (const m of blanked.matchAll(calls(blanked))) {
    const line = text.slice(0, m.index).split("\n").length;
    // `process.dlopen(module, filename)` names the library second.
    let rest = text.slice(m.index + m[0].length);
    if (m[1]) rest = rest.replace(/^[\w.]+\s*,\s*/, "");
    const lit = rest.match(/^(?:(?:[a-zA-Z_]\w*!|Some)\s*\(\s*)?(?:b|c|L)?["'`]([^"'`]*)["'`]/);
    if (!lit) out.push(`RUNTIME_LIB_UNLISTED: ${path}:${line}: loads a library whose name is not a string literal; load listed names through a literal`);
    else if (!names.includes(lit[1])) out.push(`RUNTIME_LIB_UNLISTED: ${path}:${line}: loads "${lit[1]}", which third_party/runtime.json does not list`);
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const doc = JSON.parse(readFileSync(join(root, "third_party/runtime.json"), "utf8"));
  const listed = doc.libraries ?? [];
  const names = listed.flatMap((l) => [l.name, ...(l.files ?? [])]);
  const found = [];
  try {
    for (const m of documents(process.argv, () => [metadata(root)])) found.push(...loaderUsers(m, (doc.crates ?? []).map((c) => c.crate)));
  } catch (e) {
    found.push(`RUNTIME_LOADER_UNLISTED: ${e.message}`);
  }
  for (const path of walk(root)) {
    if (!SOURCES.test(path) || SKIP.test(path)) continue;
    found.push(...violations(path, readFileSync(join(root, path), "utf8"), names));
  }
  report(found);
}
