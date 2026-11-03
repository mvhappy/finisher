import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export function fixtureRoot(files) {
  const root = mkdtempSync(join(tmpdir(), "fin-gate-"));
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
