// Crate dependency rules (sub-project 6 spec §18, plan 6A Global Constraints), over
// `cargo metadata` (normal and build edges; dev-dependencies are free):
// - `fin` links no media crate;
// - `fin-node` never links `fin-codecs`, `fin-compositor`, `fin-graphics` or `wgpu` (it links
//   `fin-media`, `fin-container` and, for the job-output move, `fin-shm`);
// - only `fin-workers` links `fin-codecs` (and `fin-svt-sys` and `fin-dav1d-sys`, through
//   it), and only `fin-workers` and `fin-wasm` link `fin-compositor`;
// - `fin-audio` is linked only by `fin-workers` and `fin-wasm` (never `fin-codecs`) and links only
//   `fin-core` and `fin-media`; the opus and vo-aacenc sys crates only by `fin-codecs`/`fin-workers`;
//   AudioToolbox bindings only on macOS, `fin-aacenc-sys` and symphonia's `aac` feature only on
//   Linux; no `rustfft` anywhere (6C Task 1).
// - `fin-workers` takes `fin-fonts` with `default-features = false` and without `fetch`
//   (workers never fetch).
// - `fin-codecs`' platform codec crates are target-gated (6B-1 plan Global Constraints): the
//   objc2 VideoToolbox, CoreMedia and CoreVideo bindings only under `cfg(target_os = "macos")`,
//   `windows` (Media Foundation, D3D11) only under `cfg(windows)`.
// - `fin-dataset-inputs` (the dataset builder's inputs; sp7 §12.1, §16.2) links no Finisher
//   crate but `fin-core`: its readers reach the journal's files only, never `control.db` or
//   any crate that opens it (`fin-node`, `fin-registry`, `fin-store`, …).
// - the plugin SDK (`fin-plugin-sdk`, sp8b Task 1) links only `fin-core`, `fin-jobs`,
//   `fin-plugin-api` and `fin-shm` of the workspace, and never wgpu: a plugin built on it carries
//   nothing of the server (`fin-node`, `fin-registry`) or the media engine (`fin-codecs`);
// - `fin` and `fin-node` link neither the SDK nor the harness (`fin-plugin-test`).
// Each breach is CRATE_DEPENDENCY naming the path's two ends.
import { fileURLToPath } from "node:url";
import { Graph, documents, metadata } from "./cargo-graph.mjs";
import { report, rootArg } from "./walk.mjs";

export const MEDIA = ["fin-media", "fin-container", "fin-codecs", "fin-compositor", "fin-shm", "fin-graphics", "fin-audio", "fin-captions"];
export const PLUGIN_TOOLING = ["fin-plugin-sdk", "fin-plugin-test"];
export const RULES = [
  { from: "fin", forbid: MEDIA, why: "fin links no media crate" },
  { from: "fin-node", forbid: ["fin-codecs", "fin-compositor", "fin-graphics", "wgpu"], why: "fin-node never links codecs, the compositor or wgpu" },
  { from: "fin", forbid: PLUGIN_TOOLING, why: "fin links neither the plugin SDK nor its harness" },
  { from: "fin-node", forbid: PLUGIN_TOOLING, why: "fin-node links neither the plugin SDK nor its harness" },
  { from: "fin-plugin-sdk", forbid: ["wgpu"], why: "a plugin built on the SDK carries no GPU stack" },
];
/** Members whose closure may hold no other workspace member than the listed ones. */
export const CONFINED = [
  { crate: "fin-dataset-inputs", may: ["fin-core"], why: "the dataset builder reads the journal's files only, never control.db" },
  { crate: "fin-audio", may: ["fin-core", "fin-media"], why: "the audio crate is pure: it links fin-core and fin-media only" },
  { crate: "fin-plugin-sdk", may: ["fin-core", "fin-jobs", "fin-plugin-api", "fin-shm"], why: "a plugin built on the SDK carries nothing of the server or the media engine" },
];
/** Crates only the listed members may link. */
export const ONLY = [
  { crate: "fin-codecs", by: ["fin-workers"] },
  { crate: "fin-svt-sys", by: ["fin-codecs", "fin-workers"] },
  { crate: "fin-dav1d-sys", by: ["fin-codecs", "fin-workers"] },
  { crate: "fin-compositor", by: ["fin-workers", "fin-wasm"] },
  // Transitive: `fin-codecs` hands out decoded audio at its native rate and layout, and the
  // worker converts it (6C Global Constraints).
  { crate: "fin-audio", by: ["fin-workers", "fin-wasm"] },
  { crate: "fin-opus-sys", by: ["fin-codecs", "fin-workers"] },
  { crate: "fin-aacenc-sys", by: ["fin-codecs", "fin-workers"] },
];

/** Direct dependencies a member may declare only under the given `cfg` target. */
export const TARGETED = [
  { crate: "fin-codecs", deps: ["objc2", "objc2-core-foundation", "objc2-core-media", "objc2-core-video", "objc2-video-toolbox"], cfg: 'cfg(target_os = "macos")' },
  { crate: "fin-codecs", deps: ["windows"], cfg: "cfg(windows)" },
  { crate: "fin-codecs", deps: ["objc2-audio-toolbox", "objc2-core-audio-types"], cfg: 'cfg(target_os = "macos")' },
  { crate: "fin-codecs", deps: ["fin-aacenc-sys"], cfg: 'cfg(target_os = "linux")' },
];
/** The one Linux-gated feature of symphonia (6C Decision 9: Finisher's own AAC code is Linux-only). */
const AAC_CFG = 'cfg(target_os = "linux")';
/** Packages no shipped member may reach: `rustfft` is symphonia's `opt-simd-*` path (spec §8.1). */
const BANNED = [{ name: "rustfft", why: "no FFT crate in the shipped graph: symphonia's opt-simd features are off" }];

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
  for (const t of TARGETED) {
    for (const d of g.packages.get(g.member(t.crate))?.dependencies ?? []) {
      if (!t.deps.includes(d.name) || d.kind === "dev" || d.target === t.cfg) continue;
      out.push(`CRATE_DEPENDENCY: ${t.crate} takes ${d.name} ${d.target ? `under ${d.target}` : "for every target"} (only under ${t.cfg})`);
    }
  }
  for (const [id, p] of g.packages) {
    if (!g.members.has(id)) continue;
    for (const d of p.dependencies ?? []) {
      if (d.name !== "symphonia" || d.kind === "dev" || !(d.features ?? []).includes("aac") || d.target === AAC_CFG) continue;
      out.push(`CRATE_DEPENDENCY: ${p.name} takes symphonia's aac feature ${d.target ? `under ${d.target}` : "for every target"} (only under ${AAC_CFG})`);
    }
  }
  for (const b of BANNED)
    for (const id of g.members) if (reach(g.name(id)).has(b.name)) breach(g.name(id), b.name, b.why);
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
