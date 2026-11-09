import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";
import { discover, parse, refusal, violations } from "./js-licenses.mjs";

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

// Review of 2026-11-09, #9: a real SPDX expression parser, so parentheses no longer hide a
// copyleft term.
for (const [expr, blocker] of [
  ["(MIT OR Apache-2.0) AND GPL-3.0-only", "GPL-3.0-only"],
  ["MIT AND GPL-3.0-only", "GPL-3.0-only"],
  ["(GPL-3.0-only OR MIT) AND AGPL-3.0", "AGPL-3.0"],
  ["GPL-2.0-only WITH Classpath-exception-2.0", "GPL-2.0-only, Classpath-exception-2.0"],
  ["GPL-2.0+", "GPL-2.0+"],
  ["mit", "mit"],
  ["MIT WITH Commons-Clause", "Commons-Clause"],
]) {
  test(`${expr} fails, blocked by ${blocker}`, () => {
    const root = fixtureRoot({ "licenses.json": report([[expr, "pkg"]]), "allow.json": "{}" });
    const r = gate(root);
    assert.equal(r.code, 1, r.out);
    assert.ok(r.out.includes(`pkg@1.0.0: ${expr} is not allowed (blocked by ${blocker})`), r.out);
  });
}

for (const expr of ["MIT and Apache-2.0", "(MIT", "MIT)", "MIT OR", "MIT WITH", "(MIT OR Apache-2.0) WITH x", ""]) {
  test(`${JSON.stringify(expr)} is not a valid expression and fails`, () => {
    const root = fixtureRoot({ "licenses.json": report([[expr, "pkg"]]), "allow.json": "{}" });
    const r = gate(root);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /not a valid SPDX expression/);
  });
}

test("allowed compound expressions pass", () => {
  const root = fixtureRoot({
    "licenses.json": report([
      ["(MIT OR Apache-2.0)", "a"],
      ["MIT AND (Apache-2.0 OR BSD-3-Clause)", "b"],
      ["(GPL-3.0-only OR MIT) AND ISC", "c"],
      ["Apache-2.0 WITH LLVM-exception", "d"],
      ["((MIT))", "e"],
      ["GPL-2.0-only OR (MIT AND Zlib)", "f"],
      ["Apache-2.0+", "g"],
    ]),
    "allow.json": "{}",
  });
  const r = gate(root);
  assert.equal(r.code, 0, r.out);
});

test("OR binds looser than AND", () => {
  // `MIT OR GPL-3.0-only AND AGPL-3.0` is `MIT OR (GPL AND AGPL)`: MIT allows it.
  assert.equal(refusal("MIT OR GPL-3.0-only AND AGPL-3.0"), null);
  assert.equal(refusal("GPL-3.0-only AND AGPL-3.0 OR MIT"), null);
  assert.deepEqual(parse("A OR B AND C WITH E"), { or: [{ id: "A" }, { and: [{ id: "B" }, { id: "C", exception: "E" }] }] });
});

// Review of 2026-11-09, #7: the gate checks every shipped workspace's production packages.
test("an empty report fails: the gate would check nothing", () => {
  const r = gate(fixtureRoot({ "licenses.json": "{}", "allow.json": "{}" }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /names no package/);
});

test("the real discovery finds finisherd's production packages", () => {
  const found = JSON.parse(discover());
  const names = new Set(Object.values(found).flat().map((p) => p.name));
  assert.ok(names.size > 0, "no package discovered");
  for (const dep of ["effect", "ws", "jose", "@effect/platform-node"]) {
    assert.ok(names.has(dep), `${dep} (a finisherd dependency) is not checked: ${[...names].join(", ")}`);
  }
  assert.deepEqual(violations(found, JSON.parse(readFileSync(new URL("./license-allowlist.json", import.meta.url), "utf8"))), []);
});
