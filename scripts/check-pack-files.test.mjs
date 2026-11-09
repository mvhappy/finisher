import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const pkg = (files) => JSON.stringify({ name: "@finisher/fin-node", version: "0.0.0", private: true, ...(files ? { files } : {}) });
const tree = (files) => ({
  "packages/fin-node/package.json": pkg(files),
  "packages/fin-node/index.js": "export {};\n",
  "packages/fin-node/index.d.ts": "export {};\n",
  "packages/fin-node/test/a.test.ts": "export {};\n",
  "packages/fin-node/test/native/fin-workers": "binary",
  "packages/fin-node/scripts/copy-workers.mjs": "export {};\n",
});
const gate = (root) => runGate("./check-pack-files.mjs", ["--root", root]);

test("a package.json files list that leaves out tests and test binaries passes", () => {
  const r = gate(fixtureRoot(tree(["*.js", "*.d.ts"])));
  assert.equal(r.code, 0, r.out);
});

test("no files list packs the tests and the test binaries", () => {
  const r = gate(fixtureRoot(tree(undefined)));
  assert.equal(r.code, 1);
  assert.match(r.out, /PACK_CONTENTS: packages\/fin-node would pack test\/a\.test\.ts/);
  assert.match(r.out, /PACK_CONTENTS: packages\/fin-node would pack test\/native\/fin-workers/);
});

test("a files list without the entry point fails", () => {
  const r = gate(fixtureRoot(tree(["*.d.ts"])));
  assert.equal(r.code, 1);
  assert.match(r.out, /would not pack index\.js/);
});
