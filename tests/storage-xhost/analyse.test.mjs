// Self-test of the analyser on synthetic rounds: every detector must fire on a planted
// fault and stay quiet on a clean round. Run: node --test tests/storage-xhost/
import assert from "node:assert/strict";
import { test } from "node:test";
import { analyseRound } from "./analyse.mjs";

const S = 1e9; // ns per second

/** A process with events at seconds `t` (mono = wall = t; offset 0 unless given). */
function proc(label, role, server, host, evs, { offsetS = 0, wallSkewS = 0 } = {}) {
  const events = [
    { ev: "start", t: 0 },
    { ev: "clock", t: 0, offsetNs: (offsetS - wallSkewS) * S, rttNs: 1e6 },
    ...evs,
    { ev: "end", t: evs.at(-1)?.t ?? 0, ok: true },
  ].map((e) => ({ label, server, ...e, mono: e.t * S, wall: (e.t + wallSkewS) * S }));
  return { id: `${label}:${role}`, label, role, host, server, events };
}
const at = (t, e) => ({ mono: t * S, wall: t * S, t, ...e });
const held = (t, epoch, extra = {}) => at(t, { ev: "lease", lease: "held", epoch, ...extra });
const commit = (t, txId, seq, txHash, epoch, ids = []) => at(t, { ev: "commit", ok: true, txId, seq, txHash, rowEpoch: epoch, lease: "held", epoch, ids });
const verify = (rows, segments = [], claims = []) =>
  proc("verify", "verify", "v", "h1", [
    { ev: "verify", t: 200, ok: true, checks: [] },
    { ev: "chain", t: 200, rows: rows.map(([seq, txId, txHash, epoch]) => [seq, txId, txHash, epoch, true, "x"]), quarantine: [] },
    { ev: "folder", t: 200, lease: { claims }, scan: { segments } },
  ]);
const seg = (epoch, writer, recs) => ({ epoch, writer, records: recs.map(([seq, txId, h]) => [seq, txId, h]) });

function clean() {
  const a = proc("a", "holder", "A", "h1", [
    held(1, 1),
    { ev: "beat", t: 6, lease: "held", epoch: 1, beatEpoch: 1, heartbeats: 1 },
    commit(2, "a1", 1, "h-a1", 1, ["a1"]),
    { ev: "lease", t: 30, lease: "lost", epoch: null },
    { ev: "final", t: 99, txStatus: {}, accepted: 1 },
  ]);
  const b = proc("b", "contender", "B", "h1", [
    { ev: "lease", t: 1, lease: "heldElsewhere", epoch: 1 },
    held(60, 2, { previous: { record: "A" } }),
    commit(61, "b1", 2, "h-b1", 2, ["a2"]),
    { ev: "final", t: 99, txStatus: {}, accepted: 1 },
  ]);
  const f = proc("f", "follower", "F", "h1", [
    { ev: "opened", t: 0.5, lease: "lost" },
    { ev: "applied", t: 4, lease: "lost", txId: "a1", seq: 1, txHash: "h-a1" },
    { ev: "applied", t: 64, lease: "lost", txId: "b1", seq: 2, txHash: "h-b1" },
  ]);
  const v = verify([[1, "a1", "h-a1", 1], [2, "b1", "h-b1", 2]], [seg(1, "A", [[1, "a1", "h-a1"]]), seg(2, "B", [[2, "b1", "h-b1"]])]);
  return [a, b, f, v];
}

test("a clean round has no findings", () => {
  const r = analyseRound(clean());
  assert.deepEqual(
    [r.counts.doubleHolders, r.counts.earlyTakeovers, r.counts.staleApplied, r.counts.idReuse, r.counts.lostAcked],
    [0, 0, 0, 0, 0],
  );
  assert.equal(r.counts.takeovers, 1);
  assert.deepEqual(r.latencyMs, [2000, 3000]);
  assert.equal(r.critical, false);
});

test("overlapping hold windows on one host are a double holder", () => {
  const [a, b, f, v] = clean();
  b.events.find((e) => e.ev === "lease" && e.lease === "held").mono = 20 * S;
  a.events.splice(5, 0, at(25, { ev: "alive", lease: "held", epoch: 1 })); // A still held at 25
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.doubleHolders, 1);
  assert.equal(r.critical, true);
});

test("two servers at one epoch are a double holder from folder evidence alone", () => {
  const [a, b, f] = clean();
  const v = verify([[1, "a1", "h-a1", 1]], [seg(1, "A", [[1, "a1", "h-a1"]]), seg(1, "B", [[2, "b1", "h-b1"]])]);
  const r = analyseRound([a, b, f, v]);
  assert.ok(r.detail.epochReuse.some((x) => x.epoch === 1));
});

test("a takeover sooner than 45 s after the old holder's last stamp is early", () => {
  const [a, b, f, v] = clean();
  b.events.find((e) => e.lease === "held").mono = 40 * S; // last beat of A at 6 s: 34 s
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.earlyTakeovers, 1);
  assert.equal(r.counts.unsafeTakeovers, 0, "34 s outlasts the 25 s unchanged-record rule");
  b.events.find((e) => e.lease === "held").mono = 20 * S;
  assert.equal(analyseRound([a, b, f, v]).counts.unsafeTakeovers, 1);
});

test("cross-host comparisons use the measured offsets and the tolerance", () => {
  // B's wall clock runs 100 s ahead; its share offset says so. Without correction the takeover
  // would look 154 s late and B's window would overlap nothing; with it, the result is as clean.
  const [a, , f, v] = clean();
  const b = proc("b", "contender", "B", "h2", [
    held(60, 2, { previous: { record: "A" } }),
    commit(61, "b1", 2, "h-b1", 2, ["a2"]),
  ], { wallSkewS: 100 });
  const r = analyseRound([a, b, f, v], { tolMs: 500 });
  assert.equal(r.detail.takeovers[0].gapMs, 54000);
  assert.equal(r.detail.takeovers[0].tolMs, 500);
});

test("an accepted commit missing from the final chain is a lost acknowledged write", () => {
  const [a, b, f] = clean();
  a.events.splice(4, 0, commit(25, "a2", 2, "h-a2", 1));
  a.events.find((e) => e.ev === "final").txStatus = { a2: { state: "setAside", reason: "DIVERGED_TAIL" } };
  const v = verify([[1, "a1", "h-a1", 1], [2, "b1", "h-b1", 2]], [seg(1, "A", [[1, "a1", "h-a1"]]), seg(2, "B", [[2, "b1", "h-b1"]])]);
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.lostAcked, 1);
  assert.equal(r.counts.lostAckedSetAside, 1);
  assert.equal(r.counts.seqCollisions, 1);
});

test("a stale record a follower applied, and one left applied, are counted", () => {
  const [a, b, f, v] = clean();
  f.events.splice(3, 0, at(70, { ev: "applied", server: "F", txId: "a9", seq: 2, txHash: "h-a9" }), at(71, { ev: "rewound", quarantined: 1 }));
  f.events.push(at(99, { ev: "chain", rows: [[1, "a1", "h-a1", 1, true], [2, "a9", "h-a9", 1, true]] }));
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.staleApplied, 1);
  assert.equal(r.counts.rewinds, 1);
  assert.equal(r.counts.staleLeftApplied, 1);
  assert.equal(r.critical, true);
});

test("an entity id bound by two commits on the chain is id reuse", () => {
  const [a, b, f, v] = clean();
  b.events.find((e) => e.ev === "commit").ids = ["a1"];
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.idReuse, 1);
});

test("a commit accepted at an older epoch after a newer epoch's commit is stale", () => {
  const [a, b, f, v] = clean();
  a.events.splice(4, 0, commit(70, "a3", 3, "h-a3", 1));
  const r = analyseRound([a, b, f, v]);
  assert.equal(r.counts.staleAcceptedAfterNewerEpoch, 1);
});
