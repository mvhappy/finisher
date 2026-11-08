import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const generator = `process.stdout.write(JSON.stringify({ version: 1, commands: [{ name: "a" }] }));`;
const files = (committed) => ({ "gen.mjs": generator, "packages/contracts/registry.json": committed });
const fresh = `${JSON.stringify({ version: 1, commands: [{ name: "a" }] }, null, 2)}\n`;
const gate = (root, ...extra) => runGate("./check-registry.mjs", ["--root", root, "--generator", `${root}/gen.mjs`, ...extra]);

test("a registry that matches the generated one passes", () => {
  const r = gate(fixtureRoot(files(fresh)));
  assert.equal(r.code, 0, r.out);
});

test("a changed registry is drift", () => {
  const r = gate(fixtureRoot(files(fresh.replace('"a"', '"b"'))));
  assert.equal(r.code, 1);
  assert.match(r.out, /REGISTRY_DRIFT: packages\/contracts\/registry\.json/);
});

test("a missing registry is drift", () => {
  const r = gate(fixtureRoot({ "gen.mjs": generator }));
  assert.equal(r.code, 1);
});

test("--write regenerates it", () => {
  const root = fixtureRoot(files("{}\n"));
  assert.equal(gate(root, "--write").code, 0);
  assert.equal(gate(root).code, 0);
});

test("the gate builds fin from the lockfile", async () => {
  const { readFileSync } = await import("node:fs");
  const src = readFileSync(new URL("./check-registry.mjs", import.meta.url), "utf8");
  assert.match(src, /\["run", "--locked", "-q", "-p", "fin"/);
});
