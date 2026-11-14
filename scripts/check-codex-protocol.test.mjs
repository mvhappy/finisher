import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { RPC, allowedMethods, findings } from "./check-codex-protocol.mjs";
import { OUT, readPinVersion } from "./gen-codex-protocol.mjs";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

/** A copy of what the gate reads, with `edit(path, text)` applied to one file. */
function copyRoot(edit) {
  const tmp = mkdtempSync(join(tmpdir(), "fin-codex-protocol-"));
  const version = readPinVersion(ROOT);
  for (const p of ["vendor/codex/pin.json", `vendor/codex/${version}/schema`, `vendor/codex/${version}/schema-experimental`, OUT, RPC]) {
    mkdirSync(dirname(join(tmp, p)), { recursive: true });
    cpSync(join(ROOT, p), join(tmp, p), { recursive: true });
  }
  if (edit) {
    const [p, f] = edit;
    writeFileSync(join(tmp, p), f(readFileSync(join(tmp, p), "utf8")));
  }
  return tmp;
}

test("the committed protocol is current and the allow-list matches", () => {
  assert.deepEqual(findings(ROOT), []);
});

test("allowedMethods reads the set literal", () => {
  assert.deepEqual(allowedMethods('export const ALLOWED_METHODS: ReadonlySet<X> = new Set<X>([\n  "a/b",\n  "c",\n]);'), ["a/b", "c"]);
  assert.equal(allowedMethods("const x = 1;"), null);
});

test("a widened allow-list fails", () => {
  const tmp = copyRoot([RPC, (t) => t.replace('"initialize",', '"initialize",\n  "account/login/start",')]);
  try {
    const f = findings(tmp);
    assert.ok(f.some((x) => x.startsWith("CODEX_PROTOCOL_METHODS") && x.includes("account/login/start")), f.join("\n"));
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test("an edited generated file fails", () => {
  const tmp = copyRoot([OUT, (t) => t.replace('openEnum(["completed", "interrupted", "failed", "inProgress"])', 'Schema.Literals(["completed", "interrupted", "failed", "inProgress"])')]);
  try {
    assert.ok(findings(tmp).some((x) => x.startsWith("CODEX_PROTOCOL_DRIFT")));
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test("a method the schema lacks fails", () => {
  const tmp = copyRoot([RPC, (t) => t.replace('"initialize",', '"initialize",\n  "thread/rollback",')]);
  try {
    assert.ok(findings(tmp).some((x) => x.includes("thread/rollback, which the pinned schema does not have")));
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});
