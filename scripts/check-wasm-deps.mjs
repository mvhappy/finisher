import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { rootArg } from "./walk.mjs";

// `deny.toml` `[graph] targets` lists only the shipped OS triples, so cargo-deny never sees a
// dependency that only wasm32 pulls in. fin-core, fin-text and fin-plugin-api also build for
// `wasm32-unknown-unknown`: this gate fails if their wasm32 dependency tree holds a package
// that none of the shipped triples' trees holds (WASM_ONLY_DEPENDENCY), so such a package
// cannot skip the licence check.

export const CRATES = ["fin-core", "fin-text", "fin-plugin-api"];
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
    for (const p of extra) console.log(`WASM_ONLY_DEPENDENCY: ${p} is in the ${WASM} tree of ${CRATES.join(", ")} only; deny.toml's [graph] targets would keep it out of the licence check`);
    process.exitCode = extra.length ? 1 : 0;
  } catch (e) {
    console.log(`WASM_ONLY_DEPENDENCY: ${e.message}`);
    process.exitCode = 1;
  }
}
