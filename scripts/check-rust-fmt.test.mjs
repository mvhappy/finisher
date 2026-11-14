import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const crate = (lib) => ({
  "Cargo.toml": '[package]\nname = "fmt-fixture"\nversion = "0.0.0"\nedition = "2024"\n\n[workspace]\n',
  "src/lib.rs": lib,
});
const gate = (root, ...more) => runGate("./check-rust-fmt.mjs", ["--root", root, ...more]);

test("formatted Rust passes", () => {
  const r = gate(fixtureRoot(crate("pub fn f() -> u32 {\n    1\n}\n")));
  assert.equal(r.code, 0, r.out);
});

test("unformatted Rust is drift", () => {
  const r = gate(fixtureRoot(crate("pub fn f()->u32{1}\n")));
  assert.equal(r.code, 1);
  assert.match(r.out, /RUST_FMT_DRIFT/);
  assert.match(r.out, /lib\.rs/);
});

test("one package at a time (Windows' path) passes formatted Rust and finds drift", () => {
  assert.equal(gate(fixtureRoot(crate("pub fn f() -> u32 {\n    1\n}\n")), "--per-package").code, 0);
  const r = gate(fixtureRoot(crate("pub fn f()->u32{1}\n")), "--per-package");
  assert.equal(r.code, 1);
  assert.match(r.out, /RUST_FMT_DRIFT/);
  assert.match(r.out, /lib\.rs/);
});
