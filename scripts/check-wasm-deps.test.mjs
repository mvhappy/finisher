import { test } from "node:test";
import assert from "node:assert/strict";
import { packages, wasmOnly } from "./check-wasm-deps.mjs";

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
