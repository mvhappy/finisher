// Crate dependency rules (sub-project 6 spec §18, plan 6A Global Constraints), over
// `cargo metadata` (normal and build edges; dev-dependencies are free):
// - `fin` links no media crate;
// - `fin-node` never links `fin-codecs`, `fin-compositor`, `fin-graphics` or `wgpu` (it links
//   `fin-media`, `fin-container` and, for the job-output move, `fin-shm`);
// - only `fin-workers` links `fin-codecs` (and `fin-svt-sys` and `fin-dav1d-sys`, through
//   it), and only `fin-workers` and `fin-wasm` link `fin-compositor`;
// - `fin-workers` takes `fin-fonts` with `default-features = false` and without `fetch`
//   (workers never fetch).
// - `fin-dataset-inputs` (the dataset builder's inputs; sp7 §12.1, §16.2) links no Finisher
//   crate but `fin-core`: its readers reach the journal's files only, never `control.db` or
//   any crate that opens it (`fin-node`, `fin-registry`, `fin-store`, …).
// Each breach is CRATE_DEPENDENCY naming the path's two ends.
import { fileURLToPath } from "node:url";
import { Graph, documents, metadata } from "./cargo-graph.mjs";
import { report, rootArg } from "./walk.mjs";

export const MEDIA = ["fin-media", "fin-container", "fin-codecs", "fin-compositor", "fin-shm", "fin-graphics", "fin-audio", "fin-captions"];
export const RULES = [
  { from: "fin", forbid: MEDIA, why: "fin links no media crate" },
  { from: "fin-node", forbid: ["fin-codecs", "fin-compositor", "fin-graphics", "wgpu"], why: "fin-node never links codecs, the compositor or wgpu" },
];
/** Members whose closure may hold no other workspace member than the listed ones. */
export const CONFINED = [{ crate: "fin-dataset-inputs", may: ["fin-core"], why: "the dataset builder reads the journal's files only, never control.db" }];
/** Crates only the listed members may link. */
export const ONLY = [
  { crate: "fin-codecs", by: ["fin-workers"] },
  { crate: "fin-svt-sys", by: ["fin-codecs", "fin-workers"] },
  { crate: "fin-dav1d-sys", by: ["fin-codecs", "fin-workers"] },
  { crate: "fin-compositor", by: ["fin-workers", "fin-wasm"] },
];

export function check(doc) {
  const g = new Graph(doc);
  const out = [];
  const seen = new Set();
  const breach = (from, to, why) => {
    if (seen.has(`${from}>${to}`)) return;
    seen.add(`${from}>${to}`);
    out.push(`CRATE_DEPENDENCY: ${from} reaches ${to} (${why})`);
  };
  const reach = (name) => {
    const id = g.member(name);
    return id ? new Set([...g.closure([id])].filter((x) => x !== id).map((x) => g.name(x))) : new Set();
  };
  for (const r of RULES) {
    const got = reach(r.from);
    for (const f of r.forbid) if (got.has(f)) breach(r.from, f, r.why);
  }
  for (const o of ONLY) {
    for (const id of g.members) {
      const name = g.name(id);
      if (name === o.crate || o.by.includes(name)) continue;
      if (reach(name).has(o.crate)) breach(name, o.crate, `only ${o.by.join(" and ")} may link it`);
    }
  }
  for (const c of CONFINED) {
    if (!g.member(c.crate)) continue;
    for (const id of g.members) {
      const name = g.name(id);
      if (name !== c.crate && !c.may.includes(name) && reach(c.crate).has(name)) breach(c.crate, name, c.why);
    }
  }
  const workers = g.packages.get(g.member("fin-workers"));
  for (const d of workers?.dependencies ?? []) {
    if (d.name !== "fin-fonts" || d.kind === "dev") continue;
    if (d.uses_default_features !== false || (d.features ?? []).includes("fetch"))
      out.push("CRATE_DEPENDENCY: fin-workers takes fin-fonts with its fetcher (use default-features = false; workers never fetch)");
  }
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = rootArg();
  try {
    report(documents(process.argv, () => [metadata(root)]).flatMap(check));
  } catch (e) {
    report([`CRATE_DEPENDENCY: ${e.message}`]);
  }
}
