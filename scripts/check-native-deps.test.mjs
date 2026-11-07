import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { blake3 } from "./blake3.mjs";
import { check, shipped } from "./check-native-deps.mjs";
import { spdxAllowed } from "./cargo-graph.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const ALLOW = ["BSD-2-Clause", "BSD-3-Clause", "BSD-3-Clause-Clear", "MIT", "ISC", "Zlib", "Apache-2.0"];

/** A fake `cargo metadata` document: `pkgs` are [name, {links, license, deps: [[name, kind]]}]. */
function fakeMetadata(members, pkgs) {
  const id = (n) => `path+file:///w/${n}#${n}@1.0.0`;
  return {
    workspace_members: members.map(id),
    packages: pkgs.map(([name, p]) => ({ id: id(name), name, version: "1.0.0", license: p.license ?? "MIT", links: p.links ?? null, dependencies: [] })),
    resolve: {
      nodes: pkgs.map(([name, p]) => ({
        id: id(name),
        deps: (p.deps ?? []).map(([d, kind]) => ({ name: d, pkg: id(d), dep_kinds: [{ kind: kind ?? null, target: null }] })),
      })),
    },
  };
}

const LICENSE = "Copyright (c) the authors. BSD-3-Clause-Clear.\n";

function baseNative(extra = {}) {
  return {
    libraries: [{ name: "LibFoo", package: "foo-sys", tag: "v1.2.3", license: "BSD-3-Clause", files: { "third_party/foo/LICENSE": blake3(Buffer.from(LICENSE)) }, ...extra }],
    allow: ALLOW,
    patentLicences: ["AOMedia-Patent-License-1.0"],
  };
}

const NOTICES = "# Notices\n\n## Native libraries\n\n### LibFoo (BSD-3-Clause) v1.2.3\nstatus: used\n";

test("the four failures: an unlisted links package, a GPL licence, a changed licence hash and a missing notice", () => {
  const doc = fakeMetadata(["app"], [
    ["app", { deps: [["foo-sys"], ["bar-sys"]] }],
    ["foo-sys", { links: "foo", deps: [["cc", "build"]] }],
    ["bar-sys", { links: "bar" }],
    ["cc", {}],
  ]);
  const native = baseNative({ license: "GPL-2.0-only", tag: "v9.9.9" });
  const out = check({
    pkgs: shipped([doc]),
    native,
    notices: NOTICES,
    gitlink: () => null,
    hash: () => blake3(Buffer.from("a different licence\n")),
  });
  assert.equal(out.length, 4, out.join("\n"));
  assert.match(out[0], /^NATIVE_UNLISTED: bar-sys 1\.0\.0 \(links = "bar"\)/);
  assert.match(out.join("\n"), /NATIVE_LICENSE: LibFoo \(foo-sys\) is GPL-2\.0-only/);
  assert.match(out.join("\n"), /NATIVE_LICENSE_FILE: .*licence file changed: re-review and update the hash/);
  assert.match(out.join("\n"), /NATIVE_NOTICE: .*does not name v9\.9\.9/);
});

test("a build-dependency on cmake, nasm-rs, bindgen or pkg-config counts as native; dev edges and dev-only members do not", () => {
  const doc = fakeMetadata(["app", "fin-testfonts"], [
    ["app", { deps: [["a"], ["b"], ["devonly", "dev"]] }],
    ["a", { deps: [["cmake", "build"]] }],
    ["b", { deps: [["pkg-config", "build"]] }],
    ["devonly", { links: "dev" }],
    ["fin-testfonts", { deps: [["tf-sys"]] }],
    ["tf-sys", { links: "tf" }],
    ["cmake", {}],
    ["pkg-config", {}],
  ]);
  const names = shipped([doc]).filter((p) => p.native).map((p) => p.name).sort();
  assert.deepEqual(names, ["a", "b"]);
});

test("a listed, allowed, hashed and noticed library passes; a submodule must be at its commit", () => {
  const doc = fakeMetadata(["app"], [["app", { deps: [["foo-sys"]] }], ["foo-sys", { links: "foo" }]]);
  const hash = (p) => (p === "third_party/foo/LICENSE" ? blake3(Buffer.from(LICENSE)) : null);
  const ok = check({ pkgs: shipped([doc]), native: baseNative(), notices: NOTICES, gitlink: () => null, hash });
  assert.deepEqual(ok, []);
  const sub = baseNative({ path: "third_party/foo", commit: "a".repeat(40), files: { LICENSE: blake3(Buffer.from(LICENSE)) } });
  const at = (sha) => check({ pkgs: shipped([doc]), native: sub, notices: NOTICES, gitlink: () => sha, hash });
  assert.deepEqual(at("a".repeat(40)), []);
  assert.match(at("b".repeat(40)).join("\n"), /NATIVE_SUBMODULE: .* is at b+, native\.json says a+/);
  assert.match(at(null).join("\n"), /NATIVE_SUBMODULE: .* is not a git submodule/);
});

test("an MPL-2.0 package must be named with its version in the MPL-2.0 §3.2 section", () => {
  const doc = fakeMetadata(["app"], [["app", { deps: [["symphonia"]] }], ["symphonia", { license: "MPL-2.0" }]]);
  const run = (notices) => check({ pkgs: shipped([doc]), native: { libraries: [], allow: ALLOW }, notices, gitlink: () => null, hash: () => null });
  assert.match(run("## Other\n").join("\n"), /MPL_NOTICE: symphonia 1\.0\.0/);
  assert.deepEqual(run("## MPL-2.0 §3.2 — source\n- symphonia 1.0.0\n"), []);
});

test("SPDX expressions: OR takes either side, AND needs both, WITH counts as its licence", () => {
  assert.equal(spdxAllowed("MIT OR GPL-2.0-only", ALLOW), true);
  assert.equal(spdxAllowed("MIT AND GPL-2.0-only", ALLOW), false);
  assert.equal(spdxAllowed("(Apache-2.0 AND ISC) OR LGPL-3.0", ALLOW), true);
  assert.equal(spdxAllowed("Apache-2.0 WITH LLVM-exception", ALLOW), true);
  assert.equal(spdxAllowed("MIT/Apache-2.0", ALLOW), true);
  assert.equal(spdxAllowed("", ALLOW), false);
});

test("the gate script runs on a fake metadata document", () => {
  const LIC = "licence\n";
  const root = fixtureRoot({
    "third_party/native.json": JSON.stringify({ libraries: [{ name: "LibFoo", package: "foo-sys", version: "1.0", license: "MIT", files: { "LICENSE-foo": blake3(Buffer.from(LIC)) } }], allow: ALLOW }),
    "THIRD_PARTY_NOTICES.md": "## LibFoo 1.0\n",
    "LICENSE-foo": LIC,
  });
  const meta = join(root, "meta.json");
  writeFileSync(meta, JSON.stringify(fakeMetadata(["app"], [["app", { deps: [["foo-sys"], ["bar"]] }], ["foo-sys", { links: "foo" }], ["bar", {}]])));
  assert.equal(runGate("./check-native-deps.mjs", ["--root", root, "--metadata", meta]).code, 0);
  writeFileSync(join(root, "LICENSE-foo"), "changed\n");
  const r = runGate("./check-native-deps.mjs", ["--root", root, "--metadata", meta]);
  assert.equal(r.code, 1);
  assert.match(r.out, /NATIVE_LICENSE_FILE/);
});

test("more build tools, build scripts that run a compiler, and listed versions are checked", () => {
  const root = fixtureRoot({ "w/runs/build.rs": 'fn main() { std::process::Command::new("cc").arg("x.c").status().unwrap(); }\n', "w/quiet/build.rs": "fn main() {}\n" });
  const doc = fakeMetadata(["app"], [
    ["app", { deps: [["auto"], ["runs"], ["quiet"], ["foo-sys"]] }],
    ["auto", { deps: [["autotools", "build"]] }],
    ["runs", {}],
    ["quiet", {}],
    ["foo-sys", { links: "foo" }],
    ["autotools", {}],
  ]);
  for (const p of doc.packages) {
    if (p.name === "runs" || p.name === "quiet") p.targets = [{ kind: ["custom-build"], src_path: join(root, "w", p.name, "build.rs") }];
  }
  const native = shipped([doc]).filter((p) => p.native).map((p) => `${p.name}: ${p.native}`).sort();
  assert.deepEqual(native, ["auto: build-depends on autotools", "foo-sys: links = \"foo\"", "runs: its build script runs cc"]);
  const hash = () => blake3(Buffer.from(LICENSE));
  const listed = (version) => ({ libraries: [{ name: "LibFoo", package: "foo-sys", version, license: "MIT", files: {} }], allow: ALLOW });
  const run = (version) => check({ pkgs: shipped([doc]).filter((p) => p.name === "foo-sys"), native: listed(version), notices: `## LibFoo ${version}\n`, gitlink: () => null, hash });
  assert.deepEqual(run("1"), []);
  assert.deepEqual(run("1.0"), []);
  assert.match(run("2").join("\n"), /NATIVE_VERSION: LibFoo \(foo-sys\): native\.json says 2, the graph has 1\.0\.0/);
});

test("an optional dependency only a weak feature names is not shipped (cargo#10801)", () => {
  const id = (n) => `path+file:///w/${n}#${n}@1.0.0`;
  const doc = (features) => ({
    workspace_members: [id("app")],
    packages: [
      {
        id: id("app"),
        name: "app",
        version: "1.0.0",
        license: "MIT",
        links: null,
        dependencies: [{ name: "weak-sys", optional: true, rename: null }],
        features: { alloc: ["weak-sys?/alloc"], fmt: ["dep:weak-sys"] },
      },
      { id: id("weak-sys"), name: "weak-sys", version: "1.0.0", license: "MIT", links: "weak", dependencies: [] },
    ],
    resolve: {
      nodes: [
        { id: id("app"), features, deps: [{ name: "weak_sys", pkg: id("weak-sys"), dep_kinds: [{ kind: null, target: null }] }] },
        { id: id("weak-sys"), features: [], deps: [] },
      ],
    },
  });
  const names = (d) => [...shipped([d]).values()].map((p) => p.name).sort();
  assert.deepEqual(names(doc(["alloc"])), ["app"]);
  assert.deepEqual(names(doc(["alloc", "fmt"])), ["app", "weak-sys"]);
});
