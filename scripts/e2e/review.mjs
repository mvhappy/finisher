// The "review" scenario (5E Task 11): an agent credential on its own branch asks for a review,
// the local person decides, merges and drops, a conflicting merge opens a `mergeConflict` item, the
// project moves to a second server data (the drop holds, the agent branch is re-derived), the
// policy refuses auto merge and the agent, and the journal and audit are read for what they hold.
import { cpSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { entriesIn, filesIn } from "./agent-lib.mjs";
import { writeCredentialFile } from "./auth-lib.mjs";
import { writeFixture } from "./fixture.mjs";
import { clipsOf } from "./history-lib.mjs";
import { assertEqual, connectWs, consoleRpc, errorCode, fin, finRaw, live, log, removeTmp, scenarioTmp, socketCall, startServer, stopServer, waitFor } from "./lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/** The call's error code; fails when it succeeded. */
async function refusedWith(call, what) {
  try {
    await call();
  } catch (e) {
    return errorCode(e);
  }
  throw new Error(`${what}: expected a refusal`);
}

/** Every audit row the console can list. */
async function auditOf(server) {
  const rows = [];
  for (let cursor; ; ) {
    const page = await consoleRpc(server, "auth.audit.list", { limit: 1000, ...(cursor ? { cursor } : {}) }, { confirm: false });
    if (!Array.isArray(page.entries) || (page.next !== undefined && typeof page.next !== "string")) throw new Error(`unexpected auth.audit.list answer: ${JSON.stringify(page).slice(0, 300)}`);
    rows.push(...page.entries);
    cursor = page.next;
    if (cursor === undefined) return rows;
  }
}

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-rev-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const sdB = join(tmp, "s2");
  const project = join(tmp, "review project");
  const servers = [];
  const sockets = [];
  let server;
  try {
    server = await startServer(sd);
    servers.push(server);
    const raw = (args) => finRaw(sd, project, args);
    const local = (args) => {
      const r = raw(args);
      if (r.status !== 0) throw new Error(`fin ${args.join(" ")} exited ${r.status}: ${r.text}`);
      return r.json;
    };
    const as = (cred, args) => finRaw(sd, project, args, { FIN_CREDENTIAL: cred.path, FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: project });
    const agentOk = (cred, args) => {
      const r = as(cred, args);
      if (r.status !== 0) throw new Error(`agent fin ${args.join(" ")} exited ${r.status}: ${r.text}`);
      return r.json;
    };
    const refused = (r, status, code, what) => assertEqual([r.status, r.json?.error?.code], [status, code], what);

    step(1, "project, an asset, an agent credential on its own branches and a wide one");
    const created = fin(sd, tmp, ["project", "create", project, "--name", "Review"]);
    const projectId = created.project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    local(["asset", "add", "clip.mp4"]);
    const mint = async (label, opts) => {
      const minted = await consoleRpc(server, "auth.credential.mint", { kind: "agent", label, ...opts });
      return { id: minted.credential.id, file: minted.file, path: writeCredentialFile(join(tmp, `${label}.cred`), minted.file) };
    };
    const agent = await mint("rev-agent", { branches: ["own"] });
    const wide = await mint("rev-wide", {});
    const insert = (cred, branch, dur = "1s") =>
      agentOk(cred, ["--branch", branch, "clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", dur]);

    step(2, "the agent makes a branch with four edits; main is protected; it asks for a review");
    const work = agentOk(agent, ["branch", "new", "work"]).branch;
    const edits = [1, 2, 3, 4].map((n) => insert(agent, "work", `${n}s`).txId);
    refused(as(agent, ["--branch", "main", "clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]), 7, "BRANCH_NOT_ALLOWED", "an own-branch credential on main");
    refused(as(wide, ["--branch", "main", "clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]), 7, "BRANCH_PROTECTED", "a wide credential on protected main");
    const summary1 = "SECRET-SUMMARY-1";
    const requested = agentOk(agent, ["review", "request", "--branch", "work", "--title", "Four edits", "--summary", summary1]);
    const itemId = requested.item.id;
    assertEqual([requested.item.kind, requested.item.state], ["branchReview", "open"], "the requested item");

    step(3, "local reviews: reject one, accept two, leave one pending; merge takes the two");
    const shown = local(["review", "show", itemId]);
    const entriesOf = (s) => s.plan.entries.filter((e) => e.branch === work.id || e.branch === undefined);
    assertEqual(entriesOf(shown).map((e) => e.origin), edits, "show lists the four entries in order");
    refused(as(agent, ["review", "decide", itemId, "--accept", edits[0]]), 7, "SCOPE_DENIED", "the agent cannot decide");
    const comment = "the intro drags SECRET-COMMENT";
    agentOk(agent, ["review", "comment", itemId, comment, "--tx", edits[0]]);
    const rejectReason = "SECRET-REASON-1";
    const decided = local(["review", "decide", itemId, "--reject", edits[0], "--accept", `${edits[1]},${edits[2]}`, "--reason", rejectReason]);
    assertEqual(decided.decisions.length, 3, "three decisions");
    const merged = local(["review", "merge", itemId]);
    assertEqual([...merged.merge.merged].sort(), [edits[1], edits[2]].sort(), "exactly the two accepted origins merged");
    const stateAfter = merged.item.state;
    log(`item after the partial merge: ${stateAfter}`);
    const mainSnap = async () => (await ws.call("snapshot.get", { projectId, branch: "main" }));
    const ws = await connectWs(server);
    sockets.push(ws);
    const clipCount = async () => clipsOf(await mainSnap()).t1.length;
    assertEqual(await clipCount(), 2, "main holds the two accepted clips");

    step(4, "the partial merge closed the item; a fifth edit and a new request; a sixth makes the old snapshot stale");
    assertEqual(stateAfter, "merged", "a merge without --accept-pending closes the item");
    const fifth = insert(agent, "work", "5s").txId;
    const again = agentOk(agent, ["review", "request", "--branch", "work", "--title", "The rest", "--summary", "SECRET-SUMMARY-2"]);
    assertEqual(again.existing, false, "a new item for the rest");
    const item2 = again.item.id;
    const shown2 = local(["review", "show", item2]);
    assertEqual(shown2.plan.entries.filter((e) => e.status !== "alreadyPresent" && e.status !== "dropped").map((e) => e.origin), [edits[3], fifth], "the new item lists the pending one and the fifth");
    assertEqual(shown2.plan.entries.find((e) => e.origin === edits[0]).status, "dropped", "the rejected one shows as dropped");
    const oldSnapshot = JSON.stringify(shown2.item.snapshot);
    insert(agent, "work", "6s");
    local(["review", "refresh", item2]);
    refused(raw(["review", "merge", item2, "--accept-pending", "--expect", oldSnapshot]), 4, "REVIEW_STALE", "merge on the old snapshot");
    const second = local(["review", "merge", item2, "--accept-pending"]);
    assertEqual(second.merge.merged.length, 3, "the pending one, the fifth and the sixth");
    assertEqual(second.merge.merged.some((t) => t.startsWith(edits[0])), false, "not the rejected one");
    assertEqual(await clipCount(), 5, "main holds five clips");

    // A request for changes and a close on a branch of its own, with text in each (nothing reaches main).
    agentOk(agent, ["branch", "new", "side"]);
    insert(agent, "side", "7s");
    const item3 = agentOk(agent, ["review", "request", "--branch", "side", "--title", "Closing SECRET-TITLE-3", "--summary", "SECRET-SUMMARY-3"]).item.id;
    local(["review", "decide", item3, "--request-changes", "--reason", "SECRET-REASON-3"]);
    local(["review", "close", item3, "--dismissed"]);

    step(5, "a plain merge of the agent branch leaves the dropped origin out");
    const plain = local(["merge", work.id, "--skip-conflicting"]).merge;
    assertEqual(plain.merged.length, 0, "nothing new to merge");
    assertEqual(plain.excluded.some((e) => e.txId === edits[0] && e.reason === "DROPPED"), true, `DROPPED in excluded: ${JSON.stringify(plain.excluded)}`);
    assertEqual(await clipCount(), 5, "main is unchanged");

    step(6, "a conflicting merge opens a mergeConflict item; a later successful one supersedes it");
    const clash = agentOk(agent, ["branch", "new", "clash"]).branch;
    const firstClip = clipsOf(await mainSnap()).t1[0].split(":")[0];
    agentOk(agent, ["--branch", "clash", "clip", "trim", firstClip, "--end", "+1s"]);
    local(["clip", "trim", firstClip, "--end", "-1s"]);
    const conflict = raw(["merge", clash.id]);
    refused(conflict, 4, "CONFLICT", "the plain merge conflicts");
    const conflicts = local(["review", "ls", "--kind", "mergeConflict"]).items;
    assertEqual(conflicts.map((i) => i.state), ["open"], "one open mergeConflict item");
    local(["merge", clash.id, "--skip-conflicting"]);
    const after = local(["review", "ls", "--kind", "mergeConflict"]).items;
    assertEqual(after.map((i) => i.state), ["superseded"], "the later merge supersedes it");

    step(7, "the project moves to a second server data: the drop holds, the agent branch is re-derived");
    ws.close();
    sockets.length = 0;
    await waitFor("the export to catch up", () => local(["project", "status"]).status.branches.every((b) => b.exportedThrough >= b.head), 20_000);
    // No RPC or `fin` command lists records, so the drop is read from the exported segments of the folder.
    const segmentText = () => filesIn(join(project, ".finisher")).map(([, b]) => b.toString("latin1")).join("\n");
    const dropLine = segmentText().split("\n").find((l) => l.includes('"kind":"drop"') && l.includes(edits[0]));
    if (dropLine === undefined) throw new Error(`no drop record in the exported segments: ${filesIn(join(project, ".finisher")).map(([f, b]) => f.split("/").pop() + ":" + b.length)}`);
    assertEqual(dropLine.includes(edits[1]) || dropLine.includes(edits[2]), false, "the drop names only the rejected origin");
    const auditA = await auditOf(server);
    await consoleRpc(server, "project.close", { projectId });
    await stopServer(server);
    servers.length = 0;
    // The folder is copied with its lease files: the clean close released the lease, so the second server takes it at
    // once (no 45 s staleness wait).
    const moved = join(tmp, "moved project");
    cpSync(project, moved, { recursive: true });
    const record = readFileSync(join(moved, ".finisher", "lease.json"), "latin1");
    assertEqual(record.includes('"released":true'), true, "the clean close released the lease");
    const serverB = await startServer(sdB);
    servers.push(serverB);
    const localB = (args) => {
      const r = finRaw(sdB, moved, args);
      if (r.status !== 0) throw new Error(`fin ${args.join(" ")} exited ${r.status}: ${r.text}`);
      return r.json;
    };
    localB(["project", "open", moved]);
    const takeStart = Date.now();
    await waitFor("the second server to take the released lease", () => !localB(["project", "status"]).status.readOnly, 30_000);
    assertEqual(Date.now() - takeStart < 25_000, true, `taken without the staleness wait (${Date.now() - takeStart} ms)`);
    const plainB = localB(["merge", work.id, "--skip-conflicting"]).merge;
    assertEqual(plainB.merged.length, 0, "nothing to merge on the second server");
    assertEqual(plainB.excluded.some((e) => e.txId === edits[0] && e.reason === "DROPPED"), true, "the drop holds there");
    const rederived = localB(["review", "ls", "--kind", "branchReview", "--state", "open"]).items.filter((i) => i.source === clash.id);
    assertEqual(rederived.length, 1, "the active agent branch has one item");
    const openItems = localB(["review", "ls", "--kind", "branchReview", "--state", "open"]).items;
    assertEqual(openItems.some((i) => i.source === work.id), false, "the fully reviewed branch gets no re-derived item");
    assertEqual(rederived[0].rederived, true, "marked re-derived");

    step(8, "auto merge is unavailable; the agent cannot write the policy");
    const code = await refusedWith(() => consoleRpc(serverB, "review.policy", { projectId, autoMerge: { enabled: true } }), "auto merge on");
    assertEqual(code, "AUTO_MERGE_UNAVAILABLE", "auto merge refused");
    const agentB = (await consoleRpc(serverB, "auth.credential.mint", { kind: "agent", label: "rev-agent-b", branches: ["own"] })).file; // the first server's credentials stayed there
    const humanOnly = await refusedWith(() => socketCall(serverB, "review.policy", { projectId, requireReview: true }, agentB), "agent policy write");
    assertEqual(humanOnly, "HUMAN_ONLY", "the agent's policy write");
    await consoleRpc(serverB, "review.policy", { projectId, requireReview: true });

    step(9, "the journal's verdicts carry counts and no text; the audit rows carry no titles or comments");
    const verdicts = entriesIn(sd, "events").filter((e) => e.kind === "review.verdict").map((e) => e.payload);
    assertEqual(verdicts.map((v) => [v.verdict, v.by, v.accepted, v.rejected]).filter((v) => v[0] !== "changesRequested" && v[0] !== "dismissed"), [["accepted", "user", 2, 0], ["rejected", "user", 0, 1], ["merged", "user", 2, 0], ["merged", "user", 3, 0]], "the verdicts: who and how many");
    assertEqual(verdicts.length >= 6, true, `verdicts incl. request-changes and close: ${verdicts.length}`);
    for (const v of verdicts) {
      assertEqual(Object.keys(v).filter((k) => ["reason", "title", "summary", "comment", "text"].includes(k)), [], "no text in a verdict (training is off)");
      assertEqual(JSON.stringify(v).includes("Four edits"), false, "no title in a verdict");
    }
    const audit = [...auditA, ...(await auditOf(serverB))];
    const kinds = new Set(audit.map((e) => e.kind));
    log(`audit kinds: ${[...kinds]}`);
    for (const kind of ["history.drop", "review.policy"]) assertEqual(kinds.has(kind), true, `a ${kind} row`);
    const needles = ["Four edits", "The rest", "SECRET-TITLE-3", comment, rejectReason, "SECRET-REASON-3", "SECRET-SUMMARY-1", "SECRET-SUMMARY-2", "SECRET-SUMMARY-3"];
    const events = filesIn(join(sd, "events")).map(([, bytes]) => bytes.toString("utf8")).join("\n");
    // The searches must not be vacuous: the bytes searched hold what they should.
    assertEqual(events.includes("review.verdict"), true, "events/ holds the verdict entries");
    assertEqual(JSON.stringify(audit).includes("history.drop"), true, "the audit text holds history.drop");
    for (const needle of needles) {
      assertEqual(events.includes(needle), false, `${needle} is not in events/`);
      assertEqual(JSON.stringify(audit).includes(needle), false, `${needle} is in no audit row`);
    }
    log(`PASS review in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const ws of sockets) ws.close();
    for (const s of servers.reverse()) await stopServer(s);
    removeTmp(tmp);
  }
}
