// Tests for the e2e script's own checks and cleanup (the full run is `pnpm e2e`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkRevs, parseFrames } from "./e2e.mjs";

test("parseFrames splits one message carrying several NDJSON frames", () => {
  const text = `${JSON.stringify({ _tag: "Chunk", requestId: "1", values: [] })}\n${JSON.stringify({ _tag: "Exit", requestId: "1" })}\n`;
  assert.deepEqual(
    parseFrames(text).map((f) => f._tag),
    ["Chunk", "Exit"],
  );
  assert.deepEqual(parseFrames("\n"), []);
});

test("checkRevs accepts every committed rev exactly once, in order", () => {
  assert.equal(checkRevs([1, 2, 3], [1, 2, 3]), undefined);
});

test("checkRevs rejects a gap, a duplicate and a reordering", () => {
  assert.match(checkRevs([1, 3], [1, 2, 3]), /subscriber saw revs \[1,3\]/);
  assert.match(checkRevs([1, 2, 2, 3], [1, 2, 3]), /subscriber saw revs/);
  assert.match(checkRevs([2, 1, 3], [1, 2, 3]), /subscriber saw revs/);
});

test("checkRevs rejects non-contiguous commits from fin", () => {
  assert.match(checkRevs([1, 3], [1, 3]), /not contiguous/);
});

// The script itself, with a fake server that never writes finisherd.json and records its pid.
const SCRIPT = fileURLToPath(new URL("./e2e.mjs", import.meta.url));
const FAKE_SERVER = "require('node:fs').writeFileSync(process.argv[1], String(process.pid)); setInterval(() => {}, 1000);";

function runWithFakeServer(env) {
  const dir = mkdtempSync(join(tmpdir(), "e2e-test-"));
  const pidFile = join(dir, "pid");
  const r = spawnSync(process.execPath, [SCRIPT, "--skip-build"], {
    encoding: "utf8",
    timeout: 30_000,
    env: { ...process.env, E2E_SERVER_CMD: JSON.stringify([process.execPath, "-e", FAKE_SERVER, pidFile]), ...env },
  });
  const pid = Number(readFileSync(pidFile, "utf8"));
  rmSync(dir, { recursive: true, force: true });
  return { ...r, pid };
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

test("a server that never becomes ready is killed before the failure is reported", () => {
  const r = runWithFakeServer({ E2E_START_TIMEOUT_MS: "1000" });
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /e2e: FAIL: timed out waiting for finisherd\.json/);
  assert.equal(alive(r.pid), false, `server pid ${r.pid} still running`);
});

test("the watchdog kills the server and fails the run at the deadline", () => {
  const r = runWithFakeServer({ E2E_START_TIMEOUT_MS: "60000", E2E_DEADLINE_MS: "1500" });
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /e2e: FAIL: watchdog: the run did not finish within 1\.5s; killed finisherd/);
  assert.equal(alive(r.pid), false, `server pid ${r.pid} still running`);
});
