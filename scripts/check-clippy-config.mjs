// The library-reader gate (plan 5D Decision 21): clippy's `disallowed-methods` keeps every
// path-based read out of the crates that handle library paths, and this script keeps the gate
// itself honest:
// - each listed crate that exists has a `clippy.toml` naming every method in DISALLOWED;
// - every `allow`/`expect` of `clippy::disallowed_methods` in any crate sits in a file that
//   `scripts/clippy-allow.json` lists (an exact file, or a directory) with a reason;
// - every entry of the list still covers such a site.
// Each breach is CLIPPY_CONFIG, CLIPPY_ALLOW_UNLISTED or CLIPPY_ALLOW_STALE. The lint itself
// runs in `pnpm gates` (`cargo clippy -D clippy::disallowed_methods` over `CRATES`).
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg, walk } from "./walk.mjs";

/** The crates that handle library paths (each is checked once it exists). */
export const CRATES = ["fin-library", "fin-workers", "fin-node", "fin-media", "fin-analyze"];

/** The methods no library reader may call (Decision 21). */
export const DISALLOWED = [
  "std::fs::File::open",
  "std::fs::OpenOptions::open",
  "std::fs::metadata",
  "std::fs::symlink_metadata",
  "std::fs::canonicalize",
  "std::fs::read",
  "std::fs::read_to_string",
  "std::fs::read_dir",
  "std::fs::read_link",
  "std::path::Path::exists",
  "std::path::Path::try_exists",
  "std::path::Path::is_file",
  "std::path::Path::is_dir",
  "std::path::Path::is_symlink",
  "std::path::Path::metadata",
  "std::path::Path::symlink_metadata",
  "std::path::Path::canonicalize",
  "std::path::Path::read_dir",
  "std::path::Path::read_link",
  "dunce::canonicalize",
];

/** The crates that exist under `root`. */
export function listedCrates(root) {
  return CRATES.filter((c) => existsSync(join(root, "crates", c, "Cargo.toml")));
}

const ALLOW = /(?:allow|expect)\s*\(([^)]*)\)/g;

/** Whether `source` allows the lint (comments do not count). */
export function allowsLint(source) {
  const code = source.replace(/\/\/[^\n]*/g, "");
  for (const m of code.matchAll(ALLOW)) if (m[1].includes("clippy::disallowed_methods")) return true;
  return false;
}

export function check(root) {
  const out = [];
  for (const c of listedCrates(root)) {
    const file = join(root, "crates", c, "clippy.toml");
    if (!existsSync(file)) {
      out.push(`CLIPPY_CONFIG: crates/${c} has no clippy.toml (disallowed-methods, Decision 21)`);
      continue;
    }
    const text = readFileSync(file, "utf8");
    for (const p of DISALLOWED) {
      if (!text.includes(`path = "${p}"`)) out.push(`CLIPPY_CONFIG: crates/${c}/clippy.toml does not disallow ${p}`);
    }
  }
  const listFile = join(root, "scripts", "clippy-allow.json");
  const entries = existsSync(listFile) ? JSON.parse(readFileSync(listFile, "utf8")) : [];
  for (const e of entries) {
    if (typeof e.path !== "string" || typeof e.reason !== "string" || !e.reason.trim())
      out.push(`CLIPPY_CONFIG: scripts/clippy-allow.json entry without a path and a reason: ${JSON.stringify(e)}`);
  }
  const covers = (entry, file) => file === entry.path || file.startsWith(`${entry.path.replace(/\/$/, "")}/`);
  const used = new Set();
  for (const file of walk(root)) {
    if (!file.endsWith(".rs") || !file.startsWith("crates/")) continue;
    if (!allowsLint(readFileSync(join(root, file), "utf8"))) continue;
    const hit = entries.filter((e) => typeof e.path === "string" && covers(e, file));
    if (hit.length === 0) out.push(`CLIPPY_ALLOW_UNLISTED: ${file} allows clippy::disallowed_methods; list it in scripts/clippy-allow.json with a reason`);
    for (const e of hit) used.add(e);
  }
  for (const e of entries) {
    if (typeof e.path === "string" && !used.has(e)) out.push(`CLIPPY_ALLOW_STALE: scripts/clippy-allow.json lists ${e.path}, which allows nothing`);
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report(check(rootArg()));
