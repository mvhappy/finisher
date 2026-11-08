import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { metadata, spdxAllowed } from "./cargo-graph.mjs";
import { rootArg } from "./walk.mjs";

// `deny.toml` `[graph] targets` lists only the shipped OS triples, so cargo-deny never sees a
// dependency that only wasm32 pulls in. fin-core, fin-text, fin-plugin-api, fin-media,
// fin-container and fin-compositor also build for `wasm32-unknown-unknown`: a package in their
// wasm32 dependency tree that none of the shipped triples' trees holds (wgpu's WebGPU backend:
// wasm-bindgen, js-sys, web-sys, …) must carry a licence on deny.toml's `[licenses] allow`
// list, checked here since cargo-deny cannot (WASM_ONLY_DEPENDENCY). MPL-2.0 does not count
// for these: the MPL notice gate (check-native-deps) only walks the shipped triples.

export const CRATES = ["fin-core", "fin-text", "fin-plugin-api", "fin-media", "fin-container", "fin-compositor"];
export const WASM = "wasm32-unknown-unknown";
export const NATIVE = ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"];

/** Package ids (`name vX.Y.Z`) in `cargo tree --prefix none -f "{p}"` output. */
export function packages(text) {
  return new Set(
    text
      .split("\n")
      .map((l) => l.replace(/\s*\(\*\)$/, "").replace(/\s*\(proc-macro\)$/, "").replace(/\s*\(\/.*\)$/, "").trim())
      .filter(Boolean),
  );
}

/** Packages in the wasm32 tree that no native tree has. */
export function wasmOnly(wasmText, nativeTexts) {
  const native = new Set(nativeTexts.flatMap((t) => [...packages(t)]));
  return [...packages(wasmText)].filter((p) => !native.has(p)).sort();
}

/** deny.toml's `[licenses] allow` list. */
export function denyAllow(text) {
  const m = text.match(/^\[licenses\][\s\S]*?^allow\s*=\s*\[([\s\S]*?)^\]/m);
  if (!m) return [];
  return [...m[1].replace(/#.*$/gm, "").matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/** Violation lines for wasm-only packages (`name vX.Y.Z`) whose licence (`licences` maps the
 * same ids to an SPDX expression) is missing or not satisfied by `allow` (MPL-2.0 dropped). */
export function licenceViolations(extra, licences, allow) {
  const ok = allow.filter((l) => l !== "MPL-2.0");
  return extra
    .filter((p) => !licences.get(p) || !spdxAllowed(licences.get(p), ok))
    .map(
      (p) =>
        `WASM_ONLY_DEPENDENCY: ${p} is in the ${WASM} tree of ${CRATES.join(", ")} only and is ${licences.get(p) || "unlicensed"}, not on deny.toml's allow list (cargo-deny's [graph] targets keep it out of its own licence check)`,
    );
}

function tree(root, target) {
  const args = ["tree", "-q", "-e", "normal", "--prefix", "none", "-f", "{p}", "--target", target, "--manifest-path", join(root, "Cargo.toml")];
  for (const c of CRATES) args.push("-p", c);
  const r = spawnSync("cargo", args, { encoding: "utf8" });
  if (r.error || r.status !== 0) throw new Error(`cargo tree --target ${target} failed: ${r.stderr ?? r.error?.message}`);
  return r.stdout;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  try {
    const extra = wasmOnly(tree(root, WASM), NATIVE.map((t) => tree(root, t)));
    const licences = new Map(metadata(root, WASM).packages.map((p) => [`${p.name} v${p.version}`, p.license]));
    const bad = licenceViolations(extra, licences, denyAllow(readFileSync(join(root, "deny.toml"), "utf8")));
    for (const line of bad) console.log(line);
    process.exitCode = bad.length ? 1 : 0;
  } catch (e) {
    console.log(`WASM_ONLY_DEPENDENCY: ${e.message}`);
    process.exitCode = 1;
  }
}
