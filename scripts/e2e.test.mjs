// Tests for the e2e script's own checks and cleanup (the full run is `pnpm e2e`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkRevs, parseFrames } from "./e2e.mjs";
import { clipsOf, failure } from "./e2e/history-lib.mjs";
import { audioSummary, blessing, checkGolden, firstDifference, hashTree, layerSummary } from "./e2e/reel-lib.mjs";
import { realCache } from "./e2e/reel-real.mjs";

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

test("clipsOf lists every track's clips as id:start+duration", () => {
  const snapshot = { project: { timelines: [{ tracks: [{ id: "t1", items: [{ id: "c1", start: "0", duration: "4" }] }, { id: "t2", items: [] }] }] } };
  assert.deepEqual(clipsOf(snapshot), { t1: ["c1:0+4"], t2: [] });
});

test("failure checks the exit status and the error code of a refused fin call", () => {
  const refused = { status: 4, json: { error: { code: "CONFLICT" } }, text: "{}" };
  assert.equal(failure(refused, "CONFLICT", 4, "merge").code, "CONFLICT");
  assert.throws(() => failure(refused, "NOT_FOUND", 4, "merge"), /expected exit 4 NOT_FOUND/);
  assert.throws(() => failure({ ...refused, status: 3 }, "CONFLICT", 4, "merge"), /expected exit 4 CONFLICT/);
});

test("reel goldens are blessed only on request and never under CI", () => {
  assert.equal(blessing({}), false);
  assert.equal(blessing({ E2E_REEL_BLESS: "1" }), true);
  assert.throws(() => blessing({ E2E_REEL_BLESS: "1", CI: "true" }), /refused under CI/);
});

test("checkGolden writes when blessing, compares otherwise, and leaves the actual value on a mismatch", () => {
  const dir = mkdtempSync(join(tmpdir(), "reel-golden-"));
  try {
    assert.throws(() => checkGolden("g.json", { a: 1 }, { dir, bless: false }), /no golden g\.json/);
    checkGolden("g.json", { a: [1, 2] }, { dir, bless: true });
    checkGolden("g.json", { a: [1, 2] }, { dir, bless: false });
    assert.throws(() => checkGolden("g.json", { a: [1, 3] }, { dir, bless: false }), /\$\.a\[1\]: got 3, expected 2/);
    assert.deepEqual(JSON.parse(readFileSync(join(dir, "g.json.actual"), "utf8")), { a: [1, 3] });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("firstDifference names the first differing path", () => {
  assert.equal(firstDifference({ a: 1 }, { a: 1 }), undefined);
  assert.equal(firstDifference({ a: { b: "x" } }, { a: { b: "y" } }), '$.a.b: got "x", expected "y"');
  assert.equal(firstDifference({ a: 1, c: 2 }, { a: 1 }), "$.c: got 2, expected undefined");
});

test("hashTree hashes the files a filter keeps, by forward-slash relative path", () => {
  const dir = mkdtempSync(join(tmpdir(), "reel-tree-"));
  try {
    writeFileSync(join(dir, "a.txt"), "a");
    writeFileSync(join(dir, "b.txt"), "b");
    const tree = hashTree(dir, (rel) => rel === "a.txt");
    assert.deepEqual(Object.keys(tree), ["a.txt"]);
    assert.equal(tree["a.txt"], "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("plan summaries name layers, transitions, captions and gain shapes", () => {
  const plan = {
    layers: [
      { item: "c1", kind: "clip", track: "t1", opacity: 1, source: { time: "49/5" }, transition: { id: "tr1", role: "out", progress: 0.3 } },
      { item: "cap1", kind: "caption", track: "t4", opacity: 0.5, caption: { layout: { lines: [{}, {}], faces: ["x"], width: 64, height: 128 } } },
      { item: "cap2", kind: "caption", track: "t4", opacity: 1, caption: { layout: null, unmeasured: { reason: "fontMissing" } } },
    ],
  };
  assert.deepEqual(layerSummary(plan), [
    "c1 clip t1 src 49/5 tr1 out 0.3",
    "cap1 caption t4 opacity 0.5 2 lines, 1 faces, 64x128/64 px",
    "cap2 caption t4 unmeasured fontMissing",
  ]);
  const audio = { items: [{ item: "c7", track: "t3", from: "0", to: "30", speed: "1", gain: [{ from: "0", db: [-6, -6], shapes: [{ kind: "fadeIn" }] }, { from: "2", db: [-6, -6] }] }] };
  assert.deepEqual(audioSummary(audio), ["c7 t3 0..30 x1 -6dB fadeIn@0"]);
});

test("the real-font pass skips without a cache, unless FIN_REQUIRE_FONTS=1", () => {
  assert.equal(realCache({}), undefined);
  assert.throws(() => realCache({ FIN_REQUIRE_FONTS: "1" }), /FIN_FONT_CACHE is not set/);
  const missing = join(tmpdir(), "no-such-font-cache-dir");
  assert.equal(existsSync(missing), false);
  assert.throws(() => realCache({ FIN_FONT_CACHE: missing, FIN_REQUIRE_FONTS: "1" }), /does not exist/);
  assert.equal(realCache({ FIN_FONT_CACHE: tmpdir() }), tmpdir());
});
