// 4C checks of the cross-host harness (plan 4C Task 29; see README.md, "4C rows"):
// `upgrade4c` (a 4B project opens in 4C), `mixed4c` (4B and 4C servers on one project),
// `newer4c` (a 4C project on 4B) and `edit4c` (fonts and captions across hosts). The common
// checks (lost records, stale writes, double holders) are in analyse.mjs; these add what is
// particular to each row.

/** Roles only the 4C rows use: a round with one of them is analysed here, not by the history checks. */
export const ROLES_4C = ["upgrade", "holder-4b", "holder-4c", "follower-4b", "follower-4c", "editor", "follower-fetch", "follower-offline"];

const find = (procs, role) => procs.find((p) => p.role === role);
const evs = (p, ev) => (p?.events ?? []).filter((e) => e.ev === ev);
const ev1 = (p, ev) => evs(p, ev)[0];
const commits = (p) => evs(p, "commit").filter((e) => e.ok);
// A read-only reason as `NEWER_FORMAT`, from `newerFormat`, `NEWER_FORMAT` or `{reason, fix}`.
const reason = (r) => {
  const s = r && typeof r === "object" ? r.reason : r;
  return typeof s === "string" ? s.replace(/([a-z])([A-Z])/g, "$1_$2").toUpperCase() : s;
};
const verifiers = (procs) => procs.filter((p) => p.role === "verify" || p.role === "verify-other");
const sameHashes = (x, y) => Boolean(x && y && !x.error && x.stateHash === y.stateHash && x.indexHash === y.indexHash);
const short = (h) => (h ? { stateHash: h.stateHash ?? null, indexHash: h.indexHash ?? null, heads: h.heads ?? null, error: h.error } : null);

/** Whether `procs` is a round of a 4C row. */
export const is4cRound = (procs) => procs.some((p) => ROLES_4C.includes(p.role));

/** Paths where `a` and `b` differ (at most `max`), `$`-rooted like the reel's first difference. */
export function diffPaths(a, b, path = "$", out = [], max = 20) {
  if (out.length >= max) return out;
  if (a === b) return out;
  const ta = Array.isArray(a) ? "array" : a === null ? "null" : typeof a;
  const tb = Array.isArray(b) ? "array" : b === null ? "null" : typeof b;
  if (ta !== tb || (ta !== "object" && ta !== "array")) {
    out.push(path);
    return out;
  }
  if (ta === "array") {
    if (a.length !== b.length) out.push(`${path}.length`);
    for (let i = 0; i < Math.min(a.length, b.length); i++) diffPaths(a[i], b[i], `${path}[${i}]`, out, max);
    return out;
  }
  for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) diffPaths(a[k], b[k], `${path}.${k}`, out, max);
  return out;
}

/** A path a missing font may change: a caption layer's `layout` or `unmeasured`, or `textEngine`. */
export const isMeasurementPath = (p) => /^\$(\.layers\[\d+\](\.children\[\d+\])*\.caption\.(layout|unmeasured)\b|\.textEngine\b)/.test(p);

/** `{counts, critical, problems, detail, mayFail}` for a 4C round (`mayFail`: labels whose failed end is an outcome, not a problem). */
export function edit4cChecks(procs) {
  if (find(procs, "upgrade")) return upgrade(procs);
  if (find(procs, "editor")) return edit(procs);
  if (find(procs, "holder-4b")) return mixed(procs);
  return newer(procs);
}

function upgrade(procs) {
  const problems = [];
  const brancher = find(procs, "brancher");
  const u = find(procs, "upgrade");
  const before = ev1(brancher, "final")?.hashes?.heads ?? null;
  const opened = ev1(u, "upgraded");
  const after = opened?.hashes?.heads ?? null;
  const headsSame = Boolean(before && after && JSON.stringify(before) === JSON.stringify(after));
  if (!opened) problems.push("the upgrade reported nothing on open");
  else if (!headsSame) problems.push(`heads after the upgrade differ from the 4B brancher's: ${JSON.stringify({ before, after })}`);
  if (opened?.readOnly) problems.push(`the upgraded project is read-only: ${opened.readOnly}`);
  // The 4B database itself was migrated: it was there, older, before the open.
  const vBefore = opened?.dbVersionsBefore?.[opened?.projectId];
  const vAfter = opened?.dbVersionsAfter?.[opened?.projectId];
  const migrated = Number.isInteger(vBefore) && Number.isInteger(vAfter) && vBefore < vAfter;
  if (opened && !migrated) problems.push(`the 4B database was not migrated in place: ${JSON.stringify({ path: opened.dbPath, before: vBefore ?? null, after: vAfter ?? null })}`);
  const ver = ev1(u, "upgradeVerify");
  if (!ver?.ok) problems.push(`Store::verify after the upgrade failed: ${JSON.stringify((ver?.checks ?? []).filter((c) => !c.ok))}`);
  const undo = commits(u).find((e) => e.kind === "undo");
  if (!undo) problems.push("no undo record of a 4B record was accepted");
  const undoneOf = undo?.undone ?? [];
  const branched = new Set(commits(brancher).map((e) => e.txId));
  if (undo && !undoneOf.every((t) => branched.has(t))) problems.push(`the undo undid records the 4B brancher did not make: ${undoneOf}`);
  const ref = ev1(u, "undone")?.hashes ?? null;
  const views = {};
  for (const v of verifiers(procs)) views[`${v.label}@${v.host}`] = ev1(v, "hashes") ?? null;
  const diffs = Object.fromEntries(Object.entries(views).filter(([, h]) => !sameHashes(h, ref)).map(([k, h]) => [k, short(h)]));
  const nd = Object.keys(diffs).length;
  if (ref && nd) problems.push(`the rebuild from the folder differs from the upgrade's post-undo hashes (${ref.stateHash?.slice(0, 19)}): ${JSON.stringify(diffs)}`);
  if (!ref) problems.push("the upgrade reported no hashes after the undo");
  const ok = { migrated, headsSame, verify: Boolean(ver?.ok), undo: Boolean(undo), rebuild: Boolean(ref) && nd === 0 && Object.keys(views).length > 0 };
  return {
    counts: {
      upgradeMigrated: ok.migrated ? 1 : 0,
      upgradeHeadsSame: ok.headsSame ? 1 : 0,
      upgradeVerifyOk: ok.verify ? 1 : 0,
      upgradeUndone: ok.undo ? 1 : 0,
      upgradeRebuildSame: ok.rebuild ? 1 : 0,
      upgradeRecords4b: commits(brancher).length,
    },
    critical: !Object.values(ok).every(Boolean) || Boolean(opened?.readOnly),
    problems,
    detail: { dbVersion: [vBefore ?? null, vAfter ?? null], before, after, undo: undo ? { txId: undo.txId, seq: undo.seq, undone: undoneOf, of: undo.of } : null, ref: short(ref), views: Object.keys(views), diffs, verifyChecks: ver?.checks ?? null },
    mayFail: [],
  };
}

/** The follower `p`'s final view and every verifier's, against `ref`: `{views, diffs}`. */
function viewsAgainst(procs, followers, ref) {
  const views = {};
  for (const f of followers) views[`${f.label}@${f.host}`] = ev1(f, "followed")?.hashes ?? null;
  for (const v of verifiers(procs)) views[`${v.label}@${v.host}`] = ev1(v, "hashes") ?? null;
  const diffs = Object.fromEntries(Object.entries(views).filter(([, h]) => !sameHashes(h, ref)).map(([k, h]) => [k, short(h)]));
  return { views, diffs };
}

function mixed(procs) {
  const problems = [];
  const a = find(procs, "holder-4b");
  const b = find(procs, "holder-4c");
  const c = find(procs, "follower-4b");
  const d = find(procs, "follower-4c");
  const accA = commits(a);
  const accB = commits(b);
  const applied = new Set(evs(c, "applied").map((e) => e.txId));
  const missed = accA.filter((e) => !applied.has(e.txId)).map((e) => e.txId);
  const applied4c = accB.filter((e) => applied.has(e.txId)).map((e) => e.txId);
  const ro = evs(c, "readOnly").map((e) => reason(e.reason));
  const newer = ro.includes("NEWER_FORMAT");
  if (!accA.length) problems.push("the 4B holder had no record accepted");
  if (!accB.length) problems.push("the 4C holder had no record accepted");
  if (missed.length) problems.push(`the 4B follower missed ${missed.length} of the 4B holder's records: ${missed.slice(0, 5)}`);
  if (applied4c.length) problems.push(`the 4B follower applied 4C records: ${applied4c.slice(0, 5)}`);
  if (!newer) problems.push(`the 4B follower never turned read-only NEWER_FORMAT (readOnly: ${JSON.stringify(ro)})`);
  // The 4C follower's view after b's done marker, and the fresh 4C verifiers, must agree.
  const followed = ev1(d, "followed");
  if (!followed?.reached) problems.push(`the 4C follower did not reach the 4C holder's done marker (${followed ? "timed out" : "no followed event"})`);
  const ref = followed?.hashes ?? null;
  const { views, diffs } = viewsAgainst(procs, [], ref);
  const nd = Object.keys(diffs).length;
  if (nd) problems.push(`views differ from the 4C follower's (${ref?.stateHash?.slice(0, 19)}): ${JSON.stringify(diffs)}`);
  // When the 4B follower turned read-only, against the 4C holder's first record.
  const firstB = accB[0];
  const turned = evs(c, "readOnly").find((e) => reason(e.reason) === "NEWER_FORMAT");
  return {
    counts: {
      mixed4cAccepted4b: accA.length,
      mixed4cAccepted4c: accB.length,
      mixed4cFollowed4b: accA.length - missed.length,
      mixed4cMissed: missed.length,
      mixed4cApplied4c: applied4c.length,
      mixed4cNewerFormat: newer ? 1 : 0,
      mixed4cHashMismatches: ref ? nd : 1,
      mixed4cViews: Object.keys(views).length + 1,
    },
    critical: missed.length > 0 || applied4c.length > 0 || !newer || !ref || nd > 0 || !followed?.reached || !accA.length || !accB.length,
    problems,
    detail: {
      missed, applied4c, readOnly: ro, finalReadOnly: reason(ev1(c, "final")?.readOnly ?? null),
      newerAfterFirst4cMs: turned && firstB ? Math.round((turned.wall - firstB.wall) / 1e6) : null,
      takeoverMs: ev1(b, "writable")?.waitMs ?? null, ref: short(ref), views: Object.keys(views), diffs,
    },
    mayFail: [],
  };
}

function newer(procs) {
  const problems = [];
  const a = find(procs, "holder-4c");
  const old = find(procs, "follower-4b");
  const v4b = find(procs, "verify-4b");
  const accA = commits(a);
  if (!accA.length) problems.push("the 4C holder had no record accepted");
  // What 4B made of the project: opened read-only (NEWER_FORMAT), opened writable, or refused.
  const outcome = (p) => {
    if (!p) return { state: "missing" };
    const end = ev1(p, "end");
    const opened = ev1(p, "opened");
    if (!opened) return { state: "refused", error: end?.error ?? `exit ${p.exit}` };
    const ro = [...evs(p, "readOnly").map((e) => reason(e.reason)), reason(opened.readOnly ?? null), reason(ev1(p, "status")?.status?.readOnly ?? null)].filter(Boolean);
    return ro.includes("NEWER_FORMAT") ? { state: "readOnly", readOnly: [...new Set(ro)] } : { state: "writable", readOnly: [...new Set(ro)] };
  };
  const fo = outcome(old);
  const vo = outcome(v4b);
  for (const [who, o] of [["4B follower", fo], ["4B verify", vo]]) if (!["readOnly", "refused"].includes(o.state)) problems.push(`the ${who} neither opened read-only NEWER_FORMAT nor refused the project: ${JSON.stringify(o)}`);
  // Nothing written by 4B: commits, segments, the lease record, claims.
  const servers4b = new Set([old?.server, v4b?.server].filter(Boolean));
  const folder = ev1(find(procs, "verify"), "folder");
  const segs = (folder?.scan?.segments ?? []).filter((s) => servers4b.has(s.writer)).map((s) => s.name);
  const rec = folder?.lease?.record;
  const recBy4b = rec && servers4b.has(rec.serverId ?? rec.server_id) ? 1 : 0;
  const claims = (folder?.lease?.claims ?? []).filter((c) => servers4b.has(c.owner)).map((c) => c.name);
  const commits4b = commits(old).length;
  const wrote = segs.length + recBy4b + claims.length + commits4b;
  if (wrote) problems.push(`4B wrote to the 4C project: ${JSON.stringify({ segments: segs, leaseRecord: recBy4b, claims, commits: commits4b })}`);
  if (!folder) problems.push("no folder evidence from the 4C verifier");
  if (ev1(old, "warning")?.what === "followerAcquiredAtOpen") problems.push("the 4B follower acquired the lease at open");
  const ok = (o) => o.state === "readOnly" || o.state === "refused";
  return {
    counts: {
      newer4cAccepted: accA.length,
      newer4cReadOnly: [fo, vo].filter((o) => o.state === "readOnly").length,
      newer4cRefused: [fo, vo].filter((o) => o.state === "refused").length,
      newer4cWritable: [fo, vo].filter((o) => !ok(o)).length,
      newer4cWrote4b: wrote,
      newer4cApplied4b: evs(old, "applied").length,
    },
    critical: !ok(fo) || !ok(vo) || wrote > 0 || !accA.length,
    problems,
    detail: { follower: fo, verify: vo, wrote: { segments: segs, leaseRecord: recBy4b, claims, commits: commits4b }, probes: (folder?.lease?.probes ?? []).filter((p) => servers4b.has(p.server)).length },
    // A refusal by 4B is a recorded outcome, not a failed node.
    mayFail: [old, v4b].filter((p) => p && outcome(p).state === "refused").map((p) => p.label),
  };
}

function edit(procs) {
  const problems = [];
  const e = find(procs, "editor");
  const f = find(procs, "follower-fetch");
  const o = find(procs, "follower-offline");
  const mine = ev1(e, "plans4c");
  if (!mine) problems.push("the editor reported no plans");
  const ref = mine?.hashes ?? null;
  const frames = mine?.plans?.frames ?? [];
  if (mine?.plans?.error) problems.push(`the editor's eval failed: ${mine.plans.error}`);
  // The editor measured every caption.
  const editorUnmeasured = frames.flatMap((fr) => fr.captions.filter((c) => c.unmeasured || (c.missing ?? []).length).map((c) => `${fr.t}:${c.item}`));
  if (editorUnmeasured.length) problems.push(`the editor left captions unmeasured: ${editorUnmeasured.slice(0, 5)}`);
  if (frames.length < 4) problems.push(`the editor's plans cover ${frames.length} captions, expected 4`);
  // Hashes: every follower after its follow cycle, and the fresh verifiers.
  for (const p of [f, o]) {
    const fl = ev1(p, "followed");
    if (!fl?.reached) problems.push(`${p?.label ?? "a follower"} did not reach the editor's done marker`);
  }
  const { views, diffs } = viewsAgainst(procs, [f, o].filter(Boolean), ref);
  const nd = Object.keys(diffs).length;
  if (ref && nd) problems.push(`views differ from the editor's (${ref.stateHash?.slice(0, 19)}): ${JSON.stringify(diffs)}`);
  // Plans: the fetch follower's equal the editor's; the offline one's differ only in measurement.
  const byT = (p) => new Map((ev1(p, "plans4c")?.plans?.frames ?? []).map((fr) => [fr.t, fr]));
  const fp = byT(f);
  const op = byT(o);
  const planMismatch = [];
  const offlineOther = [];
  let offlineUnmeasured = 0;
  let offlineMeasured = 0;
  for (const fr of frames) {
    const x = fp.get(fr.t);
    if (!x || x.sha256 !== fr.sha256) planMismatch.push({ t: fr.t, got: x?.sha256 ?? null, paths: x ? diffPaths(x.plan, fr.plan) : null });
    const y = op.get(fr.t);
    if (!y) {
      offlineOther.push({ t: fr.t, missing: true });
      continue;
    }
    const other = diffPaths(y.plan, fr.plan, "$", [], 50).filter((p) => !isMeasurementPath(p));
    if (y.shapeSha256 !== fr.shapeSha256 || other.length) offlineOther.push({ t: fr.t, paths: other });
    for (const c of y.captions) {
      if (c.unmeasured === "fontMissing") offlineUnmeasured++;
      else offlineMeasured++;
    }
    if (y.textEngine != null) offlineOther.push({ t: fr.t, textEngine: y.textEngine });
  }
  if (frames.length && (fp.size !== frames.length || op.size !== frames.length)) problems.push(`followers' plans cover other times: editor ${frames.map((x) => x.t)}, fetch ${[...fp.keys()]}, offline ${[...op.keys()]}`);
  if (planMismatch.length) problems.push(`the fetch follower's plans differ from the editor's: ${JSON.stringify(planMismatch).slice(0, 600)}`);
  if (offlineOther.length) problems.push(`the offline follower's plans differ beyond caption measurement: ${JSON.stringify(offlineOther).slice(0, 600)}`);
  if (offlineMeasured) problems.push(`the offline follower measured ${offlineMeasured} captions (expected every one unmeasured, fontMissing)`);
  // The fetch follower fetched every pinned file itself.
  const pinned = new Set(Object.values(mine?.pins ?? {}));
  const fetch = evs(f, "fonts").at(-1)?.fetch ?? null;
  const downloaded = fetch?.downloaded ?? 0;
  if (!fetch) problems.push("the fetch follower reported no fetch");
  else if (downloaded !== pinned.size || fetch.failed) problems.push(`the fetch follower downloaded ${downloaded} of ${pinned.size} pinned files (${fetch.failed} failed, ${fetch.cached} already cached): ${JSON.stringify(fetch.files.filter((x) => x.how !== "downloaded"))}`);
  const cold = ev1(f, "fontCache");
  const editorFetch = evs(e, "fonts")[0]?.fetch ?? null;
  return {
    counts: {
      edit4cViews: Object.keys(views).length,
      edit4cHashMismatches: ref ? nd : 1,
      edit4cFrames: frames.length,
      edit4cPlanMismatches: planMismatch.length,
      edit4cOfflineOtherDiffs: offlineOther.length,
      edit4cOfflineUnmeasured: offlineUnmeasured,
      edit4cOfflineMeasured: offlineMeasured,
      edit4cEditorUnmeasured: editorUnmeasured.length,
      edit4cPinnedFiles: pinned.size,
      edit4cDownloaded: downloaded,
      edit4cDownloadedBytes: fetch?.bytes ?? 0,
    },
    critical: !mine || !ref || nd > 0 || planMismatch.length > 0 || offlineOther.length > 0 || offlineMeasured > 0 || editorUnmeasured.length > 0 || downloaded !== pinned.size || Boolean(fetch?.failed) || frames.length < 4,
    problems,
    detail: {
      ref: short(ref), views: Object.keys(views), diffs, pins: mine?.pins ?? null, times: frames.map((x) => x.t),
      editorFetch: editorFetch && { seeded: editorFetch.seeded, downloaded: editorFetch.downloaded, cached: editorFetch.cached, failed: editorFetch.failed, ms: editorFetch.ms },
      followerFetch: fetch && { downloaded, bytes: fetch.bytes, ms: fetch.ms, failed: fetch.failed, cleared: cold?.cleared ?? null },
      planMismatch, offlineOther, textEngine: frames.find((x) => x.textEngine)?.textEngine ?? null,
    },
    mayFail: [],
  };
}

/** The per-round console fragment for 4C counts. */
export function roundLine4c(c) {
  if (c.upgradeRebuildSame !== undefined) return ` migrated=${c.upgradeMigrated} heads=${c.upgradeHeadsSame} verify=${c.upgradeVerifyOk} undo=${c.upgradeUndone} rebuild=${c.upgradeRebuildSame}`;
  if (c.mixed4cNewerFormat !== undefined) return ` 4bFollowed=${c.mixed4cFollowed4b}/${c.mixed4cAccepted4b} applied4c=${c.mixed4cApplied4c} newer=${c.mixed4cNewerFormat} 4c=${c.mixed4cAccepted4c} hashMismatch=${c.mixed4cHashMismatches}/${c.mixed4cViews}`;
  if (c.newer4cWrote4b !== undefined) return ` readOnly=${c.newer4cReadOnly} refused=${c.newer4cRefused} writable=${c.newer4cWritable} wrote4b=${c.newer4cWrote4b}`;
  if (c.edit4cFrames !== undefined) return ` hashMismatch=${c.edit4cHashMismatches}/${c.edit4cViews} plans=${c.edit4cFrames} planMismatch=${c.edit4cPlanMismatches} offlineOther=${c.edit4cOfflineOtherDiffs} offlineUnmeasured=${c.edit4cOfflineUnmeasured} downloaded=${c.edit4cDownloaded}/${c.edit4cPinnedFiles}`;
  return "";
}

/** The report's lines for 4C counts (totals over rounds). */
export function reportLines4c(c) {
  const out = [];
  if (c.upgradeRebuildSame !== undefined) out.push(`upgrade4c: database migrated in place ${c.upgradeMigrated}, heads kept ${c.upgradeHeadsSame}, verify ok ${c.upgradeVerifyOk}, 4B record undone ${c.upgradeUndone}, rebuild same ${c.upgradeRebuildSame} (${c.upgradeRecords4b} 4B records)`);
  if (c.mixed4cNewerFormat !== undefined) out.push(`mixed4c: 4B follower applied ${c.mixed4cFollowed4b}/${c.mixed4cAccepted4b} 4B records (missed ${c.mixed4cMissed}), 4C records applied ${c.mixed4cApplied4c} of ${c.mixed4cAccepted4c}, NEWER_FORMAT in ${c.mixed4cNewerFormat} rounds, hash mismatches ${c.mixed4cHashMismatches} over ${c.mixed4cViews} views`);
  if (c.newer4cWrote4b !== undefined) out.push(`newer4c: 4B read-only ${c.newer4cReadOnly}, refused ${c.newer4cRefused}, writable ${c.newer4cWritable}; 4B writes ${c.newer4cWrote4b}; 4B applied ${c.newer4cApplied4b} of ${c.newer4cAccepted} 4C records`);
  if (c.edit4cFrames !== undefined) out.push(`edit4c: hash mismatches ${c.edit4cHashMismatches} over ${c.edit4cViews} views; ${c.edit4cFrames} plans, fetch-follower mismatches ${c.edit4cPlanMismatches}; offline: ${c.edit4cOfflineUnmeasured} captions unmeasured, ${c.edit4cOfflineMeasured} measured, ${c.edit4cOfflineOtherDiffs} other differences; editor unmeasured ${c.edit4cEditorUnmeasured}; downloaded ${c.edit4cDownloaded}/${c.edit4cPinnedFiles} files (${c.edit4cDownloadedBytes} bytes)`);
  return out;
}
