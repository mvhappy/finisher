// Workers never start processes (5C-1 plan Decision 24, spec §6.2): no process-creating API in
// `crates/fin-workers/src/` or `crates/fin-jobs/src/`, outside `test_family/` (the `test`
// family's `test.spawn` breaks the rule on purpose) and `watchdog.rs` (not a worker). Comments
// and string literals are ignored; `.clone()` and `fn clone` are not `clone(2)`. Each finding
// is `WORKER_SPAWN <path>:<line>: <api>`; Decision 22 holds the rule at run time.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg, walk } from "./walk.mjs";

export const ROOTS = ["crates/fin-workers/src/", "crates/fin-jobs/src/"];

/** The forbidden APIs: a name for the report and its pattern (over code without comments or strings). */
export const FORBIDDEN = [
  ["std::process::Command", /\bprocess::Command\b|\bCommand::new\b|\bprocess::\{[^}]*\bCommand\b/],
  ["fork", /(?<![\w.])(?:libc::|nix::unistd::)?fork\s*\(/],
  ["vfork", /(?<![\w.])(?:libc::)?vfork\s*\(/],
  ["posix_spawn", /\bposix_spawn\w*/],
  ["exec", /(?<![\w])(?:libc::|nix::unistd::)?exec(?:v|ve|vp|vpe|l|le|lp|veat|vP)\s*\(|\.exec\s*\(|\bfexecve\b/],
  ["CreateProcess", /\bCreateProcess\w*/],
  ["ShellExecute", /\bShellExecute\w*/],
  ["clone", /\b(?:libc|nix::sched|sched)::clone3?\b|\bSYS_clone3?\b|(?<![\w.:])(?<!fn )clone3?\s*\(/],
];

/** Whether `path` (repo-relative, "/") is checked. */
export function checked(path) {
  if (!path.endsWith(".rs") || !ROOTS.some((r) => path.startsWith(r))) return false;
  return !path.includes("/test_family/") && !path.endsWith("/watchdog.rs");
}

/** `text` with comments and string and char literals blanked (newlines kept, so lines still count). */
export function code(text) {
  let out = "";
  let i = 0;
  const blank = (s) => s.replace(/[^\n]/g, " ");
  while (i < text.length) {
    const c = text[i];
    const n = text[i + 1];
    if (c === "/" && n === "/") {
      const end = text.indexOf("\n", i);
      const j = end === -1 ? text.length : end;
      out += blank(text.slice(i, j));
      i = j;
    } else if (c === "/" && n === "*") {
      // Rust block comments nest.
      let depth = 1;
      let j = i + 2;
      while (j < text.length && depth > 0) {
        if (text[j] === "/" && text[j + 1] === "*") (depth++, (j += 2));
        else if (text[j] === "*" && text[j + 1] === "/") (depth--, (j += 2));
        else j++;
      }
      out += blank(text.slice(i, j));
      i = j;
    } else if (c === "r" && /^r#*"/.test(text.slice(i, i + 8)) && !/\w/.test(text[i - 1] ?? "")) {
      const hashes = /^r(#*)"/.exec(text.slice(i))[1];
      const close = `"${hashes}`;
      const end = text.indexOf(close, i + 2 + hashes.length);
      const j = end === -1 ? text.length : end + close.length;
      out += blank(text.slice(i, j));
      i = j;
    } else if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += blank(text.slice(i, j + 1));
      i = j + 1;
    } else if (c === "'" && /^'(?:\\.|[^\\'])'/.test(text.slice(i, i + 4))) {
      // A char literal (a lifetime has no closing quote).
      const j = text.indexOf("'", i + 2) + 1;
      out += blank(text.slice(i, j));
      i = j;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** The findings in one file's text. */
export function findings(path, text) {
  const out = [];
  code(text)
    .split("\n")
    .forEach((line, n) => {
      for (const [name, re] of FORBIDDEN) if (re.test(line)) out.push(`WORKER_SPAWN ${path}:${n + 1}: ${name}`);
    });
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  const violations = [];
  for (const path of walk(root)) if (checked(path)) violations.push(...findings(path, readFileSync(join(root, path), "utf8")));
  report(violations);
}
