import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { checkCatalog, isImmutableUrl } from "./check-font-catalog.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const read = (rel) => JSON.parse(readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8"));
// The cases tests/catalog.rs runs through `FontCatalog::from_json`.
const CASES = read("../crates/fin-fonts/tests/catalog_gate_cases.json");
const RULES = read("../crates/fin-fonts/catalog/rules.json");

function set(doc, pointer, value) {
  const parts = pointer.split("/").slice(1);
  const key = parts.pop();
  const parent = parts.reduce((o, k) => o[k], doc);
  if (value === null) delete parent[key];
  else parent[Array.isArray(parent) ? Number(key) : key] = value;
}

test("the built-in catalog passes", () => {
  assert.deepEqual(checkCatalog(read("../crates/fin-fonts/catalog/catalog.json")), []);
});

test("the shared base catalog passes", () => {
  assert.deepEqual(checkCatalog(CASES.base), []);
});

for (const c of CASES.cases) {
  test(`catalog gate refuses: ${c.name}`, () => {
    assert.ok(RULES.rules[c.rule], `${c.rule} is a rule`);
    const doc = structuredClone(CASES.base);
    for (const [pointer, value] of Object.entries(c.set)) set(doc, pointer, value);
    const found = checkCatalog(doc);
    assert.ok(found.length > 0, "refused");
    // The first violation is the one `FontCatalog::validate` reports (it stops there).
    assert.equal(found[0].rule, c.rule, JSON.stringify(found));
  });
}

test("immutable URLs", () => {
  const c = "0123456789abcdef0123456789abcdef01234567";
  assert.ok(isImmutableUrl(`https://raw.githubusercontent.com/o/r/${c}/a.ttf`));
  assert.ok(isImmutableUrl(`https://cdn.jsdelivr.net/gh/o/r@${c}/a.ttf`));
  assert.ok(isImmutableUrl("https://github.com/o/r/releases/download/v4.1/x.zip"));
  assert.ok(!isImmutableUrl("https://raw.githubusercontent.com/o/r/main/a.ttf"));
  assert.ok(!isImmutableUrl(`https://example.com/a.ttf?v=${c}`));
});

test("the CLI reports FONT_CATALOG_INVALID and exits 1", () => {
  const doc = structuredClone(CASES.base);
  doc.entries[0].licence.spdx = "CC-BY-NC-4.0";
  const root = fixtureRoot({ "crates/fin-fonts/catalog/catalog.json": JSON.stringify(doc) });
  const r = runGate("./check-font-catalog.mjs", ["--root", root]);
  assert.equal(r.code, 1);
  assert.match(r.out, /FONT_CATALOG_INVALID: licence: \/entries\/0\/licence\/spdx: CC-BY-NC-4.0/);
  const ok = fixtureRoot({ "crates/fin-fonts/catalog/catalog.json": JSON.stringify(CASES.base) });
  assert.equal(runGate("./check-font-catalog.mjs", ["--root", ok]).code, 0);
});
