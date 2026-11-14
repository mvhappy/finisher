// Native build gate (sub-project 6 spec §16.3 item 1). `cargo deny` sees a crate's licence,
// not the C library its build script compiles. Keyed on `cargo metadata` over the shipped
// graph (normal and build edges from every workspace member but the dev-only ones, for each
// `deny.toml` target):
// - a package with a `links` key, a build-dependency on a native build tool (cc, cmake,
//   nasm-rs, bindgen, pkg-config, autotools, cxx-build, system-deps, vcpkg), or a build script
//   that runs a compiler or assembler itself (`Command::new("cc")`, …), must be listed in
//   `third_party/native.json` by `package` (NATIVE_UNLISTED), and a listed `version` must match
//   the resolved one (NATIVE_VERSION: `1` matches 1.8.7);
// - a listed licence (and patent licence) must be on the file's allow lists (NATIVE_LICENSE);
// - a listed `path` must be a git submodule at `commit` (NATIVE_SUBMODULE), and each `files`
//   entry (relative to `path`, else to the repository) must exist with the listed BLAKE3
//   (NATIVE_LICENSE_FILE: re-review the licence and update the hash);
// - `THIRD_PARTY_NOTICES.md` must have a section (`##` or `###`) naming the library and its tag or version
//   (NATIVE_NOTICE);
// - every MPL-2.0 package in the graph must be named with its version in the notices' MPL-2.0
//   §3.2 section (MPL_NOTICE).
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3 } from "./blake3.mjs";
import { Graph, denyTargets, documents, metadata, spdxAllowed } from "./cargo-graph.mjs";
import { report, rootArg } from "./walk.mjs";

/** Workspace members that never ship (test fonts, the cross-host harness). */
export const DEV_ONLY = ["fin-testfonts", "fin-store-xhost"];
export const NATIVE_TOOLS = ["cc", "cmake", "nasm-rs", "bindgen", "pkg-config", "autotools", "cxx-build", "system-deps", "vcpkg"];
/** A build script that starts a compiler, assembler or build system by name. */
const RUNS_TOOL = /Command::new\(\s*"(cc|c\+\+|clang|clang\+\+|gcc|g\+\+|cl|cl\.exe|nasm|yasm|as|make|cmake|ninja|meson)"/;

/** The tool a package's build script (`custom-build` target) runs by name, if any. */
function buildScriptTool(p, read) {
  for (const t of p.targets ?? []) {
    if (!(t.kind ?? []).includes("custom-build") || !t.src_path) continue;
    const m = read(t.src_path)?.match(RUNS_TOOL);
    if (m) return m[1];
  }
  return null;
}

/** Packages of the shipped graph: `{name, version, license, native: reason | null}` by id.
 * `read(path)` gives a build script's text (or null). */
export function shipped(docs, read = readOrNull) {
  const out = new Map();
  for (const doc of docs) {
    const g = new Graph(doc);
    const roots = [...g.members].filter((id) => !DEV_ONLY.includes(g.name(id)));
    for (const id of g.closure(roots)) {
      const p = g.packages.get(id);
      if (!p || out.has(id)) continue;
      const tools = g.direct(id, "build").filter((n) => NATIVE_TOOLS.includes(n));
      const runs = buildScriptTool(p, read);
      const native = p.links
        ? `links = "${p.links}"`
        : tools.length
          ? `build-depends on ${tools.join(", ")}`
          : runs
            ? `its build script runs ${runs}`
            : null;
      out.set(id, { name: p.name, version: p.version, license: p.license ?? "", native });
    }
  }
  return [...out.values()];
}

function readOrNull(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

function sections(notices) {
  return notices.split(/^#{2,3} /m).slice(1).map((s) => ({ heading: s.split("\n")[0], text: s }));
}

/**
 * Violations for the shipped packages `pkgs` against `native` (the parsed native.json) and the
 * notices text. `gitlink(path)` answers a submodule's commit (or null), `hash(path)` a file's
 * BLAKE3 (or null when missing); both take repository-relative paths.
 */
export function check({ pkgs, native, notices, gitlink, hash }) {
  const out = [];
  const libs = native.libraries ?? [];
  const allow = native.allow ?? [];
  const patents = native.patentLicences ?? [];
  const secs = sections(notices);
  const reported = new Set();
  for (const p of pkgs) {
    if (!p.native || reported.has(p.name)) continue;
    reported.add(p.name);
    if (!libs.some((l) => l.package === p.name))
      out.push(`NATIVE_UNLISTED: ${p.name} ${p.version} (${p.native}) builds native code; list it in third_party/native.json with its licence, licence-file hashes and notice`);
  }
  for (const l of libs) {
    const who = `${l.name} (${l.package})`;
    const resolved = [...new Set(pkgs.filter((p) => p.name === l.package).map((p) => p.version))];
    if (l.version && resolved.length && !resolved.some((v) => v === l.version || v.startsWith(`${l.version}.`)))
      out.push(`NATIVE_VERSION: ${who}: native.json says ${l.version}, the graph has ${resolved.join(", ")}; re-review and update the entry and its notice`);
    if (!l.license || !spdxAllowed(l.license, allow)) out.push(`NATIVE_LICENSE: ${who} is ${l.license || "unlicensed"}, not on native.json's allow list`);
    if (l.patents && !patents.includes(l.patents)) out.push(`NATIVE_LICENSE: ${who}'s patent licence ${l.patents} is not on native.json's patentLicences`);
    if (l.path) {
      const at = gitlink(l.path);
      if (!at) out.push(`NATIVE_SUBMODULE: ${who}: ${l.path} is not a git submodule`);
      else if (at !== l.commit) out.push(`NATIVE_SUBMODULE: ${who}: ${l.path} is at ${at}, native.json says ${l.commit}`);
    }
    for (const [file, want] of Object.entries(l.files ?? {})) {
      const rel = l.path ? `${l.path}/${file}` : file;
      const got = hash(rel);
      if (got === null) out.push(`NATIVE_LICENSE_FILE: ${who}: ${rel} is missing`);
      else if (got !== want) out.push(`NATIVE_LICENSE_FILE: ${who}: licence file changed: re-review and update the hash (${rel}: ${got})`);
    }
    const word = new RegExp(`(^|[^\\w-])${l.name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}($|[^\\w-])`);
    const sec = secs.find((s) => word.test(s.heading));
    const tag = l.tag ?? l.version;
    if (!sec) out.push(`NATIVE_NOTICE: THIRD_PARTY_NOTICES.md has no section naming ${l.name}`);
    else if (tag && !sec.text.includes(tag)) out.push(`NATIVE_NOTICE: THIRD_PARTY_NOTICES.md's ${l.name} section does not name ${tag}`);
  }
  const mpl = secs.find((s) => /MPL-2\.0 §3\.2/.test(s.heading));
  const seen = new Set();
  for (const p of pkgs) {
    if (!/MPL-2\.0/.test(p.license) || seen.has(`${p.name}@${p.version}`)) continue;
    seen.add(`${p.name}@${p.version}`);
    if (!mpl || !mpl.text.includes(`${p.name} ${p.version}`))
      out.push(`MPL_NOTICE: ${p.name} ${p.version} is MPL-2.0; name it as "${p.name} ${p.version}" in THIRD_PARTY_NOTICES.md's MPL-2.0 §3.2 section`);
  }
  return out;
}

function gitlinkAt(root) {
  return (path) => {
    const r = spawnSync("git", ["ls-files", "-s", "--", path], { cwd: root, encoding: "utf8" });
    const m = r.status === 0 && r.stdout.match(/^160000 ([0-9a-f]{40}) /m);
    return m ? m[1] : null;
  };
}

/** The BLAKE3 of a listed file's text with CRLF line ends read as LF: a Windows checkout
 *  (`core.autocrlf=true`, the Git for Windows default, which a parent `.gitattributes` does not
 *  reach inside a submodule) has the same licence text with CRLF ends. */
export function lfHash(bytes) {
  const out = [];
  for (let i = 0; i < bytes.length; i++) if (!(bytes[i] === 0x0d && bytes[i + 1] === 0x0a)) out.push(bytes[i]);
  return blake3(Buffer.from(out));
}

function hashAt(root) {
  return (path) => {
    const full = join(root, path);
    return existsSync(full) ? lfHash(readFileSync(full)) : null;
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  try {
    const docs = documents(process.argv, () => denyTargets(root).map((t) => metadata(root, t)));
    const native = JSON.parse(readFileSync(join(root, "third_party/native.json"), "utf8"));
    const notices = readFileSync(join(root, "THIRD_PARTY_NOTICES.md"), "utf8");
    report(check({ pkgs: shipped(docs), native, notices, gitlink: gitlinkAt(root), hash: hashAt(root) }));
  } catch (e) {
    report([`NATIVE_GATE: ${e.message}`]);
  }
}
