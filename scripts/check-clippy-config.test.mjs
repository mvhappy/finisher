import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { DISALLOWED, allowsLint } from "./check-clippy-config.mjs";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-clippy-config.mjs", ["--root", root]);
const toml = DISALLOWED.map((p) => `    { path = "${p}", reason = "test" },`).join("\n");
const CLIPPY = `disallowed-methods = [\n${toml}\n]\n`;
const ALLOW = "#[allow(clippy::disallowed_methods)]\nfn f() {}\n";
const ok = (extra = {}) => ({
  "crates/fin-library/Cargo.toml": "[package]\n",
  "crates/fin-library/clippy.toml": CLIPPY,
  "crates/fin-library/src/lib.rs": "pub fn f() {}\n",
  "scripts/clippy-allow.json": "[]",
  ...extra,
});

test("a tree with configs and no allows passes", () => {
  const r = gate(fixtureRoot(ok()));
  assert.equal(r.code, 0, r.out);
});

test("listed_crate_without_config_fails", () => {
  const r = gate(fixtureRoot(ok({ "crates/fin-workers/Cargo.toml": "[package]\n" })));
  assert.equal(r.code, 1);
  assert.match(r.out, /CLIPPY_CONFIG: crates\/fin-workers has no clippy\.toml/);
});

test("a config that lacks a method fails", () => {
  const short = CLIPPY.replace(/.*std::fs::read_dir".*\n/, "");
  const r = gate(fixtureRoot(ok({ "crates/fin-library/clippy.toml": short })));
  assert.equal(r.code, 1);
  assert.match(r.out, /does not disallow std::fs::read_dir/);
});

test("a crate that does not exist yet is not required to have a config", () => {
  const r = gate(fixtureRoot(ok()));
  assert.equal(r.code, 0, r.out);
  assert.doesNotMatch(r.out, /fin-analyze/);
});

test("unlisted_allow_fails", () => {
  const r = gate(fixtureRoot(ok({ "crates/fin-node/src/x.rs": ALLOW })));
  assert.equal(r.code, 1);
  assert.match(r.out, /CLIPPY_ALLOW_UNLISTED: crates\/fin-node\/src\/x\.rs/);
});

test("a listed allow passes, by file and by directory", () => {
  const list = JSON.stringify([
    { path: "crates/fin-node/src/x.rs", reason: "a reason" },
    { path: "crates/fin-node/tests", reason: "tests" },
  ]);
  const r = gate(
    fixtureRoot(ok({ "crates/fin-node/src/x.rs": ALLOW, "crates/fin-node/tests/t.rs": `#![allow(unused, clippy::disallowed_methods)]\n`, "scripts/clippy-allow.json": list })),
  );
  assert.equal(r.code, 0, r.out);
});

test("an entry that allows nothing is stale", () => {
  const list = JSON.stringify([{ path: "crates/fin-node/src/gone.rs", reason: "was needed" }]);
  const r = gate(fixtureRoot(ok({ "scripts/clippy-allow.json": list })));
  assert.equal(r.code, 1);
  assert.match(r.out, /CLIPPY_ALLOW_STALE: .*gone\.rs/);
});

test("an entry without a reason fails", () => {
  const list = JSON.stringify([{ path: "crates/fin-node/src/x.rs", reason: " " }]);
  const r = gate(fixtureRoot(ok({ "crates/fin-node/src/x.rs": ALLOW, "scripts/clippy-allow.json": list })));
  assert.equal(r.code, 1);
  assert.match(r.out, /without a path and a reason/);
});

test("allows in comments and other lints do not count", () => {
  assert.equal(allowsLint("// #[allow(clippy::disallowed_methods)]\n"), false);
  assert.equal(allowsLint("#[allow(clippy::too_many_arguments)]\n"), false);
  assert.equal(allowsLint("#[allow(\n    clippy::disallowed_methods,\n)]\n"), true);
  assert.equal(allowsLint('#[expect(clippy::disallowed_methods, reason = "x")]\n'), true);
});

test("the repository passes its own gate", () => {
  const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
  const r = gate(repo);
  assert.equal(r.code, 0, r.out);
});

test("clippy_flags_each_disallowed_method", (t) => {
  // A fixture crate whose clippy.toml is the shipped list, calling every method once.
  // `dunce::canonicalize` needs a dependency, so it is only checked by name above.
  const calls = DISALLOWED.filter((p) => !p.startsWith("dunce::")).map((p, i) => {
    const path = p.replace(/^std::/, "");
    if (path === "fs::OpenOptions::open") return `    let _ = std::fs::OpenOptions::new().open(p);`;
    if (path.startsWith("path::Path::")) return `    let _ = p.${path.split("::").pop()}();`;
    return `    let _ = std::${path}(p);`;
  });
  const dir = mkdtempSync(join(tmpdir(), "fin-clippy-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "src"));
  writeFileSync(join(dir, "Cargo.toml"), '[package]\nname = "fixture"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n');
  writeFileSync(join(dir, "clippy.toml"), CLIPPY);
  writeFileSync(join(dir, "src/lib.rs"), `pub fn all(p: &std::path::Path) {\n${calls.join("\n")}\n}\n`);
  const r = spawnSync("cargo", ["clippy", "--offline", "--message-format=json"], { cwd: dir, encoding: "utf8", maxBuffer: 1 << 28 });
  const flagged = new Set();
  for (const line of r.stdout.split("\n")) {
    if (!line.startsWith("{")) continue;
    const m = JSON.parse(line);
    const text = m.message?.message ?? "";
    const hit = text.match(/use of a disallowed method `([^`]+)`/);
    if (hit) flagged.add(hit[1]);
  }
  const want = DISALLOWED.filter((p) => !p.startsWith("dunce::"));
  assert.deepEqual(want.filter((p) => !flagged.has(p)), [], `clippy: ${r.stderr.slice(-400)}`);
});
