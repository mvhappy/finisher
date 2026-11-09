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
// - package.json: a dependency key or `npm:@finisher-cloud/…` alias.
//
// A dynamic import whose specifier is computed at run time cannot be checked here.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { code as rustCode } from "./check-worker-spawn.mjs";
import { report, rootArg, walk } from "./walk.mjs";

const NPM_SCOPE = /^@finisher-cloud(?:\/|$)/;
const CRATE = /finisher[-_]cloud[\w-]*/;
const KEYWORDS_BEFORE_REGEX = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "yield", "await"]);

/** TS/JS tokens of `text`: `{ t: "id" | "str" | "tpl" | "p", v }`. Comments are dropped; a
 * template without substitutions is a `str`. Regex literals are skipped where a regex can start. */
export function tokens(text) {
  const out = [];
  const stack = []; // "tpl" when a `${` is open, "blk" for a plain `{`
  let i = 0;
  const last = () => out[out.length - 1];
  const template = (resumed) => {
    // `i` is just past the opening backtick or the `}` that closes a substitution.
    let v = "";
    for (; i < text.length; i++) {
      const c = text[i];
      if (c === "\\") v += text[++i] ?? "";
      else if (c === "`") {
        i++;
        return out.push(resumed ? { t: "tpl", v: "" } : { t: "str", v });
      } else if (c === "$" && text[i + 1] === "{") {
        i += 2;
        out.push({ t: "tplpart", v });
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
        if (text[i] === "\\") i++;
        v += text[i] ?? "";
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
    } else {
      out.push({ t: "p", v: c });
      i++;
    }
  }
  return out.filter((t) => t.t !== "tplpart");
}

function regexAllowed(prev) {
  if (!prev) return true;
  if (prev.t === "id") return KEYWORDS_BEFORE_REGEX.has(prev.v);
  if (prev.t === "p") return ![")", "]", "}"].includes(prev.v);
  return false;
}

/** The cloud package specifiers a TS/JS file imports, requires or re-exports. */
export function tsSpecifiers(text) {
  const t = tokens(text);
  const found = [];
  const at = (k, type, v) => t[k]?.t === type && (v === undefined || t[k].v === v);
  for (let k = 0; k < t.length; k++) {
    if (t[k].t !== "id" || at(k - 1, "p", ".")) continue;
    let spec;
    switch (t[k].v) {
      case "from":
        if (at(k + 1, "str")) spec = t[k + 1].v;
        break;
      case "import":
        if (at(k + 1, "str")) spec = t[k + 1].v;
        else if (at(k + 1, "p", "(") && at(k + 2, "str")) spec = t[k + 2].v;
        break;
      case "require":
        if (at(k + 1, "p", "(") && at(k + 2, "str")) spec = t[k + 2].v;
        else if (at(k + 1, "p", ".") && at(k + 2, "id", "resolve") && at(k + 3, "p", "(") && at(k + 4, "str")) spec = t[k + 4].v;
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

/** `text` of a TOML file without `#` comments (a `#` inside a string stays). */
function tomlCode(text) {
  return text
    .split("\n")
    .map((line) => {
      let quote = null;
      for (let i = 0; i < line.length; i++) {
        const c = line[i];
        if (quote) {
          if (c === "\\" && quote === '"') i++;
          else if (c === quote) quote = null;
        } else if (c === '"' || c === "'") quote = c;
        else if (c === "#") return line.slice(0, i);
      }
      return line;
    })
    .join("\n");
}

/** Cloud crates a `Cargo.toml` depends on: by key (`name = …`, `name.workspace = true`,
 * `[dependencies.name]`) or by `package = "name"` under any alias. */
export function cargoNames(text) {
  const toml = tomlCode(text);
  const found = new Set();
  for (const m of toml.matchAll(/(?:^|[\s,{])"?('?)(finisher[-_]cloud[\w-]*)\1"?\s*(?:=|\.\w+\s*=|\])/gm)) found.add(m[2]);
  for (const m of toml.matchAll(/\bpackage\s*=\s*["'](finisher[-_]cloud[\w-]*)["']/g)) found.add(m[1]);
  for (const m of toml.matchAll(/^\s*\[[^\]\n]*\.(?:"|')?(finisher[-_]cloud[\w-]*)(?:"|')?\s*\]/gm)) found.add(m[1]);
  return [...found];
}

/** Cloud packages a `Cargo.lock` resolved. */
export function lockNames(text) {
  return [...new Set([...text.matchAll(/^name\s*=\s*"(finisher[-_]cloud[\w-]*)"/gm)].map((m) => m[1]))];
}

/** Cloud packages a `package.json` depends on, by key or `npm:` alias. */
export function packageJsonNames(text) {
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    return [];
  }
  const found = new Set();
  for (const field of ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]) {
    for (const [name, spec] of Object.entries(json?.[field] ?? {})) {
      if (NPM_SCOPE.test(name)) found.add(name);
      const alias = typeof spec === "string" ? spec.match(/@finisher-cloud\/[^@\s]+/) : null;
      if (alias) found.add(alias[0]);
    }
  }
  return [...found];
}

/** Whether `path` (repo-relative, "/") is a file kind the gate reads. */
export function relevant(path) {
  const base = path.split("/").pop();
  return /\.(ts|tsx|mts|cts|js|jsx|mjs|cjs|rs)$/.test(path) || ["Cargo.toml", "Cargo.lock", "package.json"].includes(base);
}

/** The violations in one file (`path` repo-relative with "/"). */
export function check(path, text) {
  const base = path.split("/").pop();
  if (/\.(ts|tsx|mts|cts|js|jsx|mjs|cjs)$/.test(path)) return tsSpecifiers(text).map((s) => `${path}: imports ${s}`);
  if (path.endsWith(".rs")) return rustNames(text).map((n) => `${path}: uses ${n}`);
  if (base === "Cargo.toml") return cargoNames(text).map((n) => `${path}: depends on ${n}`);
  if (base === "Cargo.lock") return lockNames(text).map((n) => `${path}: resolves ${n}`);
  if (base === "package.json") return packageJsonNames(text).map((n) => `${path}: depends on ${n}`);
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
