// Self-test of the history checks (analyse-history.mjs) on synthetic rounds: a clean `history`
// and `mixed` round have no findings, and each planted fault is found.
import assert from "node:assert/strict";
import { test } from "node:test";
import { analyseRound } from "./analyse.mjs";

const S = 1e9;
function proc(label, role, server, host, evs) {
  const events = [{ ev: "start", t: 0 }, ...evs, { ev: "end", t: 200, ok: true }].map((e) => ({ label, server, mono: e.t * S, wall: e.t * S, ...e }));
  return { id: `${label}:${role}`, label, role, host, server, events };
}
const commit = (t, txId, branch, seq, extra = {}) => ({ ev: "commit", t, ok: true, txId, branch, seq, txHash: `h-${txId}`, rowEpoch: extra.epoch ?? 1, lease: "held", epoch: extra.epoch ?? 1, ids: [], kind: "edit", ...extra });
const H = { stateHash: "blake3:s", indexHash: "blake3:i", heads: {} };

/** Brancher A on host h1 (branch br1: e1, trim e2 conflicting, e3), merger B on h2, follower F
 * on h1, verifiers on both hosts. */
function history() {
  const rows = [
    [1, "s1", "h-s1", 1, true, "x", "main", "edit"],
    [2, "x1", "h-x1", 1, true, "x", "main", "branch"],
    [3, "m1", "h-m1", 1, true, "x", "main", "edit"],
    [4, "g-1", "h-g-1", 2, true, "x", "main", "merge"],
    [5, "g-2", "h-g-2", 2, true, "x", "main", "merge"],
    [2, "e1", "h-e1", 1, true, "x", "br1", "edit"],
    [3, "e2", "h-e2", 1, true, "x", "br1", "edit"],
    [4, "e3", "h-e3", 1, true, "x", "br1", "edit"],
  ];
  const a = proc("a", "brancher", "A", "h1", [
    { ev: "lease", t: 1, lease: "held", epoch: 1 },
    commit(2, "s1", "main", 1, { phase: "setup" }),
    commit(3, "x1", "main", 2, { phase: "branch", kind: "branch", branchId: "br1" }),
    commit(4, "e1", "br1", 2, { phase: "edits", conflict: false }),
    commit(5, "e2", "br1", 3, { phase: "edits", conflict: true }),
    commit(6, "m1", "main", 3, { phase: "edits", conflict: true }),
    commit(7, "e3", "br1", 4, { phase: "edits", conflict: false }),
    { ev: "final", t: 9, txStatus: {} },
  ]);
  const b = proc("b", "merger", "B", "h2", [
    { ev: "lease", t: 70, lease: "held", epoch: 2, previous: { record: "A" } },
    { ev: "writable", t: 70, waitMs: 55_000 },
    { ev: "caughtUp", t: 70, behind: [] },
    commit(71, "g-1", "main", 4, { kind: "merge", epoch: 2 }),
    commit(71, "g-2", "main", 5, { kind: "merge", epoch: 2 }),
    { ev: "merged", t: 74, merged: 2, skipped: [{ txId: "e2", reason: "SAME_EDGE", againstTx: "m1" }], ...H },
    { ev: "final", t: 75, txStatus: {} },
  ]);
  const f = proc("f", "follower", "F", "h1", [{ ev: "opened", t: 2 }, { ev: "followed", t: 80, hashes: H }]);
  const chain = { ev: "chain", t: 100, rows: rows.filter((r) => r[6] === "main"), all: rows, quarantine: [] };
  const v = proc("verify", "verify", "V", "h1", [{ ev: "verify", t: 100, ok: true, checks: [] }, chain, { ev: "hashes", t: 100, ...H }, { ev: "folder", t: 100, lease: { claims: [] }, scan: { segments: [] } }]);
  const w = proc("verify-b", "verify-other", "W", "h2", [{ ev: "hashes", t: 100, ...H }]);
  return [a, b, f, v, w];
}

test("a clean history round has no findings", () => {
  const r = analyseRound(history());
  assert.deepEqual(r.problems, []);
  assert.equal(r.critical, false);
  assert.equal(r.counts.lostAcked, 0);
  assert.equal(r.counts.idReuse, 0, "seqs repeat across branches, not within one");
  assert.deepEqual([r.counts.historyMerged, r.counts.historySkipped, r.counts.historyHashMismatches, r.counts.historyViews], [2, 1, 0, 3]);
});

test("a view that differs from the merger's is a mismatch", () => {
  const p = history();
  p[4].events.find((e) => e.ev === "hashes").indexHash = "blake3:other";
  const r = analyseRound(p);
  assert.equal(r.counts.historyHashMismatches, 1);
  assert.equal(r.critical, true);
});

test("a follower that never saw the merge is a mismatch", () => {
  const p = history();
  p[2].events = p[2].events.filter((e) => e.ev !== "followed");
  const r = analyseRound(p);
  assert.equal(r.counts.historyHashMismatches, 1);
  assert.ok(r.problems.some((x) => x.includes("did not apply the merge")));
});

test("an unreported skipped record and a wrong merge count are critical", () => {
  const p = history();
  const m = p[1].events.find((e) => e.ev === "merged");
  m.skipped = [];
  m.merged = 3;
  const r = analyseRound(p);
  assert.equal(r.counts.historySkippedMissing, 1);
  assert.equal(r.counts.historyMergedWrong, 1);
  assert.equal(r.critical, true);
});

test("a branch record missing from the resolved rows is a lost record", () => {
  const p = history();
  const c = p[3].events.find((e) => e.ev === "chain");
  c.all = c.all.filter((r) => r[1] !== "e3");
  const r = analyseRound(p);
  assert.equal(r.counts.lostAcked, 1);
  assert.equal(r.critical, true);
});

/** A 4B brancher with an edit-only phase, and a 4A follower. */
function mixed({ follow = ["s1", "p1", "p2"], ro = "NEWER_FORMAT", shared = false } = {}) {
  const a = proc("a", "brancher", "A", "h1", [
    { ev: "lease", t: 1, lease: "held", epoch: 1 },
    commit(2, "s1", "main", 1, { phase: "setup" }),
    commit(3, "p1", "main", 2, { phase: "plain" }),
    commit(4, "p2", "main", 3, { phase: "plain" }),
    commit(5, "x1", "main", 4, { phase: "branch", kind: "branch", branchId: "br1" }),
    commit(6, "e1", "br1", 4, { phase: "edits" }),
  ]);
  const o = proc("o", "follower-4a", "O", "h2", [
    { ev: "opened", t: 2 },
    ...follow.map((t, i) => ({ ev: "applied", t: 3 + i, txId: t, seq: i + 1, txHash: `h-${t}` })),
    { ev: "readOnly", t: 9, reason: ro },
    { ev: "final", t: 20, readOnly: ro, head: follow.length },
  ]);
  const rows = [[1, "s1", "h-s1", 1, true, "x", "main"], [2, "p1", "h-p1", 1, true, "x", "main"], [3, "p2", "h-p2", 1, true, "x", "main"], [4, "x1", "h-x1", 1, true, "x", "main"], [4, "e1", "h-e1", 1, true, "x", "br1"]];
  const segments = [{ name: "s1", branch: "main", epoch: 1, writer: "A", records: [[1, "s1"], [2, "p1"]] }, { name: "s2", branch: "main", epoch: 1, writer: "A", records: [[3, "p2"], [4, "x1"]] }];
  const v = proc("verify", "verify", "V", "h1", [{ ev: "chain", t: 30, rows: rows.slice(0, 4), all: rows }, { ev: "folder", t: 30, lease: { claims: [] }, scan: { segments: shared ? segments : [] } }]);
  return [a, o, v];
}

test("a 4A follower that follows the edit-only part and turns NEWER_FORMAT is clean", () => {
  const r = analyseRound(mixed());
  assert.deepEqual(r.problems, []);
  assert.deepEqual([r.counts.mixedFollowed, r.counts.mixedEditOnly, r.counts.mixedNewerFormat], [3, 3, 1]);
  assert.equal(r.critical, false);
});

test("a 4A follower that misses an edit, applies past the branch, or never turns read-only is critical", () => {
  assert.equal(analyseRound(mixed({ follow: ["s1", "p1"] })).counts.mixedMissed, 1);
  assert.equal(analyseRound(mixed({ follow: ["s1", "p1", "p2", "x1"] })).counts.mixedAppliedPastBranch, 1);
  const r = analyseRound(mixed({ ro: null }));
  assert.equal(r.counts.mixedNewerFormat, 0);
  assert.equal(r.critical, true);
});

test("an edit exported with the branch record (a v2 segment) is reported, not missed", () => {
  const r = analyseRound(mixed({ follow: ["s1", "p1"], shared: true }));
  assert.deepEqual([r.counts.mixedFollowed, r.counts.mixedEditOnly, r.counts.mixedEditsInBranchSegment, r.counts.mixedMissed], [2, 2, 1, 0]);
  assert.equal(r.critical, false);
});
