// Self-test of the 4C checks (analyse-edit4c.mjs) on synthetic rounds: a clean round of each 4C
// row has no findings, and each planted fault is found.
import assert from "node:assert/strict";
import { test } from "node:test";
import { analyseRound } from "./analyse.mjs";
import { diffPaths, isMeasurementPath } from "./analyse-edit4c.mjs";

const S = 1e9;
function proc(label, role, server, host, evs, { ok = true, error } = {}) {
  const events = [{ ev: "start", t: 0 }, ...evs, { ev: "end", t: 200, ok, error }].map((e) => ({ label, server, mono: e.t * S, wall: e.t * S, ...e }));
  return { id: `${label}:${role}`, label, role, host, server, events };
}
const commit = (t, txId, seq, extra = {}) => ({ ev: "commit", t, ok: true, txId, branch: "main", seq, txHash: `h-${txId}`, rowEpoch: extra.epoch ?? 1, lease: "held", epoch: extra.epoch ?? 1, ids: [], kind: "edit", ...extra });
const row = (seq, txId, epoch = 1, author = "x") => [seq, txId, `h-${txId}`, epoch, true, author, "main", "edit"];
const H = { stateHash: "blake3:s", indexHash: "blake3:i", heads: { main: [3, "h-u1"] } };
const verifier = (label, role, server, host, rows, extra = []) =>
  proc(label, role, server, host, [{ ev: "verify", t: 100, ok: true, checks: [] }, { ev: "chain", t: 100, rows, all: rows, quarantine: [] }, { ev: "hashes", t: 100, ...H }, { ev: "folder", t: 100, lease: { claims: [] }, scan: { segments: [] } }, ...extra]);

/** A 4B brancher, then a 4C upgrade on its server data, and a fresh 4C verify. */
function upgrade() {
  const heads = { main: [2, "h-m2"], br1: [2, "h-e1"] };
  const a = proc("a", "brancher", "A", "h1", [{ ev: "lease", t: 1, lease: "held", epoch: 1 }, commit(2, "m1", 1), commit(3, "m2", 2), { ev: "final", t: 9, txStatus: {}, hashes: { heads } }]);
  const u = proc("u", "upgrade", "A", "h1", [
    { ev: "upgraded", t: 20, hashes: { heads: structuredClone(heads) }, readOnly: null, projectId: "P", dbPath: "/d/P/project.fdb", dbVersionsBefore: { P: 2 }, dbVersionsAfter: { P: 3 } },
    { ev: "upgradeVerify", t: 20, ok: true, checks: [] },
    { ev: "lease", t: 20, lease: "held", epoch: 2, previous: { record: "A" } },
    commit(21, "u1", 3, { kind: "undo", undone: ["m2"], epoch: 2 }),
    { ev: "undone", t: 21, hashes: H },
    { ev: "final", t: 22, txStatus: {} },
  ]);
  const v = verifier("verify", "verify", "V", "h1", [row(1, "m1"), row(2, "m2"), row(3, "u1", 2)]);
  return [a, u, v];
}

test("a clean upgrade4c round has no findings", () => {
  const r = analyseRound(upgrade());
  assert.deepEqual(r.problems, []);
  assert.equal(r.critical, false);
  assert.deepEqual([r.counts.upgradeMigrated, r.counts.upgradeHeadsSame, r.counts.upgradeVerifyOk, r.counts.upgradeUndone, r.counts.upgradeRebuildSame], [1, 1, 1, 1, 1]);
});

test("upgrade4c: a moved head, a failed verify, no undo, a different rebuild or no migration are critical", () => {
  const moved = upgrade();
  moved[1].events.find((e) => e.ev === "upgraded").hashes.heads.main = [1, "h-m1"];
  assert.equal(analyseRound(moved).counts.upgradeHeadsSame, 0);
  const bad = upgrade();
  bad[1].events.find((e) => e.ev === "upgradeVerify").ok = false;
  assert.equal(analyseRound(bad).counts.upgradeVerifyOk, 0);
  const noUndo = upgrade();
  noUndo[1].events = noUndo[1].events.filter((e) => e.kind !== "undo");
  assert.equal(analyseRound(noUndo).counts.upgradeUndone, 0);
  const rebuilt = upgrade();
  rebuilt[2].events.find((e) => e.ev === "hashes").stateHash = "blake3:other";
  const r = analyseRound(rebuilt);
  assert.equal(r.counts.upgradeRebuildSame, 0);
  assert.equal(r.critical, true);
  const fresh = upgrade();
  fresh[1].events.find((e) => e.ev === "upgraded").dbVersionsBefore = {};
  assert.equal(analyseRound(fresh).counts.upgradeMigrated, 0);
});

/** 4B holder a (X), 4C holder b (Y), 4B follower c (Y), 4C follower d (X), 4C verifiers. */
function mixed({ applied = ["a1", "a2"], ro = "NEWER_FORMAT", dHash = H } = {}) {
  const a = proc("a", "holder-4b", "A", "X", [{ ev: "lease", t: 1, lease: "held", epoch: 1 }, commit(2, "a1", 1), commit(3, "a2", 2), { ev: "final", t: 5, txStatus: {} }]);
  const b = proc("b", "holder-4c", "B", "Y", [{ ev: "lease", t: 60, lease: "held", epoch: 2, previous: { record: "A" } }, { ev: "writable", t: 70, waitMs: 10_000 }, commit(71, "b1", 3, { epoch: 2 }), { ev: "final", t: 95, txStatus: {} }]);
  const c = proc("o", "follower-4b", "C", "Y", [{ ev: "opened", t: 2 }, ...applied.map((x, i) => ({ ev: "applied", t: 3 + i, txId: x, seq: i + 1, txHash: `h-${x}` })), ...(ro ? [{ ev: "readOnly", t: 73, reason: ro }] : []), { ev: "final", t: 120, readOnly: ro }]);
  const d = proc("f", "follower-4c", "D", "X", [{ ev: "opened", t: 2 }, { ev: "followed", t: 100, reached: true, hashes: dHash }]);
  const rows = [row(1, "a1"), row(2, "a2"), row(3, "b1", 2)];
  return [a, b, c, d, verifier("verify", "verify", "V", "Y", rows), verifier("verify-d", "verify-other", "W", "X", rows)];
}

test("a clean mixed4c round has no findings", () => {
  const r = analyseRound(mixed());
  assert.deepEqual(r.problems, []);
  assert.equal(r.critical, false);
  assert.deepEqual([r.counts.mixed4cFollowed4b, r.counts.mixed4cAccepted4b, r.counts.mixed4cApplied4c, r.counts.mixed4cNewerFormat, r.counts.mixed4cHashMismatches], [2, 2, 0, 1, 0]);
});

test("mixed4c: a missed 4B record, an applied 4C record, no NEWER_FORMAT or a different 4C view are critical", () => {
  assert.equal(analyseRound(mixed({ applied: ["a1"] })).counts.mixed4cMissed, 1);
  assert.equal(analyseRound(mixed({ applied: ["a1", "a2", "b1"] })).counts.mixed4cApplied4c, 1);
  assert.equal(analyseRound(mixed({ ro: null })).counts.mixed4cNewerFormat, 0);
  const r = analyseRound(mixed({ dHash: { ...H, indexHash: "blake3:other" } }));
  assert.equal(r.counts.mixed4cHashMismatches, 2);
  assert.equal(r.critical, true);
});

/** A 4C holder, then a 4B follower and a 4B verify of its project. */
function newer({ follower = "readOnly", wrote = false } = {}) {
  const a = proc("a", "holder-4c", "A", "X", [{ ev: "lease", t: 1, lease: "held", epoch: 1 }, commit(2, "a1", 1), { ev: "final", t: 5, txStatus: {} }]);
  const f =
    follower === "refused"
      ? proc("o", "follower-4b", "O", "Y", [], { ok: false, error: "open: SCHEMA_TOO_NEW: needs a newer Finisher" })
      : proc("o", "follower-4b", "O", "Y", [{ ev: "opened", t: 10, readOnly: follower === "readOnly" ? "NEWER_FORMAT" : null }, { ev: "final", t: 20, readOnly: follower === "readOnly" ? "NEWER_FORMAT" : null }]);
  const segments = [{ name: "s1", branch: "main", epoch: 1, writer: "A", records: [[1, "a1", "h-a1"]] }, ...(wrote ? [{ name: "s2", branch: "main", epoch: 2, writer: "O", records: [] }] : [])];
  const v = proc("verify", "verify", "V", "X", [{ ev: "verify", t: 100, ok: true, checks: [] }, { ev: "chain", t: 100, rows: [row(1, "a1")], all: [row(1, "a1")], quarantine: [] }, { ev: "folder", t: 100, lease: { record: { serverId: "A" }, claims: [], probes: [{ server: "O" }] }, scan: { segments } }]);
  const v4b = proc("verify-b", "verify-4b", "W", "Y", [{ ev: "opened", t: 110 }, { ev: "status", t: 111, status: { readOnly: { reason: "NEWER_FORMAT", fix: "update Finisher" } } }]);
  return [a, f, v, v4b];
}

test("a clean newer4c round has no findings, read-only or refused", () => {
  const r = analyseRound(newer());
  assert.deepEqual(r.problems, []);
  assert.deepEqual([r.counts.newer4cReadOnly, r.counts.newer4cRefused, r.counts.newer4cWrote4b], [2, 0, 0]);
  const refused = analyseRound(newer({ follower: "refused" }));
  assert.deepEqual(refused.problems, [], "a refusal is a recorded outcome");
  assert.equal(refused.counts.newer4cRefused, 1);
  assert.equal(refused.critical, false);
});

test("newer4c: a writable 4B open or a 4B segment is critical", () => {
  const w = analyseRound(newer({ follower: "writable" }));
  assert.equal(w.counts.newer4cWritable, 1);
  assert.equal(w.critical, true);
  const s = analyseRound(newer({ wrote: true }));
  assert.equal(s.counts.newer4cWrote4b, 1);
  assert.equal(s.critical, true);
});

const capPlan = (layout, unmeasured, engine) => ({
  timeline: "tl1",
  layers: [{ item: "c1", kind: "caption", opacity: 1, caption: { words: [{ text: "Hello" }], style: "preset:bold", ...(layout ? { layout } : { unmeasured }) } }],
  ...(engine ? { textEngine: engine } : {}),
});
const LAY = { lines: [{}], faces: ["Inter|700|normal"], width: 100, height: 80, words: [{}], missing: [] };
const frame = (t, plan, sha, shape = "shape") => ({ t, sha256: sha, shapeSha256: shape, textEngine: plan.textEngine ?? null, captions: plan.caption ?? plan.layers.map((l) => (l.caption.layout ? { item: l.item, lines: 1, missing: [] } : { item: l.item, unmeasured: l.caption.unmeasured.reason })), plan });
const TIMES = ["1/5", "17/10", "16/5", "67/10"];

/** Editor a, fetch follower f, offline follower o, verifiers. */
function edit({ fetched = 4, offline = "fontMissing", fetchSha = "m", offShape = "shape" } = {}) {
  const measured = capPlan(LAY, null, "fin-text/0.0.0 rev/2 x");
  const unmeasured = capPlan(null, { reason: offline, keys: ["Inter|700|normal"] });
  const pins = { "Inter|700|normal": "i", "Noto Sans Arabic|400|normal": "ar", "Noto Sans CJK|400|normal": "cjk", "Noto Color Emoji|400|normal": "em" };
  const a = proc("a", "editor", "A", "X", [{ ev: "lease", t: 1, lease: "held", epoch: 1 }, commit(2, "e1", 1), { ev: "fonts", t: 3, fetch: { seeded: 4, downloaded: 0, cached: 0, failed: 0, files: [] } }, { ev: "plans4c", t: 10, hashes: H, pins, plans: { frames: TIMES.map((t) => frame(t, measured, "m")) } }, { ev: "final", t: 12, txStatus: {} }]);
  const f = proc("f", "follower-fetch", "F", "Y", [
    { ev: "fontCache", t: 1, cleared: 4 },
    { ev: "followed", t: 20, reached: true, hashes: H },
    { ev: "fonts", t: 25, fetch: { downloaded: fetched, failed: 4 - fetched, cached: 0, bytes: 31e6, files: Array.from({ length: 4 }, (_, i) => ({ how: i < fetched ? "downloaded" : "failed" })) } },
    { ev: "plans4c", t: 26, hashes: H, plans: { frames: TIMES.map((t) => frame(t, measured, fetchSha)) } },
  ]);
  const o = proc("o", "follower-offline", "O", "Y", [{ ev: "followed", t: 20, reached: true, hashes: H }, { ev: "plans4c", t: 21, hashes: H, plans: { frames: TIMES.map((t) => frame(t, unmeasured, "u", offShape)) } }]);
  return [a, f, o, verifier("verify", "verify", "V", "X", [row(1, "e1")]), verifier("verify-b", "verify-other", "W", "Y", [row(1, "e1")])];
}

test("a clean edit4c round has no findings", () => {
  const r = analyseRound(edit());
  assert.deepEqual(r.problems, []);
  assert.equal(r.critical, false);
  assert.deepEqual([r.counts.edit4cViews, r.counts.edit4cHashMismatches, r.counts.edit4cFrames, r.counts.edit4cPlanMismatches, r.counts.edit4cOfflineUnmeasured, r.counts.edit4cOfflineOtherDiffs, r.counts.edit4cDownloaded], [4, 0, 4, 0, 4, 0, 4]);
});

test("edit4c: a fetch follower's other plan, a short fetch, a measured offline caption or other offline differences are critical", () => {
  assert.equal(analyseRound(edit({ fetchSha: "x" })).counts.edit4cPlanMismatches, 4);
  const short = analyseRound(edit({ fetched: 3 }));
  assert.equal(short.counts.edit4cDownloaded, 3);
  assert.equal(short.critical, true);
  assert.equal(analyseRound(edit({ offline: "fontUnpinned" })).counts.edit4cOfflineMeasured, 4);
  const other = analyseRound(edit({ offShape: "other" }));
  assert.equal(other.counts.edit4cOfflineOtherDiffs, 4);
  assert.equal(other.critical, true);
});

test("edit4c: a follower view that differs is a mismatch", () => {
  const p = edit();
  p[2].events.find((e) => e.ev === "followed").hashes = { ...H, stateHash: "blake3:other" };
  const r = analyseRound(p);
  assert.equal(r.counts.edit4cHashMismatches, 1);
  assert.equal(r.critical, true);
});

test("diffPaths names the differing paths, and only caption measurement counts as measurement", () => {
  const a = capPlan(LAY, null, "e");
  const b = capPlan(null, { reason: "fontMissing", keys: [] });
  const paths = diffPaths(b, a).sort();
  assert.deepEqual(paths, ["$.layers[0].caption.layout", "$.layers[0].caption.unmeasured", "$.textEngine"]);
  assert.ok(paths.every(isMeasurementPath));
  const c = structuredClone(b);
  c.layers[0].opacity = 0.5;
  assert.deepEqual(diffPaths(c, b), ["$.layers[0].opacity"]);
  assert.equal(isMeasurementPath("$.layers[0].caption.words[0].text"), false);
  assert.equal(isMeasurementPath("$.layers[1].children[0].caption.layout.width"), true);
});
