import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-eval-floats.mjs", ["--root", root]);
const EVAL = "crates/fin-core/src/eval/x.rs";

test("powf, f64::sin and libm::exp fail (libm::exp as both libm and ::exp)", () => {
  const r = gate(
    fixtureRoot({
      [EVAL]: "fn a(x: f64) -> f64 { x.powf(2.0) }\nfn b(x: f64) -> f64 { f64::sin(x) }\n",
      "crates/fin-core/src/layout.rs": "fn c(x: f64) -> f64 { libm::exp(x) }\n",
    }),
  );
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").length, 4, r.out);
  assert.match(r.out, /eval\/x\.rs:1: calls \.powf\(\)/);
  assert.match(r.out, /eval\/x\.rs:2: calls ::sin/);
  assert.match(r.out, /layout\.rs:1: uses libm/);
  assert.match(r.out, /layout\.rs:1: calls ::exp/);
});

test("every scoped directory, other functions and spellings are refused", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-core/src/params/a.rs": "let (s, c) = x . sin_cos ();\n",
      "crates/fin-core/src/model/b.rs": "let y = f32 :: mul_add(x, 2.0, 1.0);\n",
      "crates/fin-core/src/eval/deep/c.rs": "use std::f64::consts::PI;\nlet z = x.hypot(y).atan2(w);\n",
      "crates/fin-core/src/eval/d.rs": "let g = <f64>::powi(x, 2);\nlet h = x.exp_m1();\n",
    }),
  );
  assert.equal(r.code, 1);
  for (const want of [
    /params\/a\.rs:1: calls \.sin_cos\(\)/,
    /model\/b\.rs:1: calls ::mul_add/,
    /deep\/c\.rs:1: uses the float constants/,
    /deep\/c\.rs:2: calls \.hypot\(\)/,
    /d\.rs:1: calls ::powi/,
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
        "let cosine = sinh_free; let logs = x.login; let e = Op::$variant { .. };",
        "let t = x.sinh_free(); let u = path::lnx(1); let v = x.exp;",
      ].join("\n"),
      "crates/fin-core/tests/x.rs": "let y = x.powf(2.0);\n",
      "crates/fin-text/src/lib.rs": "let y = libm::roundf(x);\n",
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("module-root files outside eval/params/layout are in scope", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-core/src/time.rs": "fn a(x: f64) -> f64 { x.ln() }\n",
      "crates/fin-core/src/lib.rs": "fn b(x: f64) -> f64 { x.cbrt() }\n",
      "crates/fin-core/src/ops/c.rs": "fn c(x: f64) -> f64 { x.exp2() }\n",
    }),
  );
  assert.equal(r.code, 1);
  for (const want of [/time\.rs:1: calls \.ln\(\)/, /lib\.rs:1: calls \.cbrt\(\)/, /ops\/c\.rs:1: calls \.exp2\(\)/])
    assert.match(r.out, want);
});

test("aliases, generic and qualified paths, function values, split calls and macros are refused", () => {
  const r = gate(
    fixtureRoot({
      [EVAL]: [
        "type F = f64; let a = F::sin(x);", // 1: alias
        "fn g<G: Float>(x: G) -> G { G::exp(x) }", // 2: generic
        "fn t<T: Float>(x: T) -> T { T::powf(x, x) }", // 3: generic
        "let b = <f64 as Tr>::powf(x, 2.0);", // 4: qualified
        "impl W { fn n(self) -> f64 { Self::ln(self) } }", // 5: Self
        "let c = v.iter().map(f64::cos);", // 6: function value, no call
        "let d = x.sin", // 7: split across lines …
        "  ();", // 8
        "let e = x", // 9: … and across the dot
        "  .tanh(", // 10
        "  );", // 11
        "macro_rules! m { ($f:ident) => { f64::$f(x) }; }", // 12
        "macro_rules! k { ($f:ident) => { x.$f() }; }", // 13
        "macro_rules! q { ($f:ident) => { <f64 as Tr>::$f(x) }; }", // 14
      ].join("\n"),
    }),
  );
  assert.equal(r.code, 1, r.out);
  for (const want of [
    /x\.rs:1: calls ::sin/,
    /x\.rs:2: calls ::exp/,
    /x\.rs:3: calls ::powf/,
    /x\.rs:4: calls ::powf/,
    /x\.rs:5: calls ::ln/,
    /x\.rs:6: calls ::cos/,
    /x\.rs:7: calls \.sin\(\)/,
    /x\.rs:10: calls \.tanh\(\)/,
    /x\.rs:12: calls f64::\$metavariable/,
    /x\.rs:13: calls a metavariable method/,
    /x\.rs:14: calls f64::\$metavariable/,
  ])
    assert.match(r.out, want);
  assert.equal(r.out.trim().split("\n").length, 11, r.out);
});

test("split calls hidden by a comment or string between the parts still count; text in them does not", () => {
  const r = gate(
    fixtureRoot({
      [EVAL]: ["let a = x.sin /* gap */ ();", 'let b = "x.sin\n()";', "// x.sin", "// ()"].join("\n"),
    }),
  );
  assert.equal(r.code, 1, r.out);
  assert.equal(r.out.trim().split("\n").length, 1, r.out);
  assert.match(r.out, /x\.rs:1: calls \.sin\(\)/);
});

test("render modules refuse platform transcendentals and SIMD but allow libm and the constants", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-compositor/src/reference/blend.rs": "fn a(x: f32) -> f32 { x.powf(2.4) }\nfn b(x: f32) -> f32 { f32::powf(x, 2.4) }\n",
      "crates/fin-media/src/colour/convert.rs": "use std::arch::aarch64::*;\nfn c(x: f64) -> f64 { x.mul_add(2.0, 1.0) }\n",
      "crates/fin-compositor/src/geometry.rs": [
        "fn d(r: f64) -> f64 { libm::sin(r) + libm :: cos(r) + core::f64::consts::PI }",
        "fn e(x: f64) -> f64 { libm::log2(x).max(0.0) + x.sqrt() }",
      ].join("\n"),
      "crates/fin-media/src/sample.rs": "fn f(x: f64) -> f64 { x.floor() }\n",
      // Outside the render modules nothing is checked.
      "crates/fin-compositor/src/gpu.rs": "fn g(x: f32) -> f32 { x.powf(2.0) }\n",
      "crates/fin-media/src/info.rs": "fn h(x: f64) -> f64 { x.ln() }\n",
    }),
  );
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").length, 4, r.out);
  assert.match(r.out, /reference\/blend\.rs:1: calls \.powf\(\) \(render floats/);
  assert.match(r.out, /reference\/blend\.rs:2: calls ::powf/);
  assert.match(r.out, /colour\/convert\.rs:1: uses SIMD intrinsics/);
  assert.match(r.out, /colour\/convert\.rs:2: calls \.mul_add\(\)/);
});

test("the tone mapping and soft clip are render modules, and must stay where the gate looks", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-media/src/colour/mod.rs": "pub mod softclip;\npub mod tonemap;\n",
      "crates/fin-media/src/colour/tonemap.rs": "fn pq(x: f32) -> f32 { x.powf(78.84) + libm::powf(x, 0.1593) }\n",
      "crates/fin-media/src/colour/softclip.rs": "fn s(t: f32) -> f32 { libm::sqrtf(1.0 + t * t) + f32::exp(t) }\n",
    }),
  );
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").length, 2, r.out);
  assert.match(r.out, /colour\/tonemap\.rs:1: calls \.powf\(\) \(render floats/);
  assert.match(r.out, /colour\/softclip\.rs:1: calls ::exp/);

  const moved = gate(
    fixtureRoot({
      "crates/fin-media/src/colour/mod.rs": "pub mod softclip;\n",
      "crates/fin-media/src/colour/softclip.rs": "fn s(t: f32) -> f32 { libm::sqrtf(t) }\n",
      "crates/fin-media/src/hdr.rs": "fn pq(x: f32) -> f32 { x.powf(78.84) }\n",
    }),
  );
  assert.equal(moved.code, 1);
  assert.match(moved.out, /colour\/tonemap\.rs: missing/);
  assert.equal(moved.out.trim().split("\n").length, 1, moved.out);
});
