import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { check, checkTarget, treeTarget } from "./check-crate-deps.mjs";
import { metadata } from "./cargo-graph.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

/** A fake `cargo metadata` graph: `deps` maps a member to [name, kind?, declaration?]. */
function graph(members, deps, others = []) {
  const id = (n) => `path+file:///w/${n}#${n}@0.0.0`;
  const names = [...new Set([...members, ...others, ...Object.values(deps).flat().map((d) => d[0])])];
  return {
    workspace_members: members.map(id),
    packages: names.map((name) => ({
      id: id(name),
      name,
      version: "0.0.0",
      dependencies: (deps[name] ?? []).map(([d, kind, decl]) => ({ name: d, kind: kind ?? null, uses_default_features: true, features: [], ...decl })),
    })),
    resolve: {
      nodes: names.map((name) => ({ id: id(name), deps: (deps[name] ?? []).map(([d, kind]) => ({ name: d, pkg: id(d), dep_kinds: [{ kind: kind ?? null }] })) })),
    },
  };
}

const MEMBERS = ["fin", "fin-node", "fin-media", "fin-container", "fin-codecs", "fin-compositor", "fin-shm", "fin-workers", "fin-fonts"];

test("fin depending on fin-codecs is one failure", () => {
  const out = check(graph(MEMBERS, { fin: [["fin-codecs"]] }));
  assert.deepEqual(out, ["CRATE_DEPENDENCY: fin reaches fin-codecs (fin links no media crate)"]);
});

test("the allowed graph passes, and dev-dependencies are free", () => {
  const out = check(
    graph(MEMBERS, {
      "fin-node": [["fin-media"], ["fin-container"], ["fin-shm"]],
      "fin-container": [["fin-media"]],
      "fin-codecs": [["fin-container"]],
      "fin-compositor": [["fin-media"], ["fin-codecs", "dev"]],
      "fin-workers": [["fin-codecs"], ["fin-compositor"], ["fin-fonts", null, { uses_default_features: false }]],
      fin: [["fin-codecs", "dev"]],
    }),
  );
  assert.deepEqual(out, []);
});

test("fin-node reaching wgpu through another crate, a stranger linking the compositor and workers fetching fonts fail", () => {
  const out = check(
    graph([...MEMBERS, "fin-probe"], {
      "fin-node": [["fin-probe"]],
      "fin-probe": [["fin-compositor"]],
      "fin-compositor": [["wgpu"]],
      "fin-workers": [["fin-fonts", null, { uses_default_features: false, features: ["fetch"] }]],
    }),
  );
  assert.equal(out.length, 4, out.join("\n"));
  assert.match(out.join("\n"), /fin-node reaches fin-compositor/);
  assert.match(out.join("\n"), /fin-node reaches wgpu/);
  assert.match(out.join("\n"), /fin-probe reaches fin-compositor \(only fin-workers and fin-wasm may link it\)/);
  assert.match(out.join("\n"), /fin-workers takes fin-fonts with its fetcher/);
});

test("fin-dataset-inputs may reach fin-core only (sp7 §16.2: never control.db)", () => {
  const members = [...MEMBERS, "fin-dataset-inputs", "fin-core", "fin-journal", "fin-store"];
  assert.deepEqual(check(graph(members, { "fin-dataset-inputs": [["fin-core"], ["fin-journal", "dev"], ["serde_json"]] })), []);
  assert.deepEqual(check(graph(members, { "fin-dataset-inputs": [["fin-core"], ["fin-journal"]], "fin-journal": [["fin-store"]] })), [
    "CRATE_DEPENDENCY: fin-dataset-inputs reaches fin-journal (the dataset builder reads the journal's files only, never control.db)",
    "CRATE_DEPENDENCY: fin-dataset-inputs reaches fin-store (the dataset builder reads the journal's files only, never control.db)",
  ]);
});

test("the gate script reads a fake metadata document", () => {
  const root = fixtureRoot({});
  const meta = join(root, "meta.json");
  writeFileSync(meta, JSON.stringify(graph(MEMBERS, { fin: [["fin-media"]] })));
  const r = runGate("./check-crate-deps.mjs", ["--root", root, "--metadata", meta]);
  assert.equal(r.code, 1);
  assert.match(r.out, /fin reaches fin-media/);
});

test("fin-codecs' platform codec crates are target-gated (6B-1 Task 1)", () => {
  const mac = { target: 'cfg(target_os = "macos")' };
  const win = { target: "cfg(windows)" };
  const gated = {
    "fin-codecs": [
      ["objc2-video-toolbox", null, mac],
      ["objc2-core-media", null, mac],
      ["objc2-core-video", null, mac],
      ["windows", null, win],
    ],
    "fin-workers": [["fin-codecs"], ["fin-fonts", null, { uses_default_features: false }]],
  };
  assert.deepEqual(check(graph(MEMBERS, gated)), []);
  const bare = { ...gated, "fin-codecs": [["objc2-video-toolbox"], ...gated["fin-codecs"].slice(1)] };
  assert.deepEqual(check(graph(MEMBERS, bare)), [
    'CRATE_DEPENDENCY: fin-codecs takes objc2-video-toolbox for every target (only under cfg(target_os = "macos"))',
  ]);
  const wrong = { ...gated, "fin-codecs": [...gated["fin-codecs"].slice(0, 3), ["windows", null, mac]] };
  assert.deepEqual(check(graph(MEMBERS, wrong)), [
    'CRATE_DEPENDENCY: fin-codecs takes windows under cfg(target_os = "macos") (only under cfg(windows))',
  ]);
});

test("fin-plugin-sdk depending on fin-node is one failure (sp8b Task 1)", () => {
  const members = [...MEMBERS, "fin-core", "fin-jobs", "fin-plugin-api", "fin-plugin-sdk", "fin-plugin-test"];
  const allowed = { "fin-plugin-sdk": [["fin-core"], ["fin-jobs"], ["fin-plugin-api"], ["fin-shm"]], "fin-jobs": [["fin-core"]], "fin-shm": [["fin-core"]] };
  assert.deepEqual(check(graph(members, allowed)), []);
  assert.deepEqual(check(graph(members, { ...allowed, "fin-plugin-sdk": [...allowed["fin-plugin-sdk"], ["fin-node"]] })), [
    "CRATE_DEPENDENCY: fin-plugin-sdk reaches fin-node (a plugin built on the SDK carries nothing of the server or the media engine)",
  ]);
});

test("fin and fin-node link neither the plugin SDK nor the harness; the SDK links no wgpu", () => {
  const members = [...MEMBERS, "fin-core", "fin-plugin-sdk", "fin-plugin-test"];
  const out = check(graph(members, { fin: [["fin-plugin-test"]], "fin-node": [["fin-plugin-sdk"]], "fin-plugin-sdk": [["wgpu"]] }));
  assert.deepEqual(out.sort(), [
    "CRATE_DEPENDENCY: fin reaches fin-plugin-test (fin links neither the plugin SDK nor its harness)",
    "CRATE_DEPENDENCY: fin-node reaches fin-plugin-sdk (fin-node links neither the plugin SDK nor its harness)",
    "CRATE_DEPENDENCY: fin-node reaches wgpu (fin-node never links codecs, the compositor or wgpu)",
    "CRATE_DEPENDENCY: fin-plugin-sdk reaches wgpu (a plugin built on the SDK carries no GPU stack)",
  ]);
});

test("fin_node_linking_fin_audio_fails: only fin-workers and fin-wasm link fin-audio (6C Task 1)", () => {
  const members = [...MEMBERS, "fin-audio"];
  assert.deepEqual(check(graph(members, { "fin-workers": [["fin-audio"]], "fin-audio": [["fin-media"]] })), []);
  assert.deepEqual(check(graph(members, { "fin-node": [["fin-audio"]] })), [
    "CRATE_DEPENDENCY: fin-node reaches fin-audio (only fin-workers and fin-wasm may link it)",
  ]);
});

test("fin_codecs_linking_fin_audio_fails: decoded audio crosses fin-codecs at its native rate", () => {
  const members = [...MEMBERS, "fin-audio"];
  const out = check(graph(members, { "fin-codecs": [["fin-audio"]], "fin-workers": [["fin-codecs"]] }));
  assert.deepEqual(out, ["CRATE_DEPENDENCY: fin-codecs reaches fin-audio (only fin-workers and fin-wasm may link it)"]);
});

test("fin-audio links no Finisher crate but fin-core and fin-media", () => {
  const members = [...MEMBERS, "fin-audio", "fin-core"];
  assert.deepEqual(check(graph(members, { "fin-audio": [["fin-core"], ["fin-media"], ["libm"]] })), []);
  assert.deepEqual(check(graph(members, { "fin-audio": [["fin-container"]] })), [
    "CRATE_DEPENDENCY: fin-audio reaches fin-container (the audio crate is pure: it links fin-core and fin-media only)",
  ]);
});

test("the audio sys crates are linked by fin-codecs and fin-workers only", () => {
  const members = [...MEMBERS, "fin-opus-sys", "fin-aacenc-sys"];
  assert.deepEqual(check(graph(members, { "fin-codecs": [["fin-opus-sys"], ["fin-aacenc-sys", null, { target: 'cfg(target_os = "linux")' }]], "fin-workers": [["fin-codecs"]] })), []);
  assert.deepEqual(check(graph(members, { "fin-media": [["fin-opus-sys"]], "fin-container": [["fin-aacenc-sys", null, { target: 'cfg(target_os = "linux")' }]] })), [
    "CRATE_DEPENDENCY: fin-media reaches fin-opus-sys (only fin-codecs and fin-workers may link it)",
    "CRATE_DEPENDENCY: fin-container reaches fin-aacenc-sys (only fin-codecs and fin-workers may link it)",
  ]);
});

test("the AudioToolbox bindings and vo-aacenc are target-gated (6C Task 1)", () => {
  const mac = { target: 'cfg(target_os = "macos")' };
  const linux = { target: 'cfg(target_os = "linux")' };
  const gated = { "fin-codecs": [["objc2-audio-toolbox", null, mac], ["objc2-core-audio-types", null, mac], ["fin-aacenc-sys", null, linux]], "fin-workers": [["fin-codecs"]] };
  assert.deepEqual(check(graph([...MEMBERS, "fin-aacenc-sys"], gated)), []);
  const bare = { ...gated, "fin-codecs": [["objc2-audio-toolbox"], ["fin-aacenc-sys", null, mac]] };
  assert.deepEqual(check(graph([...MEMBERS, "fin-aacenc-sys"], bare)), [
    'CRATE_DEPENDENCY: fin-codecs takes objc2-audio-toolbox for every target (only under cfg(target_os = "macos"))',
    'CRATE_DEPENDENCY: fin-codecs takes fin-aacenc-sys under cfg(target_os = "macos") (only under cfg(target_os = "linux"))',
  ]);
});

test("rustfft_in_graph_fails: no FFT crate in the shipped graph (spec §8.1: SIMD)", () => {
  const out = check(graph(MEMBERS, { "fin-codecs": [["symphonia-core"]], "symphonia-core": [["rustfft"]] }));
  assert.deepEqual(out, ["CRATE_DEPENDENCY: fin-codecs reaches rustfft (no FFT crate in the shipped graph: symphonia's opt-simd features are off)"]);
  assert.deepEqual(check(graph(MEMBERS, { "fin-codecs": [["rustfft", "dev"]] })), []);
});

test("aac_feature_ungated_fails: symphonia's aac feature only under cfg(target_os = \"linux\") (Decision 9)", () => {
  const linux = { target: 'cfg(target_os = "linux")', features: ["aac", "mp3"] };
  assert.deepEqual(check(graph(MEMBERS, { "fin-codecs": [["symphonia", null, linux]], "fin-workers": [["fin-codecs"]] })), []);
  const bare = { features: ["mp3", "aac"] };
  assert.deepEqual(check(graph(MEMBERS, { "fin-codecs": [["symphonia", null, bare]], "fin-workers": [["fin-codecs"]] })), [
    'CRATE_DEPENDENCY: fin-codecs takes symphonia\'s aac feature for every target (only under cfg(target_os = "linux"))',
  ]);
  const mac = { target: 'cfg(target_os = "macos")', features: ["aac"] };
  assert.deepEqual(check(graph([...MEMBERS, "fin-probe"], { "fin-probe": [["symphonia", null, mac]] })), [
    'CRATE_DEPENDENCY: fin-probe takes symphonia\'s aac feature under cfg(target_os = "macos") (only under cfg(target_os = "linux"))',
  ]);
});

test("the aac feature cannot hide behind an umbrella feature or a direct codec dependency", () => {
  const out = (decl, name = "symphonia") => check(graph(MEMBERS, { "fin-codecs": [[name, null, decl]] }));
  assert.equal(out({ features: ["all-codecs"] }).length, 1);
  assert.equal(out({ features: ["all"] }).length, 1);
  assert.equal(out({}, "symphonia-codec-aac").length, 1);
  assert.deepEqual(out({ target: 'cfg(target_os = "linux")' }, "symphonia-codec-aac"), []);
});

test("forwarded features: the resolved graph of a non-Linux target holds no Linux-only AAC code", () => {
  // `fin-workers` forwards `symphonia/aac` through its own feature: only the resolve node shows it.
  const doc = graph(MEMBERS, { "fin-workers": [["symphonia"]], symphonia: [["symphonia-codec-aac"]] });
  assert.deepEqual(check(doc), []);
  assert.deepEqual(checkTarget(doc, "aarch64-apple-darwin"), [
    "CRATE_DEPENDENCY: fin-workers reaches symphonia-codec-aac on aarch64-apple-darwin (Finisher's own AAC code is Linux-only)",
  ]);
  const sys = graph([...MEMBERS, "fin-aacenc-sys"], { "fin-codecs": [["fin-aacenc-sys"]] });
  assert.deepEqual(checkTarget(sys, "x86_64-pc-windows-msvc"), [
    "CRATE_DEPENDENCY: fin-codecs reaches fin-aacenc-sys on x86_64-pc-windows-msvc (Finisher's own AAC code is Linux-only)",
  ]);
  assert.deepEqual(checkTarget(graph(MEMBERS, { "fin-workers": [["fin-codecs"]] }), "aarch64-apple-darwin"), []);
});

test("the audio platform crates are target-gated in every member, not only fin-codecs", () => {
  const members = [...MEMBERS, "fin-aacenc-sys"];
  assert.deepEqual(check(graph(members, { "fin-workers": [["fin-aacenc-sys"]], "fin-codecs": [["fin-aacenc-sys", null, { target: 'cfg(target_os = "linux")' }]] })), [
    'CRATE_DEPENDENCY: fin-workers takes fin-aacenc-sys for every target (only under cfg(target_os = "linux"))',
  ]);
  assert.deepEqual(check(graph(MEMBERS, { "fin-media": [["objc2-audio-toolbox"]] })), [
    'CRATE_DEPENDENCY: fin-media takes objc2-audio-toolbox for every target (only under cfg(target_os = "macos"))',
  ]);
});

test("aac_only_on_linux: the real workspace's macOS and Windows builds hold no AAC code of ours (6C Task 14)", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  for (const t of ["aarch64-apple-darwin", "x86_64-pc-windows-msvc"]) assert.deepEqual(treeTarget(root, t), [], t);
  // Not vacuous: the Linux build has both, through fin-codecs and fin-workers.
  const linux = treeTarget(root, "x86_64-unknown-linux-gnu");
  for (const bad of ["symphonia-codec-aac", "fin-aacenc-sys"])
    for (const who of ["fin-codecs", "fin-workers"]) assert.ok(linux.some((l) => l.startsWith(`CRATE_DEPENDENCY: ${who} reaches ${bad} `)), `${who} ${bad}: ${linux}`);
  // cargo metadata unifies features across targets, so the metadata check would see a false
  // breach for the Linux-gated feature: why the real run uses cargo tree.
  assert.ok(checkTarget(metadata(root, "aarch64-apple-darwin"), "aarch64-apple-darwin").length > 0);
});

test("treeTarget reads cargo tree's dependents, the package itself not counted", () => {
  const run = (_, args) => ({ status: 0, stdout: args.includes("fin-aacenc-sys") ? "fin-aacenc-sys v0.0.0 (/w)\n" : "symphonia-codec-aac v0.6.1\nsymphonia v0.6.1\nfin-codecs v0.0.0 (/w)\nfin-codecs v0.0.0 (/w) (*)\n" });
  assert.deepEqual(treeTarget("/w", "aarch64-apple-darwin", run), [
    "CRATE_DEPENDENCY: symphonia reaches symphonia-codec-aac on aarch64-apple-darwin (Finisher's own AAC code is Linux-only)",
    "CRATE_DEPENDENCY: fin-codecs reaches symphonia-codec-aac on aarch64-apple-darwin (Finisher's own AAC code is Linux-only)",
  ]);
  assert.throws(() => treeTarget("/w", "x", () => ({ status: 101, stderr: "boom" })), /boom/);
});
