import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const report = (entries) => JSON.stringify(
  Object.fromEntries(entries.map(([license, name]) => [license, [{ name, versions: ["1.0.0"] }]])),
);
const gate = (root, extra = []) =>
  runGate("./check-js-licenses.mjs", ["--input", `${root}/licenses.json`, "--allowlist", `${root}/allow.json`, ...extra]);

for (const bad of ["GPL-3.0", "LGPL-2.1-only", "AGPL-3.0", "SSPL-1.0"]) {
  test(`${bad} fails`, () => {
    const root = fixtureRoot({ "licenses.json": report([[bad, "bad-pkg"]]), "allow.json": "{}" });
    const r = gate(root);
    assert.equal(r.code, 1);
    assert.match(r.out, new RegExp(`bad-pkg@1\\.0\\.0: ${bad.replace(/[.]/g, "\\.")} is not allowed`));
  });
}

test("non-SPDX licence fails unless allowlisted with a reason", () => {
  const files = { "licenses.json": report([["SEE LICENSE IN README.md", "sdk"]]) };
  assert.equal(gate(fixtureRoot({ ...files, "allow.json": "{}" })).code, 1);
  const allowed = gate(fixtureRoot({ ...files, "allow.json": '{"sdk":"user-installed, reviewed"}' }));
  assert.equal(allowed.code, 0, allowed.out);
});

test("permissive licences pass", () => {
  const root = fixtureRoot({
    "licenses.json": report([["MIT", "a"], ["Apache-2.0", "b"], ["BSD-3-Clause", "c"], ["ISC", "d"], ["MPL-2.0", "e"]]),
    "allow.json": "{}",
  });
  const r = gate(root);
  assert.equal(r.code, 0, r.out);
});
