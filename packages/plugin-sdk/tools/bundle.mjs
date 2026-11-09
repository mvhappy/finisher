// A dependency-free bundler for this repository's own Node plugins (the `ts-tools` example
// and the SDK's test plugins): TypeScript is erased with Node's `module.stripTypeScriptTypes`
// and the module graph is joined into ONE ES module whose only imports are `node:` builtins.
// Plugin authors use esbuild instead (README); this exists so the repository needs no bundler
// dependency (no native binary, no install script) for its examples and tests.
//
// It accepts a deliberately small module subset and refuses anything else with a clear error:
// - static imports from `node:` builtins (named, `* as ns`, default), from relative paths and
//   from `aliases` (`{"@finisher/plugin-sdk": "/abs/src/index.ts"}`); `import type` is erased;
// - `export function|async function|class|const|let|var <name>` (one binding per `export`:
//   `export const a = 1, b = 2` is refused, as the second binding would not be exported),
//   `export { a, b as c }` and `export { a } from "./x.ts"`, `export * from "./x.ts"`;
// - dynamic `import("node:…")`; no other dynamic import, no `export default`, no import cycles.
// Each module runs in its own function scope, in dependency order; exports are copied when the
// module has run (no live bindings: an exported `let` that changes later is not seen).
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { dirname, extname, relative, resolve } from "node:path";

const IMPORT = /^[ \t]*import\s*(?:([\w$]+\s*,?\s*)?(\{[^}]*\}|\*\s*as\s+[\w$]+)?\s*from\s*)?(["'])([^"'\n]+)\3[ \t]*;?/gm;
const EXPORT_DECL = /^export\s+((?:async\s+)?function\*?|class|const|let|var)\s+([\w$]+)/gm;
const EXPORT_DESTRUCTURE = /^export\s+(?:const|let|var)\s*[{[]/m;
const EXPORT_LIST = /^[ \t]*export\s*\{([^}]*)\}\s*(?:from\s*(["'])([^"'\n]+)\2)?[ \t]*;?/gm;
const EXPORT_STAR = /^[ \t]*export\s*\*\s*from\s*(["'])([^"'\n]+)\1[ \t]*;?/gm;
const DYNAMIC_IMPORT = /\bimport\s*\(\s*(?:(["'])([^"'\n]*)\1)?/g;

const OPEN = "([{";
const CLOSE = ")]}";
// After one of these (or at the start) a `/` begins a regular expression, not a division.
const BEFORE_REGEX = "=(,:[!&|?{};+-*%<>~^";
// A line ending at depth 0 after one of these, with the next not starting with one of
// `CONTINUES`, ends the statement (automatic semicolon insertion).
const ENDS_EXPR = /[\w$)\]}"'`]/;
const CONTINUES = ",.?:+-*/%&|^=<>";

/**
 * Whether the declaration whose initialiser starts at `from` in `code` declares a second
 * binding: a `,` at bracket depth 0 before the statement ends. Strings, template literals,
 * regular expressions and comments are skipped (TypeScript types are already erased).
 */
function declaresMore(code, from) {
  let depth = 0;
  let last = "=";
  for (let i = from; i < code.length; i++) {
    const c = code[i];
    if (c === "/" && code[i + 1] === "/") {
      while (i < code.length && code[i] !== "\n") i++;
      i--;
      continue;
    }
    if (c === "/" && code[i + 1] === "*") {
      const end = code.indexOf("*/", i + 2);
      i = end < 0 ? code.length : end + 1;
      continue;
    }
    if (c === '"' || c === "'") {
      for (i++; i < code.length && code[i] !== c && code[i] !== "\n"; i++) if (code[i] === "\\") i++;
      last = c;
      continue;
    }
    if (c === "`") {
      let inner = 0;
      for (i++; i < code.length; i++) {
        if (code[i] === "\\") i++;
        else if (inner === 0 && code[i] === "`") break;
        else if (code[i] === "$" && code[i + 1] === "{") (inner++, i++);
        else if (inner > 0 && code[i] === "{") inner++;
        else if (inner > 0 && code[i] === "}") inner--;
      }
      last = c;
      continue;
    }
    if (c === "/" && BEFORE_REGEX.includes(last)) {
      let cls = false;
      for (i++; i < code.length && code[i] !== "\n"; i++) {
        if (code[i] === "\\") i++;
        else if (code[i] === "[") cls = true;
        else if (code[i] === "]") cls = false;
        else if (code[i] === "/" && !cls) break;
      }
      last = "/x";
      continue;
    }
    if (c === "\n" && depth === 0) {
      const next = code.slice(i + 1).match(/^\s*(\S)/)?.[1];
      if (ENDS_EXPR.test(last.at(-1)) && (next === undefined || !CONTINUES.includes(next))) return false;
      continue;
    }
    if (/\s/.test(c)) continue;
    if (OPEN.includes(c)) depth++;
    else if (CLOSE.includes(c)) {
      if (depth === 0) return false;
      depth--;
    } else if (depth === 0 && c === ";") return false;
    else if (depth === 0 && c === ",") return true;
    last = c;
  }
  return false;
}

/** `a, b as c` → [[local, exported]]; empty entries (erased `type` specifiers) dropped. */
function specifiers(list) {
  return list
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "")
    .map((s) => {
      const [from, as] = s.split(/\s+as\s+/);
      return [from.trim(), (as ?? from).trim()];
    });
}

const silence = process.emitWarning;

function strip(file, source) {
  if (![".ts", ".mts"].includes(extname(file))) return source;
  // The API warns once per process that it is experimental; that is noise here.
  process.emitWarning = (w, ...rest) => (String(w).includes("stripTypeScriptTypes") ? undefined : silence.call(process, w, ...rest));
  try {
    return stripTypeScriptTypes(source, { mode: "strip", sourceUrl: file });
  } finally {
    process.emitWarning = silence;
  }
}

/**
 * Bundles `entry` into `outfile` and returns the output's text. `aliases` maps bare
 * specifiers to absolute files; anything else that is not relative or `node:` is refused.
 * @param {{ entry: string, outfile?: string, aliases?: Record<string, string> }} opts
 * @returns {string}
 */
export function bundle({ entry, outfile, aliases = {} }) {
  const modules = new Map(); // file → {id, code, deps}
  const nodeImports = new Map(); // "node:fs" → "__fin_node_0"
  const order = [];
  const visiting = new Set();

  const nodeNs = (spec) => {
    if (!nodeImports.has(spec)) nodeImports.set(spec, `__fin_node_${nodeImports.size}`);
    return nodeImports.get(spec);
  };

  function resolveSpec(spec, from) {
    if (spec.startsWith("node:")) return { node: spec };
    if (Object.hasOwn(aliases, spec)) return { file: resolve(aliases[spec]) };
    if (spec.startsWith("./") || spec.startsWith("../")) return { file: resolve(dirname(from), spec) };
    throw new Error(`bundle: ${from} imports ${JSON.stringify(spec)}: only node:, relative and aliased imports are bundled`);
  }

  function load(file) {
    if (modules.has(file)) return modules.get(file);
    if (visiting.has(file)) throw new Error(`bundle: an import cycle reaches ${file}`);
    visiting.add(file);
    const code = strip(file, readFileSync(file, "utf8"));
    const where = relative(dirname(entry), file);
    if (/^export\s+default\b/m.test(code)) throw new Error(`bundle: ${where}: export default is not supported`);
    if (EXPORT_DESTRUCTURE.test(code)) throw new Error(`bundle: ${where}: a destructuring export is not supported`);
    for (const m of code.matchAll(DYNAMIC_IMPORT)) {
      if (!m[2]?.startsWith("node:")) throw new Error(`bundle: ${where}: a dynamic import of anything but a node: literal is not supported`);
    }
    const prelude = [];
    const exports = [];
    // Imports: replaced by bindings from the dependency's scope (or the node namespace).
    let body = code.replace(IMPORT, (_all, def, named, _q, spec) => {
      const r = resolveSpec(spec, file);
      const src = r.node !== undefined ? nodeNs(r.node) : load(r.file).id;
      if (def) prelude.push(`const ${def.replace(/[\s,]/g, "")} = ${src}.default;`);
      if (named?.startsWith("*")) prelude.push(`const ${named.replace(/^\*\s*as\s+/, "")} = ${src};`);
      else if (named) {
        const pairs = specifiers(named.slice(1, -1));
        if (pairs.length > 0) prelude.push(`const { ${pairs.map(([a, b]) => (a === b ? a : `${a}: ${b}`)).join(", ")} } = ${src};`);
      }
      return "";
    });
    body = body.replace(EXPORT_STAR, (_all, _q, spec) => {
      const dep = load(resolveSpec(spec, file).file);
      if (dep.id === undefined) throw new Error(`bundle: ${where}: export * from a node: builtin is not supported`);
      for (const name of dep.names) exports.push(`${name}: ${dep.id}.${name}`);
      return "";
    });
    body = body.replace(EXPORT_LIST, (_all, list, _q, spec) => {
      const src = spec === undefined ? undefined : resolveSpec(spec, file);
      const scope = src === undefined ? undefined : src.node !== undefined ? nodeNs(src.node) : load(src.file).id;
      for (const [local, as] of specifiers(list)) exports.push(scope === undefined ? `${as}: ${local}` : `${as}: ${scope}.${local}`);
      return "";
    });
    body = body.replace(EXPORT_DECL, (all, kw, name, at, text) => {
      if (/^(?:const|let|var)$/.test(kw) && declaresMore(text, at + all.length)) {
        throw new Error(`bundle: ${where}: an export declaring more than one binding (${kw} ${name} = …, …) is not supported: export each binding on its own`);
      }
      exports.push(`${name}: ${name}`);
      return `${kw} ${name}`;
    });
    if (/^[ \t]*(?:import|export)\b(?!\s*[(.])/m.test(body.replace(/^[ \t]*(?:\/\/|\*).*$/gm, ""))) {
      throw new Error(`bundle: ${where}: an import or export form this bundler does not know`);
    }
    const id = `__fin_mod_${order.length}`;
    const names = exports.map((e) => e.slice(0, e.indexOf(":")));
    const mod = { id, names, text: `// ${where}\nconst ${id} = (() => {\n${prelude.join("\n")}\n${body}\nreturn { ${exports.join(", ")} };\n})();\n` };
    visiting.delete(file);
    modules.set(file, mod);
    order.push(mod);
    return mod;
  }

  load(resolve(entry));
  const head = [...nodeImports].map(([spec, ns]) => `import * as ${ns} from ${JSON.stringify(spec)};`).join("\n");
  const out = `// Bundled by @finisher/plugin-sdk tools/bundle.mjs: one ES module, node: imports only.\n${head}\n${order.map((m) => m.text).join("\n")}`;
  if (outfile !== undefined) {
    mkdirSync(dirname(outfile), { recursive: true });
    writeFileSync(outfile, out);
  }
  return out;
}
