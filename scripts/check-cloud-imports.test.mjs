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

const violates = (files, pattern) => {
  const r = gate(fixtureRoot(files));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, pattern);
};
const passes = (files) => {
  const r = gate(fixtureRoot(files));
  assert.equal(r.code, 0, r.out);
};

test("every TS/JS way to load a cloud package violates", () => {
  const cases = {
    "dynamic import": 'const p = await import("@finisher-cloud/payments");\n',
    "dynamic import, single quotes, options": "const p = await import('@finisher-cloud/payments', { with: { type: 'json' } });\n",
    "dynamic import with a template literal": "const p = await import(`@finisher-cloud/payments`);\n",
    "dynamic import after a comment": 'import(/* webpackIgnore: true */ "@finisher-cloud/payments");\n',
    "import type over several lines": 'import type {\n  A,\n  B,\n} from\n  "@finisher-cloud/payments";\n',
    "side-effect import": 'import "@finisher-cloud/payments/register";\n',
    "namespace re-export": 'export * from "@finisher-cloud/payments";\n',
    "named re-export": 'export { a as b } from "@finisher-cloud/payments";\n',
    "require": 'const p = require("@finisher-cloud/payments");\n',
    "require.resolve": 'const p = require.resolve("@finisher-cloud/payments");\n',
    "import-equals": 'import p = require("@finisher-cloud/payments");\n',
    "typeof import": 'type P = typeof import("@finisher-cloud/payments");\n',
    "bare scope": 'import x from "@finisher-cloud";\n',
    "after a regex literal with a quote": 'const re = /["\']/g;\nimport("@finisher-cloud/payments");\n',
    "inside a template substitution": "const x = `${await import('@finisher-cloud/payments')}`;\n",
  };
  for (const [name, source] of Object.entries(cases)) {
    for (const file of ["packages/a/src/x.ts", "apps/b/x.mjs", "apps/b/x.cjs"]) {
      const r = gate(fixtureRoot({ [file]: source }));
      assert.equal(r.code, 1, `${name} (${file}): ${r.out}`);
      assert.match(r.out, /imports @finisher-cloud(\/payments(\/register)?)?$/m, name);
    }
  }
});

test("things that only look like cloud imports pass", () => {
  passes({
    "a.ts": [
      "// import x from '@finisher-cloud/payments'",
      "/* await import('@finisher-cloud/payments') */",
      'const s = "import x from \'@finisher-cloud/payments\'";',
      "const t = `require('@finisher-cloud/payments')`;",
      'const b = Buffer.from("@finisher-cloud/payments");',
      'const o = { from: "@finisher-cloud/payments", import: "@finisher-cloud/x" };',
      'import y from "@finisher/core";',
      'const name = "@finisher-cloud/" + "payments"; import(name);',
      "",
    ].join("\n"),
  });
});

test("Rust: nested use, extern crate, inline paths and aliases violate; comments pass", () => {
  for (const source of [
    "use {std::fmt, finisher_cloud::auth};\n",
    "pub use finisher_cloud_billing::Plan;\n",
    "extern crate finisher_cloud as cloud;\n",
    "fn f() { finisher_cloud::auth::login(); }\n",
    "fn f() { ::finisher_cloud_payments::charge(); }\n",
  ]) {
    violates({ "crates/a/src/lib.rs": source }, /lib\.rs: uses finisher_cloud/);
  }
  passes({ "crates/a/src/lib.rs": '// finisher_cloud::auth\n/* use finisher_cloud::x; */\nconst S: &str = "finisher_cloud::auth";\n' });
});

test("Cargo: a renamed dependency (package = ...) violates", () => {
  violates(
    { "crates/a/Cargo.toml": '[package]\nname = "a"\n[dependencies]\nprivate = { package = "finisher-cloud-payments", version = "1" }\n' },
    /Cargo\.toml: depends on finisher-cloud-payments/,
  );
});

test("Cargo: every spelling of a cloud dependency violates", () => {
  const cases = {
    "dependency table": '[dependencies.finisher-cloud-auth]\nversion = "1"\n',
    "dev-dependency": '[dev-dependencies]\nfinisher-cloud-auth = { path = "../x" }\n',
    "target-specific": '[target.\'cfg(unix)\'.dependencies]\nfinisher-cloud-auth = "1"\n',
    "workspace dependency": '[workspace.dependencies]\nfinisher-cloud-auth = { path = "../x" }\n',
    "workspace alias": '[workspace.dependencies]\ncloud = { package = "finisher-cloud-auth", path = "../x" }\n',
    "dotted package key": '[dependencies]\ncloud.package = "finisher-cloud-auth"\ncloud.version = "1"\n',
    "package table": '[dependencies.cloud]\npackage = "finisher-cloud-auth"\nversion = "1"\n',
    "single quotes": "[dependencies]\nprivate = { package = 'finisher_cloud_auth', version = '1' }\n",
    "quoted key": '[dependencies]\n"finisher-cloud-auth" = "1"\n',
    "patch": '[patch.crates-io]\nfinisher-cloud-auth = { path = "../x" }\n',
  };
  for (const [name, toml] of Object.entries(cases)) {
    const r = gate(fixtureRoot({ "crates/a/Cargo.toml": toml }));
    assert.equal(r.code, 1, `${name}: ${r.out}`);
    assert.match(r.out, /depends on finisher[-_]cloud[-_]auth/, name);
  }
});

test("Cargo.lock: a resolved cloud package violates", () => {
  violates({ "Cargo.lock": '[[package]]\nname = "serde"\nversion = "1.0.0"\n\n[[package]]\nname = "finisher-cloud-auth"\nversion = "0.1.0"\n' }, /Cargo\.lock: resolves finisher-cloud-auth/);
});

test("Cargo: comments and similar names pass", () => {
  passes({
    "crates/a/Cargo.toml": '[dependencies]\n# finisher-cloud-auth = "1"\nserde = "1" # package = "finisher-cloud-auth"\nfinisher-core = { path = "../fin-core" }\n',
    "Cargo.lock": '[[package]]\nname = "finisher-core"\n',
  });
});

test("package.json: a cloud dependency or npm alias violates", () => {
  violates({ "apps/a/package.json": '{"dependencies":{"@finisher-cloud/billing":"1.0.0"}}' }, /package\.json: depends on @finisher-cloud\/billing/);
  violates({ "apps/a/package.json": '{"devDependencies":{"billing":"npm:@finisher-cloud/billing@1"}}' }, /depends on @finisher-cloud\/billing/);
  passes({ "apps/a/package.json": '{"name":"@finisher-cloud-docs","dependencies":{"zod":"4"}}' });
});
