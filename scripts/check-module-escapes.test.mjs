import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-module-escapes.mjs", ["--root", root]);
const repo = fileURLToPath(new URL("..", import.meta.url));

const FORBIDDEN = {
  getBuiltinModule: 'const fs = process.getBuiltinModule("node:fs");\n',
  createRequire: 'import { createRequire } from "node:module";\n',
  moduleRequire: 'const fs = module.require("fs");\n',
};

for (const [name, text] of Object.entries(FORBIDDEN)) {
  test(`flags ${name} under apps/finisherd/src`, () => {
    const r = gate(fixtureRoot({ "apps/finisherd/src/agent/claude/x.ts": text }));
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /MODULE_ESCAPE apps\/finisherd\/src\/agent\/claude\/x\.ts:1: /);
  });
}

test("allows comments, tests and other folders", () => {
  const r = gate(
    fixtureRoot({
      "apps/finisherd/src/doc.ts": "// process.getBuiltinModule is not used here\n",
      "apps/finisherd/test/x.test.ts": FORBIDDEN.createRequire,
      "packages/fin-node/native.js": FORBIDDEN.createRequire,
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("the repository passes", () => {
  const r = gate(repo);
  assert.equal(r.code, 0, r.out);
});
