import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const SCHEMA = join(ROOT, "packages/core-types/schema/fin-core.schema.json");
const gate = (env = {}) => {
  const r = spawnSync(process.execPath, [join(ROOT, "scripts/check-envelopes.mjs"), "--root", ROOT], { encoding: "utf8", env: { ...process.env, ...env } });
  return { code: r.status, out: r.stdout + r.stderr };
};

test("the committed envelope schemas match the generated types", () => {
  const r = gate();
  assert.equal(r.code, 0, r.out);
});

test("a Rust type that drifted from its envelope schema fails the gate", () => {
  const tmp = mkdtempSync(join(tmpdir(), "fin-envelopes-"));
  try {
    const schema = JSON.parse(readFileSync(SCHEMA, "utf8"));
    delete schema.$defs.Generation.properties.n; // fin-store's Generation loses a field
    schema.$defs.Generation.required = schema.$defs.Generation.required.filter((k) => k !== "n");
    const drifted = join(tmp, "drifted.schema.json");
    writeFileSync(drifted, JSON.stringify(schema));
    const r = gate({ FIN_CORE_SCHEMA: drifted });
    assert.equal(r.code, 1);
    assert.match(r.out, /ENVELOPE_DRIFT/);
    assert.match(r.out, /GenerationSchema is fin-store's Generation/);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});
