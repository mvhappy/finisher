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

test("the gate script reads a fake metadata document", () => {
  const root = fixtureRoot({});
  const meta = join(root, "meta.json");
  writeFileSync(meta, JSON.stringify(graph(MEMBERS, { fin: [["fin-media"]] })));
  const r = runGate("./check-crate-deps.mjs", ["--root", root, "--metadata", meta]);
  assert.equal(r.code, 1);
  assert.match(r.out, /fin reaches fin-media/);
});
