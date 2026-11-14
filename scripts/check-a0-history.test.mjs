import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const FILE = "crates/fin-audio/tests/a0/expected.json";
const LIB = "crates/fin-audio/src/lib.rs";
const gate = (root) => runGate("./check-a0-history.mjs", ["--root", root]);

function git(root, ...args) {
  const r = spawnSync(
    "git",
    ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", ...args],
    { cwd: root, encoding: "utf8" },
  );
  assert.equal(r.status, 0, r.stderr);
  return r.stdout.trim();
}

function expected(audioVersion, plans) {
  return JSON.stringify({ audioVersion, chunkFrames: 480, plans }, null, 2) + "\n";
}

function put(root, path, text) {
  const full = join(root, path);
  mkdirSync(dirname(full), { recursive: true });
  writeFileSync(full, text);
}

function lib(version) {
  return `//! fin-audio\n\n/// Bumped when any hashed audio output changes.\npub const AUDIO_VERSION: u32 = ${version};\n`;
}

/// Writes expected.json and lib.rs: `v` is the JSON text (lib.rs then carries its
/// audioVersion) or `{ json, lib }` (the Rust version given apart; `json` may be omitted).
function write(root, v) {
  const { json, rust } = typeof v === "string" ? { json: v, rust: JSON.parse(v).audioVersion } : { json: v.json, rust: v.lib };
  if (json !== undefined) put(root, FILE, json);
  if (rust !== undefined) put(root, LIB, lib(rust));
}

/// A repository whose expected.json and lib.rs go through `versions` (one commit each).
function history(...versions) {
  const root = fixtureRoot({ "README.md": "x\n" });
  git(root, "init", "-q", "-b", "main");
  git(root, "add", ".");
  git(root, "commit", "-q", "-m", "init");
  versions.forEach((v, i) => {
    write(root, v);
    git(root, "add", ".");
    git(root, "commit", "-q", "-m", `c${i}`);
  });
  return root;
}

const P1 = { "01-eq": { blake3: "aa", chunks: "11" } };
const P1_MOVED = { "01-eq": { blake3: "bb", chunks: "22" } };

test("the first blessing is the baseline", () => {
  const r = gate(history(expected(0, {}), expected(1, P1)));
  assert.equal(r.code, 0, r.out);
});

test("a hash change without an audioVersion bump is a finding", () => {
  const root = history(expected(1, P1), expected(1, P1_MOVED));
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_HASH_WITHOUT_BUMP: .*01-eq.*blake3/);
  assert.match(r.out, new RegExp(git(root, "rev-parse", "--short", "HEAD")));
});

test("a hash change with a bump passes", () => {
  const r = gate(history(expected(1, P1), expected(2, P1_MOVED)));
  assert.equal(r.code, 0, r.out);
});

test("a peaks change without a bump is a finding", () => {
  const a = { "10-x": { blake3: "aa", peaks: "p1", chunks: "11" } };
  const b = { "10-x": { blake3: "aa", peaks: "p2", chunks: "11" } };
  const r = gate(history(expected(1, a), expected(1, b)));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /10-x.*peaks/);
});

test("removing a plan without a bump is a finding", () => {
  const r = gate(history(expected(1, P1), expected(1, {})));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /01-eq.*removed/);
});

test("adding a plan needs no bump", () => {
  const more = { ...P1, "11-new": { blake3: "cc", chunks: "33" } };
  const r = gate(history(expected(1, P1), expected(1, more)));
  assert.equal(r.code, 0, r.out);
});

test("a change hidden in an older commit is still found", () => {
  const r = gate(history(expected(1, P1), expected(1, P1_MOVED), expected(2, P1_MOVED)));
  assert.equal(r.code, 1, r.out);
});

test("lowering audioVersion is a finding", () => {
  const r = gate(history(expected(2, P1), expected(1, P1)));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /audioVersion/);
});

test("an uncommitted change is checked against HEAD", () => {
  const root = history(expected(1, P1));
  write(root, expected(1, P1_MOVED));
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /working tree/);
});

test("a merge is checked against each parent", () => {
  const root = history(expected(1, P1));
  git(root, "checkout", "-q", "-b", "side");
  write(root, expected(2, P1_MOVED));
  git(root, "commit", "-q", "-am", "bump");
  git(root, "checkout", "-q", "main");
  write(root, expected(2, { "01-eq": { blake3: "dd", chunks: "44" } }));
  git(root, "commit", "-q", "-am", "other bump");
  // Both sides bumped to 2 independently; the merge keeps side's hashes under version 2.
  git(root, "merge", "-q", "-s", "ours", "--no-edit", "side");
  write(root, expected(2, P1_MOVED));
  git(root, "commit", "-q", "-a", "--amend", "--no-edit");
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /01-eq/);
});

test("a shallow clone fails loudly instead of passing", () => {
  const origin = history(expected(1, P1), expected(2, P1_MOVED));
  const root = fixtureRoot({});
  const r0 = spawnSync("git", ["clone", "-q", "--depth", "1", `file://${origin}`, join(root, "c")], {
    encoding: "utf8",
  });
  assert.equal(r0.status, 0, r0.stderr);
  const r = gate(join(root, "c"));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_HISTORY_UNAVAILABLE: .*shallow/);
});

test("no git repository fails loudly", () => {
  const root = fixtureRoot({ [FILE]: expected(1, P1) });
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_HISTORY_UNAVAILABLE/);
});

test("audioVersion raised in the JSON but not in lib.rs is a finding at that commit", () => {
  // Commit A moves a hash and bumps the JSON only; commit B catches lib.rs up.
  const root = history(expected(1, P1), { json: expected(2, P1_MOVED), lib: 1 });
  const a = git(root, "rev-parse", "--short=12", "HEAD");
  write(root, { lib: 2 });
  git(root, "commit", "-q", "-am", "catch up");
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, new RegExp(`A0_VERSION_MISMATCH: commit ${a}.*audioVersion 2.*AUDIO_VERSION 1`));
});

test("a lib.rs-only bump without a re-bless is a finding", () => {
  const r = gate(history(expected(1, P1), { lib: 2 }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_VERSION_MISMATCH: commit .*audioVersion 1.*AUDIO_VERSION 2/);
});

test("the working tree's lib.rs must match its expected.json", () => {
  const root = history(expected(1, P1));
  write(root, { lib: 3 });
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_VERSION_MISMATCH: working tree/);
});

test("a lib.rs without AUDIO_VERSION is a finding", () => {
  const root = history(expected(1, P1));
  put(root, LIB, "//! no constant\n");
  git(root, "commit", "-q", "-am", "drop");
  const r = gate(root);
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /A0_VERSION_MISMATCH: .*no `pub const AUDIO_VERSION/);
});

test("a root commit holding the file is the baseline", () => {
  const root = fixtureRoot({});
  git(root, "init", "-q", "-b", "main");
  write(root, expected(1, P1));
  git(root, "add", ".");
  git(root, "commit", "-q", "-m", "root");
  write(root, expected(2, P1_MOVED));
  git(root, "add", ".");
  // Staged but uncommitted: the index must not stand in for the root commit's parent.
  write(root, expected(2, P1_MOVED));
  const r = gate(root);
  assert.equal(r.code, 0, r.out);
});
