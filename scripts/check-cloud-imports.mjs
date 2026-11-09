// No cloud-only imports (AGENTS.md): nothing in the repository may import, require, re-export,
// depend on or link a `@finisher-cloud/*` npm package or a `finisher-cloud*` crate.
//
// The gate reads what the code means, not what a line looks like:
// - TS/JS: a token scan (comments dropped, strings, templates and regexes skipped over) finds
//   static `import`/`export … from` (also over several lines), side-effect `import "x"`,
//   dynamic `import("x")`, `require("x")` and `require.resolve("x")`.
// - Rust: identifiers `finisher_cloud*` in code (comments and strings blanked): `use`, nested
//   `use {..}`, `extern crate … as`, inline paths.
// - Cargo: the dependency key, a `package = "finisher-cloud…"` rename (any spelling of the
//   table), `[dependencies.finisher-cloud…]` tables, and, in `Cargo.lock`, a resolved package
//   by that name.
// - package.json: a dependency key or `npm:@finisher-cloud/…` alias, also in `overrides`,
//   `resolutions` and `pnpm.overrides`; the pnpm, npm and yarn lock files and pnpm-workspace.yaml.
//
// A dynamic import whose specifier is computed at run time cannot be checked here (a template
// literal that starts with the scope can: `import(`@finisher-cloud/${x}`)`).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { code as rustCode } from "./check-worker-spawn.mjs";
import { report, rootArg, walk } from "./walk.mjs";

const NPM_SCOPE = /^@finisher-cloud(?:\/|$)/;
const KEYWORDS_BEFORE_REGEX = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "yield", "await"]);

/** TS/JS tokens of `text`: `{ t: "id" | "str" | "tpl" | "p", v }`. Comments are dropped; a
 * template without substitutions is a `str`. Regex literals are skipped where a regex can start. */
export function tokens(text) {
  const out = [];
  const stack = []; // "tpl" when a `${` is open, "blk" for a plain `{`
  let i = 0;
  const last = () => out[out.length - 1];
  // The character an escape after a backslash stands for (`i` at the backslash on entry,
  // at the escape's last character on exit); `\x2f` and `\u002f` spell "/".
  const escape = () => {
    const next = text[i + 1] ?? "";
    const hex = (re) => {
      const m = re.exec(text.slice(i + 1));
      if (!m) return null;
      i += m[0].length;
      return String.fromCodePoint(Number.parseInt(m[1], 16));
    };
    if (next === "x") return hex(/^x([0-9a-fA-F]{2})/) ?? "x";
    if (next === "u") return hex(/^u\{([0-9a-fA-F]{1,6})\}/) ?? hex(/^u([0-9a-fA-F]{4})/) ?? "u";
    i++;
    return { n: "\n", t: "\t", r: "\r" }[next] ?? next;
  };
  const template = (resumed) => {
    // `i` is just past the opening backtick or the `}` that closes a substitution.
    let v = "";
    for (; i < text.length; i++) {
      const c = text[i];
      if (c === "\\") v += escape();
      else if (c === "`") {
        i++;
        return out.push(resumed ? { t: "tpl", v: "" } : { t: "str", v });
      } else if (c === "$" && text[i + 1] === "{") {
        i += 2;
        if (!resumed) out.push({ t: "tplhead", v });
        stack.push("tpl");
        return;
      } else v += c;
    }
  };
  while (i < text.length) {
    const c = text[i];
    const n = text[i + 1];
    if (/\s/.test(c)) i++;
    else if (c === "/" && n === "/") {
      const end = text.indexOf("\n", i);
      i = end === -1 ? text.length : end;
    } else if (c === "/" && n === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end === -1 ? text.length : end + 2;
    } else if (c === '"' || c === "'") {
      let v = "";
      i++;
      while (i < text.length && text[i] !== c && text[i] !== "\n") {
        if (text[i] === "\\") v += escape();
        else v += text[i];
        i++;
      }
      i++;
      out.push({ t: "str", v });
    } else if (c === "`") {
      i++;
      template(false);
    } else if (c === "{") {
      stack.push("blk");
      out.push({ t: "p", v: c });
      i++;
    } else if (c === "}") {
      i++;
      if (stack.pop() === "tpl") template(true);
      else out.push({ t: "p", v: c });
    } else if (/[A-Za-z_$]/.test(c)) {
      let j = i + 1;
      while (j < text.length && /[\w$]/.test(text[j])) j++;
      out.push({ t: "id", v: text.slice(i, j) });
      i = j;
    } else if (/[0-9]/.test(c)) {
      let j = i + 1;
      while (j < text.length && /[\w.]/.test(text[j])) j++;
      out.push({ t: "num", v: text.slice(i, j) });
      i = j;
    } else if (c === "/" && regexAllowed(last())) {
      i++;
      let inClass = false;
      while (i < text.length && text[i] !== "\n" && (inClass || text[i] !== "/")) {
        if (text[i] === "\\") i++;
        else if (text[i] === "[") inClass = true;
        else if (text[i] === "]") inClass = false;
        i++;
      }
      i++;
      while (i < text.length && /[a-z]/.test(text[i])) i++;
      out.push({ t: "num", v: "/re/" });
    } else if ((c === "+" || c === "-") && n === c) {
      // `i++ / 2` divides: a postfix operator ends an expression.
      out.push({ t: "p", v: c + c });
      i += 2;
    } else {
      out.push({ t: "p", v: c });
      i++;
    }
  }
  return out;
}

function regexAllowed(prev) {
  if (!prev) return true;
  if (prev.t === "id") return KEYWORDS_BEFORE_REGEX.has(prev.v);
  if (prev.t === "p") return ![")", "]", "}", "++", "--"].includes(prev.v);
  return false;
}

/** The cloud package specifiers a TS/JS file imports, requires or re-exports. */
export function tsSpecifiers(text) {
  const t = tokens(text);
  const found = [];
  const at = (k, type, v) => t[k]?.t === type && (v === undefined || t[k].v === v);
  // A specifier: a string, or a template whose literal head names the scope (`@finisher-cloud/${x}`).
  const spec_ = (k) => (at(k, "str") || at(k, "tplhead") ? t[k].v : undefined);
  for (let k = 0; k < t.length; k++) {
    if (t[k].t !== "id") continue;
    // `a.import(..)`, `a.from(..)` and `a.require(..)` are other APIs; `module.require(..)` loads.
    if (at(k - 1, "p", ".") && !(t[k].v === "require" && at(k - 2, "id", "module"))) continue;
    let spec;
    switch (t[k].v) {
      case "from":
        spec = spec_(k + 1);
        break;
      case "import":
        if (at(k + 1, "str")) spec = t[k + 1].v;
        else if (at(k + 1, "p", "(")) spec = spec_(k + 2);
        else if (at(k + 1, "p", ".") && at(k + 2, "id", "meta") && at(k + 3, "p", ".") && at(k + 4, "id", "resolve") && at(k + 5, "p", "(")) spec = spec_(k + 6);
        break;
      case "require":
        if (at(k + 1, "p", "(")) spec = spec_(k + 2);
        else if (at(k + 1, "p", ".") && at(k + 2, "id", "resolve") && at(k + 3, "p", "(")) spec = spec_(k + 4);
        break;
    }
    if (spec !== undefined && NPM_SCOPE.test(spec)) found.push(spec);
  }
  return found;
}

/** Cloud crates named in Rust code. */
export function rustNames(text) {
  const names = new Set();
  for (const m of rustCode(text).matchAll(/\bfinisher_cloud\w*/g)) names.add(m[0]);
  return [...names];
}

/** The key segments and `package` values of a TOML document: every bare or quoted segment of
 * a table header or key (`[dependencies.a]`, `a.package = ..`, `"a" = ..`, inline-table keys),
 * and the string given to a key called `package`. Values are read, never reported, so a
 * keyword or description that mentions a crate is not a dependency on it. */
export function tomlKeys(text) {
  const keys = [];
  const packages = [];
  let i = 0;
  const ws = (newlines) => {
    for (;;) {
      const c = text[i];
      if (c === "#") while (i < text.length && text[i] !== "\n") i++;
      else if (c === " " || c === "\t" || c === "\r" || (newlines && c === "\n")) i++;
      else return;
    }
  };
  const str = () => {
    const q = text[i];
    const long = text.startsWith(q.repeat(3), i);
    i += long ? 3 : 1;
    let v = "";
    while (i < text.length) {
      if (long ? text.startsWith(q.repeat(3), i) : text[i] === q || text[i] === "\n") break;
      if (q === '"' && text[i] === "\\") i++;
      v += text[i++] ?? "";
    }
    i += long ? 3 : 1;
    return v;
  };
  const key = () => {
    const parts = [];
    for (;;) {
      ws(false);
      const c = text[i];
      if (c === '"' || c === "'") parts.push(str());
      else {
        const m = /^[A-Za-z0-9_-]+/.exec(text.slice(i, i + 256));
        if (!m) break;
        parts.push(m[0]);
        i += m[0].length;
      }
      ws(false);
      if (text[i] !== ".") break;
      i++;
    }
    keys.push(...parts);
    return parts;
  };
  const value = (path) => {
    ws(true);
    const c = text[i];
    if (c === '"' || c === "'") {
      const v = str();
      if (path.at(-1) === "package") packages.push(v);
    } else if (c === "[") {
      i++;
      for (ws(true); i < text.length && text[i] !== "]"; ws(true)) {
        const before = i;
        value([]);
        ws(true);
        if (text[i] === ",") i++;
        if (i === before) i++;
      }
      i++;
    } else if (c === "{") {
      i++;
      for (ws(true); i < text.length && text[i] !== "}"; ws(true)) {
        const before = i;
        const p = key();
        ws(false);
        if (text[i] === "=") (i++, value(p));
        ws(true);
        if (text[i] === ",") i++;
        if (i === before) i++;
      }
      i++;
    } else {
      while (i < text.length && !",]}\n".includes(text[i]) && text[i] !== "#") i++;
    }
  };
  while (i < text.length) {
    ws(true);
    if (text[i] === "[") {
      i += text[i + 1] === "[" ? 2 : 1;
      key();
      while (i < text.length && text[i] !== "\n") i++;
    } else if (i < text.length) {
      const before = i;
      const p = key();
      ws(false);
      if (text[i] === "=") (i++, value(p));
      while (i < text.length && text[i] !== "\n") i++;
      if (i === before) i++;
    }
  }
  return { keys, packages };
}

/** Cloud crates a `Cargo.toml` depends on: by key (`name = …`, `name.workspace = true`,
 * `[dependencies.name]`, `dependencies.name = …`) or by `package = "name"` under any alias. */
export function cargoNames(text) {
  const { keys, packages } = tomlKeys(text);
  const cloud = /^finisher[-_]cloud[\w-]*$/;
  return [...new Set([...keys, ...packages].filter((n) => cloud.test(n)))];
}

/** Cloud packages a `Cargo.lock` resolved. */
export function lockNames(text) {
  return [...new Set([...text.matchAll(/^name\s*=\s*"(finisher[-_]cloud[\w-]*)"/gm)].map((m) => m[1]))];
}

const DEP_FIELDS = new Set([
  "dependencies", "devDependencies", "peerDependencies", "optionalDependencies", "bundledDependencies",
  "bundleDependencies", "overrides", "resolutions", "catalog", "catalogs", "patchedDependencies",
]);

/** Cloud packages a `package.json` depends on, by key or `npm:` alias, in any dependency field
 * (also `overrides`, `resolutions`, `pnpm.overrides`, catalogs). */
export function packageJsonNames(text) {
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    return [];
  }
  const found = new Set();
  const walk = (node, inDeps) => {
    if (typeof node === "string") {
      const alias = inDeps ? node.match(/@finisher-cloud\/[^@\s]+/) : null;
      if (alias) found.add(alias[0]);
    } else if (node && typeof node === "object") {
      for (const [k, v] of Object.entries(node)) {
        if (inDeps && NPM_SCOPE.test(k)) found.add(k);
        walk(v, inDeps || DEP_FIELDS.has(k));
      }
    }
  };
  walk(json, false);
  return [...found];
}

/** Cloud packages a lock or workspace file (`pnpm-lock.yaml`, `pnpm-workspace.yaml`,
 * `package-lock.json`, `yarn.lock`) names, outside `#` comments. */
export function lockfileNames(text) {
  const found = new Set();
  for (const line of text.split("\n")) {
    if (line.trimStart().startsWith("#")) continue;
    for (const m of line.matchAll(/@finisher-cloud\/[\w.-]+/g)) found.add(m[0]);
  }
  return [...found];
}

const JS_LOCKS = ["pnpm-lock.yaml", "pnpm-workspace.yaml", "package-lock.json", "yarn.lock"];

/** Whether `path` (repo-relative, "/") is a file kind the gate reads. */
export function relevant(path) {
  const base = path.split("/").pop();
  return /\.(ts|tsx|mts|cts|js|jsx|mjs|cjs|rs)$/.test(path) || ["Cargo.toml", "Cargo.lock", "package.json", ...JS_LOCKS].includes(base);
}

/** The violations in one file (`path` repo-relative with "/"). */
export function check(path, text) {
  const base = path.split("/").pop();
  if (/\.(ts|tsx|mts|cts|js|jsx|mjs|cjs)$/.test(path)) return tsSpecifiers(text).map((s) => `${path}: imports ${s}`);
  if (path.endsWith(".rs")) return rustNames(text).map((n) => `${path}: uses ${n}`);
  if (base === "Cargo.toml") return cargoNames(text).map((n) => `${path}: depends on ${n}`);
  if (base === "Cargo.lock") return lockNames(text).map((n) => `${path}: resolves ${n}`);
  if (base === "package.json") return packageJsonNames(text).map((n) => `${path}: depends on ${n}`);
  if (JS_LOCKS.includes(base)) return lockfileNames(text).map((n) => `${path}: resolves ${n}`);
  return [];
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const violations = [];
  for (const path of walk(root)) {
    if (!relevant(path)) continue;
    let text;
    try {
      text = readFileSync(join(root, path), "utf8");
    } catch (e) {
      // A tracked symlink to a directory, or a file removed since `git ls-files`.
      if (e.code === "EISDIR" || e.code === "ENOENT") continue;
      throw e;
    }
    violations.push(...check(path, text));
  }
  report(violations);
}
