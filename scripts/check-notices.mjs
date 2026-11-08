// Notices gate (8A Task 14 follow-up). `cargo deny` checks that a crate's licence is allowed and
// `check-native-deps` that native code and MPL-2.0 crates are named, but neither asks whether a
// pure-Rust MIT/Apache crate that ships appears in `THIRD_PARTY_NOTICES.md`. This gate does, over
// the shipped graph of `check-native-deps` (every workspace member but the dev-only ones, normal
// and build edges, each `deny.toml` target):
// - every external package is named in backticks, `name`, somewhere in the notices (NOTICE_CRATE);
// - every SPDX licence id the shipped packages use appears in the notices (NOTICE_LICENSE).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { DEV_ONLY } from "./check-native-deps.mjs";
import { Graph, denyTargets, documents, metadata } from "./cargo-graph.mjs";
import { report, rootArg } from "./walk.mjs";

/** External packages of the shipped graph: `[{name, version, license}]`, one per name and version. */
export function externalShipped(docs) {
  const out = new Map();
  for (const doc of docs) {
    const g = new Graph(doc);
    const roots = [...g.members].filter((id) => !DEV_ONLY.includes(g.name(id)));
    for (const id of g.closure(roots)) {
      const p = g.packages.get(id);
      if (!p || g.members.has(id)) continue;
      out.set(`${p.name}@${p.version}`, { name: p.name, version: p.version, license: p.license ?? "" });
    }
  }
  return [...out.values()];
}

/** The SPDX ids of an expression (a `WITH` exception is dropped: its licence's id counts). */
export function spdxIds(expr) {
  return expr
    .replace(/[()]/g, " ")
    .split(/\s+(?:OR|AND)\s+|\s*\/\s*/)
    .map((s) => s.trim().split(/\s+/)[0])
    .filter(Boolean);
}

/** Violations for the shipped `pkgs` against the notices text. */
export function check({ pkgs, notices }) {
  const out = [];
  const named = new Set([...notices.matchAll(/`([^`\s]+)`/g)].map((m) => m[1]));
  const seen = new Set();
  for (const p of pkgs) {
    if (named.has(p.name) || seen.has(p.name)) continue;
    seen.add(p.name);
    out.push(`NOTICE_CRATE: ${p.name} ${p.version} (${p.license || "no licence"}) ships but \`${p.name}\` is not in THIRD_PARTY_NOTICES.md; add it to the Cargo dependencies section`);
  }
  const ids = new Set(pkgs.flatMap((p) => spdxIds(p.license)));
  for (const id of [...ids].sort()) {
    if (!notices.includes(id)) out.push(`NOTICE_LICENSE: shipped crates use ${id} but THIRD_PARTY_NOTICES.md never names it`);
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  try {
    const docs = documents(process.argv, () => denyTargets(root).map((t) => metadata(root, t)));
    const notices = readFileSync(join(root, "THIRD_PARTY_NOTICES.md"), "utf8");
    report(check({ pkgs: externalShipped(docs), notices }));
  } catch (e) {
    report([`NOTICE_GATE: ${e.message}`]);
  }
}
