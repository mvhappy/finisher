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

const finCrate = (main, features = "") =>
  fixtureRoot({
    "Cargo.toml": '[workspace]\nresolver = "3"\nmembers = ["crates/fin"]\n',
    "crates/fin/Cargo.toml": `[package]\nname = "fin"\nversion = "0.0.0"\nedition = "2024"\n\n[features]\n${features}test-hooks = []\n`,
    "crates/fin/src/main.rs": main,
  });

test("check_test_hooks_flags_fin", () => {
  const guarded = 'fn main() { let _ = test_hook("FIN_OFFLINE_SNAPSHOT"); }\nfn test_hook(_: &str) {}\n';
  assert.equal(gate(finCrate(guarded)).code, 0);
  // A direct read of a hook variable.
  const direct = gate(finCrate('fn main() { let _ = std::env::var_os(SNAPSHOT_ENV); }\nconst SNAPSHOT_ENV: &str = "FIN_OFFLINE_SNAPSHOT";\n'));
  assert.equal(direct.code, 1);
  assert.match(direct.out, /TEST_HOOK_UNGUARDED: crates\/fin\/src\/main.rs:1/);
  assert.match(gate(finCrate('fn main() { std::env::var("FIN_TEST_COMMIT_GATE").ok(); }\n')).out, /TEST_HOOK_UNGUARDED/);
  // Any FIN_TEST_* name, by literal or through its constant (another file's).
  const panic = gate(
    fixtureRoot({
      "Cargo.toml": '[workspace]\nresolver = "3"\nmembers = ["crates/fin"]\n',
      "crates/fin/Cargo.toml": '[package]\nname = "fin"\nversion = "0.0.0"\nedition = "2024"\n',
      "crates/fin/src/main.rs": 'mod diag;\nfn main() { let _ = std::env::var_os(diag::PANIC_ENV); let _ = std::env::var(DEADLINE_ENV); }\nconst DEADLINE_ENV: &str = "FIN_DEADLINE_S";\n',
      "crates/fin/src/diag.rs": 'pub const PANIC_ENV: &str = "FIN_TEST_PANIC";\npub fn f() { std::env::var_os("FIN_TEST_ANYTHING"); }\n',
    }),
  );
  assert.equal(panic.code, 1);
  assert.match(panic.out, /TEST_HOOK_UNGUARDED: crates\/fin\/src\/diag.rs:2/);
  assert.match(panic.out, /TEST_HOOK_UNGUARDED: crates\/fin\/src\/main.rs:2/);
  assert.doesNotMatch(panic.out, /main.rs:3/);
  // fin's feature on in a normal build.
  const on = gate(finCrate("fn main() {}\n", 'default = ["test-hooks"]\n'));
  assert.equal(on.code, 1);
  assert.match(on.out, /TEST_HOOKS_IN_BUILD: crates\/fin\/Cargo.toml enables "test-hooks" by default/);
});
