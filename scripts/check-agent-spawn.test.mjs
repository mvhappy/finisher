import { test } from "node:test";
import assert from "node:assert/strict";
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
