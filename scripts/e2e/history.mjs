// The "history" scenario: branches, a conflicting merge, skip-conflicting, undo/redo/undo --to,
// checkpoints, try keep/drop, cherry-pick, revert, a merge the other way, dry runs, and a
// real `project recover --create`, all through `fin` -> finisherd, with WebSocket subscribers
// checking which branch each record is published to.
import { join } from "node:path";
import { assertEqual, connectWs, fin, finRaw, live, log, removeTmp, scenarioTmp, startServer, stopServer } from "./lib.mjs";
import { writeFixture } from "./fixture.mjs";
import { clipsOf, failure, watch } from "./history-lib.mjs";
import { dryRuns, recover } from "./history-extra.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const settle = () => new Promise((r) => setTimeout(r, 300));

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-hist-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "history project");
  const servers = [];
  const sockets = [];
  try {
    const server = await startServer(sd);
    servers.push(server);
    const f = (args, env) => fin(sd, project, args, env);
    const raw = (args, env) => finRaw(sd, project, args, env ?? {});
    const created = fin(sd, tmp, ["project", "create", project, "--name", "History"]);
    const projectId = created.project.projectId;
    const ws = await connectWs(server);
    sockets.push(ws);
    const snap = (branch) => ws.call("snapshot.get", { projectId, branch });
    const heads = () => Object.fromEntries(f(["branch", "ls", "--all"]).branches.map((b) => [b.display, b.head]));
    const insertT1 = (extra = [], dur = "1s") =>
      f([...extra, "clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", dur]);

    step(1, "asset, two clips on t1");
    await writeFixture(join(project, "clip.mp4"));
    // Another author adds the asset, so `fin`'s own undo stack starts at its clips.
    f(["asset", "add", "clip.mp4"], { FIN_AUTHOR: "setup" });
    f(["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"]);
    f(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur", "3s"]);

    step(2, "branch new feature; on it: insert on t2 (audio), trim c1 +1s");
    const feature = f(["branch", "new", "feature"]);
    assertEqual([feature.branch.display, feature.branch.state, feature.branch.parent], ["feature", "active", "main"], "branch new");
    const featureId = feature.branch.id;
    const fSnap0 = await snap("feature");
    const featureWatch = watch(ws, projectId, "feature", fSnap0.rev, fSnap0.generation);
    const onFeature = ["--branch", "feature"];
    const t2 = f([...onFeature, "clip", "insert", "--no-audio", "--track", "t2", "--at", "0", "--asset", "a1", "--dur", "2s"]);
    const featureTrim = f([...onFeature, "clip", "trim", "c1", "--end", "+1s"]);
    await featureWatch.waitFor("both feature edits", (s) => s.length >= 2);
    assertEqual(featureWatch.txIds(), [t2.txId, featureTrim.txId], "feature subscriber txIds");
    assertEqual(featureWatch.branches(), [featureId], "feature records are published to the feature branch only");
    assertEqual(clipsOf(await snap("feature")), { t1: ["c1:0+5", "c2:5+3"], t2: ["c3:0+2"] }, "feature clips");

    step(3, "on main: trim c1 -1s, insert at the end of t1");
    const mainTrim = f(["clip", "trim", "c1", "--end", "-1s"]);
    const mainInsert = insertT1();
    const afterStep3 = await snap("main");
    assertEqual(clipsOf(afterStep3), { t1: ["c1:0+3", "c2:3+3", "c4:6+1"], t2: [] }, "main clips after step 3");
    assertEqual(featureWatch.seen.length, 2, "main edits do not reach the feature subscriber");

    const mainWatch = watch(ws, projectId, "main", afterStep3.rev, afterStep3.generation);
    const dryAnswer = (r) => r.json?.merge ?? r.json;

    step(4, "dry run first: merge feature writes nothing");
    const beforeDry = heads();
    const dry = raw(["merge", "feature", "--skip-conflicting", "--dry-run"]);
    assertEqual(dry.status, 0, "dry merge exit");
    assertEqual(dryAnswer(dry).dryRun, true, "dry merge says dryRun");
    assertEqual([dryAnswer(dry).merged.length, dryAnswer(dry).skipped.length], [1, 1], "dry merge plan");
    assertEqual(heads(), beforeDry, "a dry run leaves every head alone");

    step(5, "merge feature -> exit 4, one SAME_EDGE conflict");
    const conflict = failure(raw(["merge", "feature"]), "CONFLICT", 4, "merge feature");
    assertEqual(conflict.details.conflicts.map((c) => c.reason), ["SAME_EDGE"], "conflict reasons");
    assertEqual(heads(), beforeDry, "a refused merge writes nothing");

    step(6, "merge feature --skip-conflicting -> one merged, one skipped");
    const merged = f(["merge", "feature", "--skip-conflicting"]).merge;
    assertEqual([merged.merged.length, merged.skipped.length], [1, 1], "merge outcome");
    assertEqual(merged.skipped[0].reason, "SAME_EDGE", "skipped reason");
    assertEqual(merged.merged[0], t2.txId, "the t2 insert is the merged record");
    assertEqual(merged.skipped[0].txId, featureTrim.txId, "the trim is the skipped record");
    assertEqual(clipsOf(await snap("main")).t2.length, 1, "t2 clip arrived on main");

    step(7, "fin undo (same author) undoes the merge unit");
    const undone = f(["undo"]);
    assertEqual(undone.undone, [merged.mergeId], "undone unit is the merge");
    const afterUndo = await snap("main");
    assertEqual(clipsOf(afterUndo), clipsOf(afterStep3), "main is back to the step 3 clips");
    assertEqual(afterUndo.project, afterStep3.project, "main project equals the step 3 snapshot");
    const list = f(["branch", "ls"]).branches;
    assertEqual(list.find((b) => b.display === "feature").state, "active", "feature stays active");

    step(8, "main subscriber saw the merge record, then the undo record, in order");
    await mainWatch.waitFor("merge and undo records", (s) => s.length >= 2);
    await settle();
    const ids = mainWatch.txIds();
    assertEqual(ids.length, 2, "exactly the merge copy and the undo");
    if (!ids[0].startsWith(merged.mergeId)) throw new Error(`first record ${ids[0]} is not a copy of merge ${merged.mergeId}`);
    assertEqual(ids[1], undone.txId, "second record is the undo");
    assertEqual(mainWatch.seen.map((t) => t.rev), [merged.rev, undone.rev], "revs in order");
    assertEqual(mainWatch.branches(), ["main"], "all on main");
    assertEqual(featureWatch.seen.length, 2, "the feature subscriber saw nothing of it");

    step(9, "redo re-applies the merge, undo takes it away again");
    const redone = f(["redo"]);
    assertEqual(redone.redone, [undone.txId], "redo names the undo it reverses");
    assertEqual(clipsOf(await snap("main")).t2.length, 1, "t2 clip is back after redo");
    const undone2 = f(["undo"]);
    assertEqual(clipsOf(await snap("main")), clipsOf(afterStep3), "undo again");

    step(10, "checkpoint, an edit, undo --to the checkpoint");
    const cp = f(["checkpoint", "cp1"]);
    const atCp = await snap("main");
    insertT1([], "2s");
    insertT1([], "2s");
    const back = f(["undo", "--to", "cp1"]);
    assertEqual(back.undone.length, 2, "two edits undone");
    assertEqual(clipsOf(await snap("main")), clipsOf(atCp), "main equals the checkpoint state");
    failure(raw(["undo", "--to", "no-such-checkpoint"]), "NOT_FOUND", 3, "undo --to unknown");

    step(11, "rename, archive and restore");
    f(["branch", "rename", "feature", "feat"]);
    assertEqual(f(["branch", "ls"]).branches.map((b) => b.display), ["main", "feat"], "renamed");
    f(["branch", "archive", "feat"]);
    assertEqual(f(["branch", "ls"]).branches.map((b) => b.display), ["main"], "archived branch is hidden");
    assertEqual(f(["branch", "ls", "--all"]).branches.find((b) => b.id === featureId).state, "archived", "ls --all shows it archived");
    const refused = raw(["--branch", featureId, "clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]);
    failure(refused, "BRANCH_ARCHIVED", 3, "write to an archived branch");
    f(["branch", "archive", featureId, "--restore"]);
    assertEqual(f(["branch", "ls"]).branches.map((b) => b.display), ["main", "feat"], "restored");

    step(12, "try drop archives; try keep merges into the parent");
    f(["try", "scratch"]);
    insertT1([], "1s"); // lands on the try branch
    assertEqual(clipsOf(await snap("main")), clipsOf(atCp), "main untouched while trying");
    const dropped = f(["try", "drop"]);
    assertEqual(dropped.branch.state, "archived", "drop archives");
    assertEqual(clipsOf(await snap("main")), clipsOf(atCp), "main untouched after drop");
    f(["try"]);
    const kept = insertT1([], "1s");
    const keep = f(["try", "keep"]).merge;
    assertEqual(clipsOf(await snap("main")).t1.length, clipsOf(atCp).t1.length + 1, "keep brought the clip to main");

    step(13, "cherry-pick and revert");
    f(["branch", "new", "pick"]);
    const picked = insertT1(["--branch", "pick"], "1s");
    const pick = f(["cherry-pick", picked.txId]);
    assertEqual(pick.noOp, undefined, "cherry-pick wrote");
    const again = f(["cherry-pick", picked.txId]);
    assertEqual(again.noOp, "ALREADY_PRESENT", "a second cherry-pick of the same record is a no-op");
    f(["branch", "new", "rv"]);
    insertT1(["--branch", "rv"], "1s");
    const mergedRv = f(["merge", "rv"]).merge;
    const beforeRevert = clipsOf(await snap("main"));
    const reverted = f(["revert", mergedRv.mergeId]);
    assertEqual(reverted.reverted !== undefined, true, "revert answers");
    assertEqual(clipsOf(await snap("main")).t1.length, beforeRevert.t1.length - 1, "revert removed the merged clip");

    step(14, "merge the other way: main into side");
    f(["branch", "new", "side"]);
    const sideOwn = insertT1(["--branch", "side"], "1s");
    const mainOwn = insertT1([], "2s");
    const sideBefore = clipsOf(await snap("side")).t1.length;
    const down = f(["merge", "main", "--into", "side"]).merge;
    assertEqual(down.merged, [mainOwn.txId], "main's new record is merged into side");
    assertEqual(clipsOf(await snap("side")).t1.length, sideBefore + 1, "side gained main's clip");

    step(15, "subscribers: records on the right branches, in order; a stale generation ends HISTORY_REWOUND");
    await mainWatch.waitFor("the last main record", (s) => s.some((t) => t.tx.txId.startsWith(mergedRv.mergeId)) && s.some((t) => t.tx.txId === reverted.txId));
    await settle();
    const mainIds = mainWatch.txIds();
    for (const id of [undone.txId, redone.txId, undone2.txId, cp.txId, back.txId, pick.txId, reverted.txId]) {
      if (!mainIds.includes(id)) throw new Error(`main subscriber missed ${id}`);
    }
    for (const mergeId of [merged.mergeId, keep.mergeId, mergedRv.mergeId]) {
      if (!mainIds.some((id) => id.startsWith(mergeId))) throw new Error(`main subscriber missed a copy of merge ${mergeId}`);
    }
    // Records written on other branches, or merged into them, never reach main.
    for (const id of [t2.txId, featureTrim.txId, picked.txId, kept.txId, sideOwn.txId]) {
      if (mainIds.includes(id)) throw new Error(`main subscriber received ${id}, a record of another branch`);
    }
    if (mainIds.some((id) => id.startsWith(down.mergeId))) throw new Error("the merge into side reached main");
    const revs = mainWatch.seen.map((t) => t.rev);
    assertEqual(revs, [...revs].sort((a, b) => a - b), "main revs arrive in order");
    assertEqual(new Set(revs).size, revs.length, "no rev twice");
    assertEqual(mainWatch.branches(), ["main"], "only main records on the main subscriber");
    assertEqual(featureWatch.seen.length, 2, "the feature subscriber saw only the feature edits");
    const stale = ws.subscribe({ projectId, branch: "main", fromRev: 0, generation: { incarnation: "an-older-database", n: 0 } }, () => {});
    const staleExit = await stale.done;
    assertEqual(JSON.stringify(staleExit).includes("HISTORY_REWOUND"), true, "stale generation ends the stream");

    step(16, "every dry run writes nothing and publishes nothing; idempotency keys");
    await dryRuns({ f, raw, snap, mainWatch, insert: insertT1 });

    mainWatch.stop();
    featureWatch.stop();
    await recover({ tmp, sd, server, servers, sockets, writeFixture });
    log(`PASS history in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const ws of sockets) ws.close();
    for (const s of servers.reverse()) await stopServer(s);
    removeTmp(tmp);
  }
}
