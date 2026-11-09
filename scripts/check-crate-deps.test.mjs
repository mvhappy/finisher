import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { check } from "./check-crate-deps.mjs";
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
