import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, lines, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-file-size.mjs", ["--root", root]);

test("source file over 600 lines violates", () => {
  const r = gate(fixtureRoot({ "crates/a/src/big.rs": lines(601) }));
  assert.equal(r.code, 1);
  assert.match(r.out, /big\.rs: 601 lines > 600/);
});

test("test file over 1500 lines violates", () => {
  const r = gate(fixtureRoot({ "packages/a/src/big.test.ts": lines(1501) }));
  assert.equal(r.code, 1);
  assert.match(r.out, /big\.test\.ts: 1501 lines > 1500/);
});

test("files at the limit pass", () => {
  const r = gate(fixtureRoot({ "crates/a/src/ok.rs": lines(600), "src/ok.test.ts": lines(1500) }));
  assert.equal(r.code, 0, r.out);
});

test("ignored directories are skipped", () => {
  const r = gate(
    fixtureRoot({
      "target/x.rs": lines(900),
      "node_modules/p/x.ts": lines(900),
      "packages/a/src/generated/types.ts": lines(900),
    }),
  );
  assert.equal(r.code, 0, r.out);
});
