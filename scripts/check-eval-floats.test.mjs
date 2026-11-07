import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-eval-floats.mjs", ["--root", root]);
const EVAL = "crates/fin-core/src/eval/x.rs";

test("powf, f64::sin and libm::exp are three failures", () => {
  const r = gate(
    fixtureRoot({
      [EVAL]: "fn a(x: f64) -> f64 { x.powf(2.0) }\nfn b(x: f64) -> f64 { f64::sin(x) }\n",
      "crates/fin-core/src/layout.rs": "fn c(x: f64) -> f64 { libm::exp(x) }\n",
    }),
  );
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").length, 3, r.out);
  assert.match(r.out, /eval\/x\.rs:1: calls \.powf\(\)/);
  assert.match(r.out, /eval\/x\.rs:2: calls f64::sin/);
  assert.match(r.out, /layout\.rs:1: uses libm/);
});

test("every scoped directory, other functions and spellings are refused", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-core/src/params/a.rs": "let (s, c) = x . sin_cos ();\n",
      "crates/fin-core/src/captions/b.rs": "let y = f32 :: mul_add(x, 2.0, 1.0);\n",
      "crates/fin-core/src/eval/deep/c.rs": "use std::f64::consts::PI;\nlet z = x.hypot(y).atan2(w);\n",
      "crates/fin-core/src/eval/d.rs": "let g = <f64>::powi(x, 2);\nlet h = x.exp_m1();\n",
    }),
  );
  assert.equal(r.code, 1);
  for (const want of [
    /params\/a\.rs:1: calls \.sin_cos\(\)/,
    /captions\/b\.rs:1: calls f32::mul_add/,
    /deep\/c\.rs:1: uses the float constants/,
    /deep\/c\.rs:2: calls \.hypot\(\)/,
    /d\.rs:1: calls f64::powi/,
    /d\.rs:2: calls \.exp_m1\(\)/,
  ])
    assert.match(r.out, want);
});

test("allowed operations, comments, strings and other crates pass", () => {
  const r = gate(
    fixtureRoot({
      [EVAL]: [
        "// no x.powf(2.0), no f64::sin(x), no libm::exp(x)",
        "/* x.sin() /* nested */ f64::cos(x) */",
        "/// `sqrt`-shaped, see libm::sqrt",
        'let s = "x.ln() f64::consts libm::";',
        "let r = r#\"x.exp()\"#;",
        "let q = '\"'; let k = x.sqrt().abs().min(1.0).max(0.0).floor();",
        "fn f<'a>(v: &'a [f64]) -> f64 { v[0] * 2.0 / 3.0 - 1.0 + (4 as f64) }",
        "let cosine = sinh_free; let logs = x.login;",
      ].join("\n"),
      "crates/fin-core/src/ops/x.rs": "let y = x.powf(2.0);\n",
      "crates/fin-text/src/lib.rs": "let y = libm::roundf(x);\n",
    }),
  );
  assert.equal(r.code, 0, r.out);
});
