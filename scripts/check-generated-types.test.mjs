import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const generator = `
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const out = process.argv[process.argv.indexOf("--out") + 1];
mkdirSync(join(out, "schema"), { recursive: true });
mkdirSync(join(out, "src/generated"), { recursive: true });
writeFileSync(join(out, "schema/fin-core.schema.json"), "{}\\n");
writeFileSync(join(out, "src/generated/index.ts"), "export type A = 1;\\n");
`;
const files = (ts) => ({
  "packages/core-types/scripts/generate.mjs": generator,
  "packages/core-types/schema/fin-core.schema.json": "{}\n",
  "packages/core-types/src/generated/index.ts": ts,
});
const gate = (root) => runGate("./check-generated-types.mjs", ["--root", root]);

test("matching generated files pass", () => {
  const r = gate(fixtureRoot(files("export type A = 1;\n")));
  assert.equal(r.code, 0, r.out);
});

test("a hand-edited generated file is drift", () => {
  const r = gate(fixtureRoot(files("export type A = 2;\n")));
  assert.equal(r.code, 1);
  assert.match(r.out, /GENERATED_TYPES_DRIFT: packages\/core-types\/src\/generated\/index\.ts/);
});
