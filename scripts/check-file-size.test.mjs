import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { symlinkSync } from "node:fs";
import { fixtureRoot, lines, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-file-size.mjs", ["--root", root]);

test("source file over 600 lines violates", () => {
  const r = gate(fixtureRoot({ "crates/a/src/big.rs": lines(601) }));
  assert.equal(r.code, 1);
  assert.match(r.out, /big\.rs: 601 lines > 600/);
});

test("test file over 1500 lines violates", () => {
  const r = gate(fixtureRoot({ "packages/a/src/big.test.ts": lines(1501) }));
  assert.equal(r.code, 1);
  assert.match(r.out, /big\.test\.ts: 1501 lines > 1500/);
});

test("files at the limit pass", () => {
  const r = gate(fixtureRoot({ "crates/a/src/ok.rs": lines(600), "src/ok.test.ts": lines(1500) }));
  assert.equal(r.code, 0, r.out);
});

test("ignored directories are skipped", () => {
  const r = gate(
    fixtureRoot({
      "target/x.rs": lines(900),
      "node_modules/p/x.ts": lines(900),
      "packages/a/src/generated/types.ts": lines(900),
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("gitignored files in a git checkout are skipped", () => {
  const root = fixtureRoot({ ".gitignore": "build/\n", "build/gen.js": lines(900), "src/ok.ts": lines(10) });
  spawnSync("git", ["init", "-q"], { cwd: root });
  spawnSync("git", ["add", "-A"], { cwd: root });
  const r = gate(root);
  assert.equal(r.code, 0, r.out);
});

test("works without a pwd binary on PATH (Windows-like environment)", () => {
  const root = fixtureRoot({ "src/ok.ts": lines(10) });
  spawnSync("git", ["init", "-q"], { cwd: root });
  const bin = fixtureRoot({});
  symlinkSync(spawnSync("which", ["git"], { encoding: "utf8" }).stdout.trim(), `${bin}/git`);
  const r = spawnSync(process.execPath, [new URL("./check-file-size.mjs", import.meta.url).pathname, "--root", root], {
    encoding: "utf8",
    env: { ...process.env, PATH: bin },
  });
  assert.equal(r.status, 0, r.stdout + r.stderr);
});
