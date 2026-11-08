import { test } from "node:test";
import assert from "node:assert/strict";
import { denyAllow, licenceViolations, packages, wasmOnly } from "./check-wasm-deps.mjs";

test("packages strips cargo tree markers", () => {
  const p = packages("fin-core v0.0.0 (/repo/crates/fin-core)\nserde v1.0.0\nserde v1.0.0 (*)\nserde_derive v1.0.0 (proc-macro)\n");
  assert.deepEqual([...p].sort(), ["fin-core v0.0.0", "serde v1.0.0", "serde_derive v1.0.0"]);
});

test("a wasm32-only package is reported", () => {
  const wasm = "fin-text v0.0.0 (/r)\nlibm v0.2.0\nwasm-bindgen v0.2.0\n";
  assert.deepEqual(wasmOnly(wasm, ["fin-text v0.0.0 (/r)\nlibm v0.2.0\n", "libm v0.2.0\n"]), ["wasm-bindgen v0.2.0"]);
});

test("a package any shipped triple has passes", () => {
  assert.deepEqual(wasmOnly("a v1.0.0\nb v1.0.0\n", ["a v1.0.0\n", "b v1.0.0\n"]), []);
});

test("deny.toml's allow list is read without its comments", () => {
  const text = '[graph]\nallow = ["X"]\n[licenses]\nallow = [\n  "MIT",\n  # a note on "GPL-3.0"\n  "Apache-2.0",\n  "MPL-2.0",\n]\nconfidence-threshold = 0.9\n';
  assert.deepEqual(denyAllow(text), ["MIT", "Apache-2.0", "MPL-2.0"]);
});

test("a wasm-only package passes only with an allowed licence, never MPL-2.0 or none", () => {
  const licences = new Map([
    ["web-sys v0.3.0", "MIT OR Apache-2.0"],
    ["gpl v1.0.0", "GPL-3.0"],
    ["mpl v1.0.0", "MPL-2.0"],
    ["bare v1.0.0", null],
  ]);
  const bad = licenceViolations(["web-sys v0.3.0", "gpl v1.0.0", "mpl v1.0.0", "bare v1.0.0"], licences, ["MIT", "Apache-2.0", "MPL-2.0"]);
  assert.deepEqual(
    bad.map((l) => l.split(" ")[1]),
    ["gpl", "mpl", "bare"],
  );
  assert.match(bad[2], /unlicensed/);
});
