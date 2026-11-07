import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { loaderUsers, violations } from "./check-runtime-libs.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

/** A fake metadata document where `app` depends on nothing (no loader crates). */
function noLoaders(root) {
  const meta = join(root, "meta.json");
  writeFileSync(meta, JSON.stringify({ workspace_members: ["app"], packages: [{ id: "app", name: "app", version: "0.0.0" }], resolve: { nodes: [{ id: "app", deps: [] }] } }));
  return meta;
}

test("an unlisted dlopen is one failure", () => {
  const root = fixtureRoot({
      "third_party/runtime.json": JSON.stringify({ libraries: [{ name: "libva.so.2", provider: "driver", license: "MIT", note: "" }] }),
      "crates/x/src/lib.rs": 'fn a() { unsafe { dlopen(c"libfoo.so".as_ptr(), 0) }; }\nfn b() { unsafe { libloading::Library::new("libva.so.2") }; }\n',
  });
  const r = runGate("./check-runtime-libs.mjs", ["--root", root, "--metadata", noLoaders(root)]);
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
  const root = fixtureRoot({ "third_party/runtime.json": '{"libraries":[]}', "src/a.ts": "export const x = 1;\n" });
  const r = runGate("./check-runtime-libs.mjs", ["--root", root, "--metadata", noLoaders(root)]);
  assert.equal(r.code, 0, r.out);
});

test("aliased libloading imports and Library::open are checked", () => {
  const src = 'use libloading::{Library as Lib, Symbol};\nfn a() { unsafe { Lib::new("x.so") }; }\nfn b() { let l = libloading::os::unix::Library::open(Some("y.so"), 0); }\n';
  const out = violations("a.rs", src, ["y.so"]);
  assert.equal(out.length, 1, out.join("\n"));
  assert.match(out[0], /a\.rs:2: loads "x\.so"/);
});

test("a shipped crate that depends on a loader crate must be listed", () => {
  const id = (n) => `${n}@1`;
  const doc = {
    workspace_members: [id("app")],
    packages: ["app", "napi-sys", "libloading", "dev-tool"].map((n) => ({ id: id(n), name: n, version: "1.0.0" })),
    resolve: {
      nodes: [
        { id: id("app"), deps: [{ pkg: id("napi-sys"), dep_kinds: [{ kind: null }] }, { pkg: id("dev-tool"), dep_kinds: [{ kind: "dev" }] }] },
        { id: id("napi-sys"), deps: [{ pkg: id("libloading"), dep_kinds: [{ kind: null }] }] },
        { id: id("dev-tool"), deps: [{ pkg: id("libloading"), dep_kinds: [{ kind: null }] }] },
        { id: id("libloading"), deps: [] },
      ],
    },
  };
  assert.deepEqual(loaderUsers(doc, []).map((v) => v.split(" ")[1]), ["napi-sys"]);
  assert.deepEqual(loaderUsers(doc, ["napi-sys"]), []);
});
