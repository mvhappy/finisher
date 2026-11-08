import { test } from "node:test";
import assert from "node:assert/strict";
import { check, externalShipped, spdxIds } from "./check-notices.mjs";

/** A fake `cargo metadata` document: `pkgs` are [name, {license, deps: [[name, kind]]}]. */
function fakeMetadata(members, pkgs) {
  const id = (n) => `registry+x#${n}@1.0.0`;
  return {
    workspace_members: members.map(id),
    packages: pkgs.map(([name, p]) => ({ id: id(name), name, version: "1.0.0", license: p.license ?? "MIT", dependencies: [] })),
    resolve: {
      nodes: pkgs.map(([name, p]) => ({
        id: id(name),
        deps: (p.deps ?? []).map(([d, kind]) => ({ name: d, pkg: id(d), dep_kinds: [{ kind: kind ?? null, target: null }] })),
      })),
    },
  };
}

test("external shipped packages leave out members, dev-only members and dev edges", () => {
  const doc = fakeMetadata(["app", "fin-testfonts"], [
    ["app", { deps: [["lib"], ["testlib", "dev"]] }],
    ["fin-testfonts", { deps: [["fontgen"]] }],
    ["lib", { license: "MIT OR Apache-2.0" }],
    ["testlib", {}],
    ["fontgen", {}],
  ]);
  assert.deepEqual(externalShipped([doc]).map((p) => p.name), ["lib"]);
});

test("a shipped crate missing from the notices is reported once, even in two versions", () => {
  const pkgs = [
    { name: "tough", version: "0.24.0", license: "MIT OR Apache-2.0" },
    { name: "tough", version: "0.25.0", license: "MIT OR Apache-2.0" },
    { name: "spdx", version: "0.13.6", license: "MIT OR Apache-2.0" },
  ];
  const out = check({ pkgs, notices: "- `spdx` 0.13 (MIT OR Apache-2.0)\n" });
  assert.equal(out.length, 1, out.join("\n"));
  assert.match(out[0], /^NOTICE_CRATE: tough 0\.24\.0 /);
});

test("a name in prose or a longer name does not count; backticks do", () => {
  const pkgs = [{ name: "num", version: "0.4.3", license: "MIT" }];
  assert.equal(check({ pkgs, notices: "MIT. A num of crates, `num-iter`.\n" }).filter((l) => l.startsWith("NOTICE_CRATE")).length, 1);
  assert.deepEqual(check({ pkgs, notices: "MIT. `num` 0.4\n" }), []);
});

test("every licence id in use must be named", () => {
  const pkgs = [{ name: "icu", version: "2.0.0", license: "Unicode-3.0" }, { name: "j", version: "1.0.0", license: "Unlicense OR MIT" }];
  const out = check({ pkgs, notices: "`icu` `j` MIT\n" });
  assert.deepEqual(out, [
    "NOTICE_LICENSE: shipped crates use Unicode-3.0 but THIRD_PARTY_NOTICES.md never names it",
    "NOTICE_LICENSE: shipped crates use Unlicense but THIRD_PARTY_NOTICES.md never names it",
  ]);
});

test("spdxIds reads OR, AND, WITH, slashes and parentheses", () => {
  assert.deepEqual(spdxIds("(MIT OR Apache-2.0) AND Unicode-3.0"), ["MIT", "Apache-2.0", "Unicode-3.0"]);
  assert.deepEqual(spdxIds("Apache-2.0 WITH LLVM-exception"), ["Apache-2.0"]);
  assert.deepEqual(spdxIds("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
  assert.deepEqual(spdxIds(""), []);
});
