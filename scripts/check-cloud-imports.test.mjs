import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-cloud-imports.mjs", ["--root", root]);

test("TS import of a cloud package violates", () => {
  const r = gate(fixtureRoot({ "packages/a/src/x.ts": 'import x from "@finisher-cloud/billing";\n' }));
  assert.equal(r.code, 1);
  assert.match(r.out, /x\.ts: imports @finisher-cloud\/billing/);
});

test("Rust use of a cloud crate violates", () => {
  const r = gate(fixtureRoot({ "crates/a/src/lib.rs": "use finisher_cloud::auth;\n" }));
  assert.equal(r.code, 1);
  assert.match(r.out, /lib\.rs: uses finisher_cloud/);
});

test("Cargo dependency on a cloud crate violates", () => {
  const r = gate(
    fixtureRoot({ "crates/a/Cargo.toml": '[dependencies]\nfinisher-cloud-auth = "1"\n' }),
  );
  assert.equal(r.code, 1);
  assert.match(r.out, /Cargo\.toml: depends on finisher-cloud-auth/);
});

test("mentions in comments and docs pass", () => {
  const r = gate(
    fixtureRoot({
      "packages/a/src/x.ts": "// see @finisher-cloud/billing for hosted billing\n",
      "crates/a/src/lib.rs": "// finisher_cloud::auth lives in the private repo\n",
      "docs/cloud.md": 'import x from "@finisher-cloud/billing";\n',
    }),
  );
  assert.equal(r.code, 0, r.out);
});
