import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { after } from "node:test";

// Every fixture root of a test file, removed once its tests are done (the gate tests used to
// leave one `fin-gate-*` folder per fixture in the temp dir).
const roots = [];
after(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

export function fixtureRoot(files) {
  const root = mkdtempSync(join(tmpdir(), "fin-gate-"));
  roots.push(root);
  for (const [path, content] of Object.entries(files)) {
    const full = join(root, path);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, content);
  }
  return root;
}

export function lines(n) {
  return Array.from({ length: n }, (_, i) => `// line ${i + 1}`).join("\n") + "\n";
}

export function runGate(script, args) {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url)), ...args], {
    encoding: "utf8",
  });
  return { code: result.status, out: result.stdout + result.stderr };
}
