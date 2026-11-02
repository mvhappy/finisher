import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-action-pins.mjs", ["--root", root]);
const wf = (uses) => ({ ".github/workflows/ci.yml": `jobs:\n  a:\n    steps:\n      - uses: ${uses}\n` });

test("tag-pinned action violates", () => {
  const r = gate(fixtureRoot(wf("actions/checkout@v4")));
  assert.equal(r.code, 1);
  assert.match(r.out, /ci\.yml: actions\/checkout@v4 is not pinned to a commit SHA/);
});

test("SHA-pinned action passes", () => {
  const r = gate(fixtureRoot(wf("actions/checkout@0123456789abcdef0123456789abcdef01234567 # v4.2.2")));
  assert.equal(r.code, 0, r.out);
});

test("local action passes", () => {
  const r = gate(fixtureRoot(wf("./.github/actions/x")));
  assert.equal(r.code, 0, r.out);
});
