import { test } from "node:test";
import assert from "node:assert/strict";
import { violations } from "./check-runtime-libs.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

test("an unlisted dlopen is one failure", () => {
  const r = runGate("./check-runtime-libs.mjs", [
    "--root",
    fixtureRoot({
      "third_party/runtime.json": JSON.stringify({ libraries: [{ name: "libva.so.2", provider: "driver", license: "MIT", note: "" }] }),
      "crates/x/src/lib.rs": 'fn a() { unsafe { dlopen(c"libfoo.so".as_ptr(), 0) }; }\nfn b() { unsafe { libloading::Library::new("libva.so.2") }; }\n',
    }),
  ]);
  assert.equal(r.code, 1);
  assert.equal(r.out.trim().split("\n").length, 1, r.out);
  assert.match(r.out, /RUNTIME_LIB_UNLISTED: crates\/x\/src\/lib\.rs:1: loads "libfoo\.so"/);
});

test("listed names, comments and other files pass; a non-literal name and LoadLibraryW are checked", () => {
  const names = ["d3dcompiler_47.dll"];
  assert.deepEqual(violations("a.rs", '// dlopen("libfoo.so")\n/* LoadLibraryW("x.dll") */\nlet s = "dlopen(";\n', names), []);
  assert.deepEqual(violations("a.ts", 'process.dlopen(module, "d3dcompiler_47.dll");\n', names), []);
  assert.equal(violations("a.ts", 'process.dlopen(mod, "evil.node");\n', names).length, 1);
  assert.equal(violations("a.rs", "LoadLibraryW(w!(\"evil.dll\"));\n", names).length, 1);
  assert.match(violations("a.rs", "Library::new(path)\n", names)[0], /not a string literal/);
});

test("an empty list with no loads passes", () => {
  const r = runGate("./check-runtime-libs.mjs", ["--root", fixtureRoot({ "third_party/runtime.json": '{"libraries":[]}', "src/a.ts": "export const x = 1;\n" })]);
  assert.equal(r.code, 0, r.out);
});
