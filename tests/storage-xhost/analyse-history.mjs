// History checks of the cross-host harness (plan 4B Task 23; see README.md): the `history` row
// (a brancher, a merger, a follower and fresh verifiers must agree) and the `mixed` row (a 4A
// follower of a 4B holder). The common checks (lost records, stale writes, double holders)
// are in analyse.mjs; these only add what is particular to history records.

const find = (procs, role) => procs.find((p) => p.role === role);
const evs = (p, ev) => (p?.events ?? []).filter((e) => e.ev === ev);
const commits = (p) => evs(p, "commit").filter((e) => e.ok);

/** `{counts, critical, problems, detail}`; empty when the round has no brancher. */
export function historyChecks(procs) {
  const brancher = find(procs, "brancher");
  if (!brancher) return { counts: {}, critical: false, problems: [], detail: null };
  return find(procs, "follower-4a") ? mixed(procs, brancher) : history(procs, brancher);
}

function history(procs, brancher) {
  const problems = [];
  const mine = commits(brancher);
  const branchId = mine.find((e) => e.kind === "branch")?.branchId;
  const onBranch = mine.filter((e) => e.branch === branchId && e.phase === "edits");
  const conflict = onBranch.filter((e) => e.conflict).map((e) => e.txId);
  const merger = find(procs, "merger");
  const m = evs(merger, "merged")[0];
  if (!m) problems.push("the merger reported no merge");
  // The skipped record: the branch's conflicting trim, reported with its reason.
  const skipped = (m?.skipped ?? []).map((c) => ({ txId: c.txId, reason: c.reason, againstTx: c.againstTx }));
  const skippedReported = conflict.length > 0 && conflict.every((t) => skipped.some((s) => s.txId === t && s.reason === "SAME_EDGE"));
  if (m && !skippedReported) problems.push(`the conflicting record ${conflict} was not reported skipped (SAME_EDGE): ${JSON.stringify(skipped)}`);
  const expectedMerged = onBranch.length - conflict.length;
  const mergedOk = m?.merged === expectedMerged;
  if (m && !mergedOk) problems.push(`merged ${m.merged} records, expected ${expectedMerged}`);
  // Every view must match the merger's after it exported: the follower on the other host after
  // one follow cycle, and the fresh verifiers.
  const ref = m ? { stateHash: m.stateHash, indexHash: m.indexHash } : null;
  const views = {};
  const follower = find(procs, "follower");
  const followed = evs(follower, "followed")[0];
  views[`${follower?.label ?? "follower"}@${follower?.host}`] = followed?.hashes ?? null;
  if (follower && !followed) problems.push(`follower ${follower.label} did not apply the merge (no followed event)`);
  for (const v of procs.filter((p) => p.role === "verify" || p.role === "verify-other")) views[`${v.label}@${v.host}`] = evs(v, "hashes")[0] ?? null;
  let mismatches = 0;
  const diffs = {};
  for (const [k, h] of Object.entries(views)) {
    const same = ref && h && h.stateHash === ref.stateHash && h.indexHash === ref.indexHash;
    if (!same) {
      mismatches++;
      diffs[k] = { stateHash: h?.stateHash ?? null, indexHash: h?.indexHash ?? null, heads: h?.heads ?? null, error: h?.error };
    }
  }
  if (ref && mismatches) problems.push(`views differ from the merger's (${m.stateHash.slice(0, 19)}, ${m.indexHash.slice(0, 19)}): ${JSON.stringify(diffs)}`);
  const takeoverMs = evs(merger, "writable")[0]?.waitMs ?? null;
  const caughtUp = evs(merger, "caughtUp")[0]?.behind ?? null;
  if (caughtUp?.length) problems.push(`merger merged before reaching the brancher's heads: ${JSON.stringify(caughtUp)}`);
  return {
    counts: {
      historyMerged: m?.merged ?? 0,
      historySkipped: skipped.length,
      historySkippedMissing: m && skippedReported ? 0 : 1,
      historyMergedWrong: m && mergedOk ? 0 : 1,
      historyHashMismatches: ref ? mismatches : 1,
      historyViews: Object.keys(views).length,
    },
    critical: !m || !skippedReported || !mergedOk || mismatches > 0 || Boolean(caughtUp?.length),
    problems,
    detail: { branchId, conflict, skipped, merged: m?.merged ?? null, expectedMerged, ref, views: Object.keys(views), diffs, takeoverMs, mergerHeads: m?.heads ?? null },
  };
}

const reason = (r) => (typeof r === "string" ? r.replace(/([a-z])([A-Z])/g, "$1_$2").toUpperCase() : r);

function mixed(procs, brancher) {
  const problems = [];
  const mine = commits(brancher);
  const branchRec = mine.find((e) => e.kind === "branch");
  const before = mine.filter((e) => e.phase === "setup" || e.phase === "plain");
  const after = mine.filter((e) => e.phase === "edits" || e.phase === "branch");
  const old = find(procs, "follower-4a");
  const applied = new Set(evs(old, "applied").map((e) => e.txId));
  // Edits exported in the segment that holds the branch record are in a v2 segment (its
  // version follows its content), so a 4A reader cannot see them: reported, not missed.
  const folder = evs(find(procs, "verify"), "folder")[0];
  const seg = (folder?.scan?.segments ?? []).find((g) => g.records.some((r) => r[1] === branchRec?.txId));
  const inV2 = new Set((seg?.records ?? []).map((r) => r[1]));
  const sharedV2 = before.filter((e) => inV2.has(e.txId)).map((e) => e.txId);
  const missed = before.filter((e) => !applied.has(e.txId) && !inV2.has(e.txId)).map((e) => e.txId);
  const appliedAfter = after.filter((e) => applied.has(e.txId)).map((e) => e.txId);
  const ro = evs(old, "readOnly").map((e) => reason(e.reason));
  const newer = ro.includes("NEWER_FORMAT");
  const final = evs(old, "final")[0];
  const finalRo = reason(final?.readOnly ?? null);
  if (missed.length) problems.push(`4A follower missed ${missed.length} edit-only records: ${missed.slice(0, 5)}`);
  if (appliedAfter.length) problems.push(`4A follower applied records past the branch record: ${appliedAfter.slice(0, 5)}`);
  if (!newer) problems.push(`4A follower never turned read-only NEWER_FORMAT (readOnly: ${JSON.stringify(ro)})`);
  const end = evs(old, "end")[0];
  return {
    counts: {
      mixedEditOnly: before.length - sharedV2.length,
      mixedFollowed: before.length - sharedV2.length - missed.length,
      mixedEditsInBranchSegment: sharedV2.length,
      mixedMissed: missed.length,
      mixedAppliedPastBranch: appliedAfter.length,
      mixedNewerFormat: newer ? 1 : 0,
    },
    critical: missed.length > 0 || appliedAfter.length > 0 || !newer,
    problems,
    detail: { branchSeq: branchRec?.seq ?? null, branchSegment: seg?.name ?? null, sharedV2, readOnly: ro, finalReadOnly: finalRo, oldHead: final?.head ?? null, oldEnd: end ? { ok: end.ok, error: end.error } : null, missed, appliedAfter },
  };
}
