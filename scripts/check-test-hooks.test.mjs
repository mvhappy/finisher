import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const lib = { "Cargo.toml": '[package]\nname = "hooked"\nversion = "0.0.0"\nedition = "2024"\n\n[features]\ntest-hooks = []\n', "src/lib.rs": "" };
const user = (features) => ({
  "Cargo.toml": `[package]\nname = "user"\nversion = "0.0.0"\nedition = "2024"\n\n[dependencies]\nhooked = { path = "../hooked"${features} }\n`,
  "src/lib.rs": "",
});
const workspace = (features, dev = false) =>
  fixtureRoot({
    "Cargo.toml": '[workspace]\nresolver = "3"\nmembers = ["hooked", "user"]\n',
    ...Object.fromEntries(Object.entries(lib).map(([k, v]) => [`hooked/${k}`, v])),
    ...Object.fromEntries(
      Object.entries(user(dev ? "" : features)).map(([k, v]) => [
        `user/${k}`,
        dev ? `${v}\n[dev-dependencies]\nhooked = { path = "../hooked"${features} }\n` : v,
      ]),
    ),
  });
const gate = (root) => runGate("./check-test-hooks.mjs", ["--root", root]);

test("a build without test-hooks passes", () => {
  const r = gate(workspace(""));
  assert.equal(r.code, 0, r.out);
});

test("test-hooks through a dev-dependency only passes", () => {
  const r = gate(workspace(', features = ["test-hooks"]', true));
  assert.equal(r.code, 0, r.out);
});

test("test-hooks in a normal dependency fails", () => {
  const r = gate(workspace(', features = ["test-hooks"]'));
  assert.equal(r.code, 1);
  assert.match(r.out, /TEST_HOOKS_IN_BUILD/);
});

test("test-http in a normal dependency fails", () => {
  const root = workspace(', features = ["test-http"]');
  const manifest = join(root, "hooked", "Cargo.toml");
  writeFileSync(manifest, readFileSync(manifest, "utf8").replace("test-hooks = []", "test-http = []"));
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /TEST_HOOKS_IN_BUILD.*test-http/);
});
