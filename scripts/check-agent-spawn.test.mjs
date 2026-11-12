import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-agent-spawn.mjs", ["--root", root]);
const repo = fileURLToPath(new URL("..", import.meta.url));

const FORBIDDEN = {
  named: 'import { spawn } from "node:child_process";\n',
  bare: 'import cp from "child_process";\n',
  typeOnly: 'import type { ChildProcess } from "node:child_process";\n',
  dynamic: 'const cp = await import("node:child_process");\n',
  require: 'const cp = require("child_process");\n',
  cluster: 'import cluster from "node:cluster";\n',
  workers: 'import { Worker } from "node:worker_threads";\n',
  sideEffect: 'import "node:child_process";\n',
  bun: "Bun.spawn(['sh']);\n",
  binding: 'process.binding("spawn_sync");\n',
  createRequire: 'const r = createRequire(import.meta.url);\n',
};

for (const [name, text] of Object.entries(FORBIDDEN)) {
  test(`flags ${name} under agent/`, () => {
    const r = gate(fixtureRoot({ "apps/finisherd/src/agent/cli/x.ts": text }));
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /AGENT_SPAWN apps\/finisherd\/src\/agent\/cli\/x\.ts:1: /);
  });
}

test("allows the sandbox, the built-in fin tool and its command map, comments and other folders", () => {
  const text = FORBIDDEN.named;
  const r = gate(
    fixtureRoot({
      "apps/finisherd/src/agent/sandbox.ts": text,
      "apps/finisherd/src/agent/builtin/fin-tool.ts": text,
      "apps/finisherd/src/agent/builtin/environment.ts": text,
      "apps/finisherd/src/agent/doc.ts": '// import { spawn } from "node:child_process";\n',
      "apps/finisherd/src/jobs/x.ts": text,
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("the repository passes", () => {
  const r = gate(repo);
  assert.equal(r.code, 0, r.out);
});

test("the Claude Code adapter's folder starts nothing (plan 7D Task 3b)", async () => {
  // A fixture: `agent/claude/` is checked like the rest of `agent/`.
  const r = gate(fixtureRoot({ "apps/finisherd/src/agent/claude/launch.ts": FORBIDDEN.named }));
  assert.equal(r.code, 1, r.out);
  assert.match(r.out, /AGENT_SPAWN apps\/finisherd\/src\/agent\/claude\/launch\.ts:1: /);
  // This repository's folder (whatever is there yet, nothing at all included): no file is
  // exempt, none names `child_process`.
  const { checked, findings } = await import("./check-agent-spawn.mjs");
  const dir = join(repo, "apps/finisherd/src/agent/claude");
  const files = existsSync(dir) ? readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.(?:[cm]?[jt]s|tsx)$/.test(f)) : [];
  for (const f of files) {
    const path = `apps/finisherd/src/agent/claude/${f.split(sep).join("/")}`;
    assert.equal(checked(path), true, path);
    const text = readFileSync(join(dir, f), "utf8");
    assert.deepEqual(findings(path, text), [], path);
    assert.doesNotMatch(text, /child_process/, path);
  }
});
