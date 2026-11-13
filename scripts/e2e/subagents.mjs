// The "subagents" scenario (plan 7B Task 13): rules, approvals, subagents, carve-outs, rollback,
// saved prompts, compaction and a session's review request, from the console, on built-in
// sessions of the local preset against a scripted model server on loopback (no key, no network;
// `subagents-script.mjs` says what the model does). The server is `serve` with one test option,
// the `auto` merge trigger's `observe` (`subagents-server.mjs`).
//   1. a person adds `ask marker.add` for the project and starts an `editor` session R; its first
//      `fin marker add` waits on a `rule` card (`waitingApproval`); the same call by R's credential
//      answers `APPROVAL_PENDING` (exit 8) with that card, and `fin approval wait` exit 8 too; the
//      person allows it with `remember: session`; `fin approval wait` exits 0, the call runs once,
//      and the second identical call passes by the remembered rule (no second card);
//   2. an `editor` session P spawns two `clip-finder` children with `fin agent spawn`, waits for
//      both with `fin agent wait --any` twice; the children end `briefDone` with summaries, P gets
//      a `subagent` node per child; P's tokens and money left are its limits minus what it and
//      both children spent (the carves settled), nothing still reserved;
//   3. P merges child one's branch into its own in run 2; `finisherd agent rollback P --to-run 1`
//      undoes the merge (P's branch as before the merge);
//   4. a spawn chain below an `editor` stops at depth 3: the third-level child's spawn answers
//      `DEPTH_EXCEEDED {kind: depth}` (exit 7) and the chain unwinds;
//   5. `finisherd agent run make-shorts --param count=2` starts a `clip-finder` session with the
//      expanded template (`trigger: savedPrompt`);
//   6. a session whose run 2 passes 75 % of its window compacts the runs before it (a `summary`
//      node, the compaction prompt sent once) and completes;
//   7. an `auto` session's `fin review request` opens a `branchReview` item (recorded on the
//      session, a `review` notice); the `auto` merge trigger answers `unavailable` (no
//      `VerdictSource` before sub-project 10), the item stays open and `main` is untouched; turning
//      the policy on is `AUTO_MERGE_UNAVAILABLE`; the children's ends with unmerged commits and no
//      review reached the trigger as `noReview`.
// Deviations from the plan's wording: the rule is on `marker.add` (`render` is not a command
// yet); the console's answers and rule are its RPCs with the challenge (the console commands
// need a terminal); money is the local model's owner-set price (`agents.models`).
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { controlDb, getSession, nodesOf, txsOf, waitSession } from "./agent-lib.mjs";
import { startModelServer } from "./agent-model.mjs";
import { consoleCli } from "./auth-lib.mjs";
import { writeFixture } from "./fixture.mjs";
import { FIN, ROOT, assertEqual, connectWs, consoleRpc, errorCode, finRaw, fin as finOk, live, log, removeTmp, scenarioTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { MODEL, PROMPTS, SUMMARY, WINDOW, finResult, scripted } from "./subagents-script.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
/** Micros per million tokens: what the local model costs here, so carves of money are exercised. */
const PRICE = { input: 1_000_000, output: 2_000_000, cacheRead: 0, cacheWrite: 0 };

function refused(r, status, code, what) {
  if (r.status !== status || r.json?.error?.code !== code) throw new Error(`${what}: expected exit ${status} ${code}, got exit ${r.status}: ${r.text} ${r.stderr}`);
  return r.json.error;
}

/** A call's refusal code (`consoleRpc` throws on an error answer). */
async function refusedWith(f, what) {
  try {
    await f();
  } catch (e) {
    return errorCode(e);
  }
  throw new Error(`${what}: expected a refusal`);
}

/** A session's ledger rows: `{tokens, money}` each `{limit, used, reserved}`. */
const ledgerOf = (sd, id) =>
  Object.fromEntries(controlDb(sd, (db) => db.prepare('SELECT kind, "limit" AS "limit", used, reserved FROM budgets WHERE sessionId = ?').all(id)).map((r) => [r.kind, { limit: r.limit, used: r.used, reserved: r.reserved }]));

/** The parsed results of a session's `fin` tool calls, oldest first. */
const finResultsOf = (sd, id) => nodesOf(sd, id).filter((n) => n.kind === "toolResult").map((n) => ({ ok: n.payload.ok, ...finResult(JSON.stringify(n.payload.output)) }));

export async function run() {
  const tmp = scenarioTmp("finisher-e2e-subagents-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "film");
  const mergeLog = join(tmp, "auto-merge.jsonl");
  let server;
  let ws;
  let models;
  try {
    step(0, "server (with the auto merge trigger observed), a project of two clips, the scripted model server");
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    const models0 = [{ id: MODEL, provider: "local", window: WINDOW, maxOutput: 1000, price: PRICE }];
    writeFileSync(join(sd, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN, models: models0 }, fonts: { autoFetch: false } })}\n`);
    writeFileSync(mergeLog, "");
    server = await startServer(sd, { FIN_E2E_AUTO_MERGE_LOG: mergeLog }, { entry: join(ROOT, "scripts", "e2e", "subagents-server.mjs") });
    const person = (args, cwd = project) => finOk(sd, cwd, args);
    const projectId = person(["project", "create", project, "--name", "Film"], tmp).project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    person(["asset", "add", "clip.mp4"]);
    for (const at of ["0", "end"]) person(["clip", "insert", "--no-audio", "--track", "t1", "--at", at, "--asset", "a1", "--dur", "4s"]);
    ws = await connectWs(server);
    const content = async (branch) => JSON.stringify((await ws.call("snapshot.get", { projectId, branch })).project);
    const mainBefore = await content("main");
    models = await startModelServer({ model: MODEL, respond: scripted });
    await consoleRpc(server, "agent.local.set", { endpoint: models.baseUrl });

    const start = (prompt, extra = []) => {
      const r = consoleCli(sd, ["agent", "start", "--project", project, "--provider", "local", "--model", MODEL, "--prompt", prompt, "--json", ...extra]);
      assertEqual(r.status, 0, `agent start (${r.stderr})`);
      return JSON.parse(r.stdout).session;
    };
    const credential = (id) => join(sd, "agents", id, "credential.json");
    const agentFin = (id, args) => finRaw(sd, join(project, "workspace", id), args, { FIN_CREDENTIAL: credential(id), FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: project });
    const approvalsOf = async (id, status) => (await consoleRpc(server, "agent.approval.list", status === undefined ? {} : { status }, { confirm: false })).approvals.filter((a) => a.sessionId === id);
    const settled = (id) => waitSession(server, id, ["idle", "interrupted", "ended"]);

    step(1, "ask marker.add; the agent's call waits on a rule card; allowed with remember: session; the second call passes by the rule");
    const { rule } = await consoleRpc(server, "agent.rules.add", { effect: "ask", command: "marker.add", projectId });
    assertEqual([rule.layer, rule.effect, rule.command], ["local", "ask", "marker.add"], "the person's rule");
    const R = start(PROMPTS.mark, ["--title", "markers"]);
    await waitSession(server, R.id, ["waitingApproval"]);
    const [card] = await approvalsOf(R.id, "open");
    assertEqual([card?.kind, card?.subject?.command], ["rule", "marker.add"], "R's open card");
    // The same call by R's credential meets the same card: pending is exit 8, never a denial.
    const pending = refused(agentFin(R.id, ["marker", "add", "--at", "1s", "--label", "first"]), 8, "APPROVAL_PENDING", "R's marker add while the card is open");
    assertEqual(pending.details?.approval, card.id, "the identical call names the open card");
    refused(agentFin(R.id, ["approval", "wait", card.id, "--timeout", "1"]), 8, "APPROVAL_PENDING", "fin approval wait before the answer");
    const answered = await consoleRpc(server, "agent.approval.answer", { id: card.id, decision: "allow", remember: "session" });
    assertEqual([answered.approval.status, answered.rule?.layer], ["answered", "local"], "the answer and its remembered rule");
    const waited = agentFin(R.id, ["approval", "wait", card.id]);
    assertEqual(waited.status, 0, `fin approval wait after the answer (${waited.text})`);
    const doneR = await settled(R.id);
    assertEqual(doneR.status, "idle", "R after its run");
    assertEqual(finResultsOf(sd, R.id).map((r) => r.ok), [true, true], "both marker adds of R succeeded");
    assertEqual((await approvalsOf(R.id)).length, 1, "one card for R: the second call passed by the remembered rule");
    const markers = (await content(doneR.rootBranch)).split('"label":"first"').length - 1;
    assertEqual(markers, 2, "two markers on R's branch: the allowed call ran once, the remembered one once");
    const listed = (await consoleRpc(server, "agent.rules.list", { projectId, sessionId: R.id }, { confirm: false })).rules;
    const remembered = listed.find((r) => r.source === `approval:${card.id}`);
    assertEqual([remembered?.effect, remembered?.command, remembered?.sessionId, remembered?.remembered], ["allow", "marker.add", R.id, true], "the remembered rule is listed with its source");

    step(2, "an editor spawns two clip finders and waits for both with --any twice; the carves settle on it");
    const P = start(PROMPTS.spawn, ["--title", "parent", "--budget-usd", "1", "--budget-tokens", "400000"]);
    const doneP = await settled(P.id);
    assertEqual(doneP.status, "idle", "P after its run");
    const pResults = finResultsOf(sd, P.id);
    assertEqual(pResults.map((r) => r.exit), [0, 0, 0, 0], "P's spawns and waits succeeded");
    const [one, two] = pResults.slice(0, 2).map((r) => r.stdout);
    const waits = pResults.slice(2).map((r) => r.stdout.children);
    assertEqual(waits.every((c) => c.length >= 1 && c.some((x) => ["ended", "idle"].includes(x.status))), true, "each wait --any answered a child that was done");
    assertEqual(new Set(waits.map((c) => c.find((x) => ["ended", "idle"].includes(x.status)).child)).size, 2, "the two waits answered the two children");
    const kids = [];
    for (const k of [one, two]) {
      const s = await waitSession(server, k.session, ["ended"]);
      assertEqual([s.parent, s.rootSession, s.depth, s.role, s.mode, s.statusReason, s.rootBranch], [P.id, P.id, 1, "clip-finder", "draft", "briefDone", k.branch], `child ${k.session}`);
      assertEqual(txsOf(sd, k.session).length, 1, `child ${k.session} committed its trim`);
      kids.push(s);
    }
    const heard = nodesOf(sd, P.id).filter((n) => n.kind === "subagent").map((n) => n.payload);
    assertEqual(heard.map((h) => h.child).sort(), kids.map((k) => k.id).sort(), "P has a subagent node per child");
    assertEqual(heard.every((h) => h.status === "ended" && h.summary.startsWith("Clip ")), true, "the subagent nodes carry the children's summaries");
    const pLedger = ledgerOf(sd, P.id);
    const kidLedgers = kids.map((k) => ledgerOf(sd, k.id));
    const pOwn = (await getSession(server, P.id)).usage;
    for (const [kind, own] of [["money", pOwn.moneyMicros], ["tokens", null]]) {
      const children = kidLedgers.reduce((n, l) => n + l[kind].used, 0);
      assertEqual(children > 0, true, `the children spent ${kind}`);
      assertEqual(pLedger[kind].reserved, 0, `P's ${kind}: nothing still reserved for its children`);
      if (own !== null) assertEqual(pLedger[kind].used, own + children, `P's ${kind} used = its own spend + both children's`);
      log(`P ${kind}: limit ${pLedger[kind].limit}, used ${pLedger[kind].used} (children ${children}), remaining ${pLedger[kind].limit - pLedger[kind].used}`);
    }
    assertEqual(pLedger.money.limit, 1_000_000, "P's money limit as started");
    // The default carve is half the parent's remaining at the spawn (its own spend, the first carve held).
    const [c1, c2] = kidLedgers.map((k) => k.money.limit);
    assertEqual([c1 > 490_000 && c1 <= 500_000, c2 > 240_000 && c2 <= Math.floor((1_000_000 - c1) / 2)], [true, true], `the children's default money carves (${c1}, ${c2})`);

    step(3, "P merges child one's branch in run 2; a person rolls P back to run 1, which undoes the merge");
    const beforeMerge = await content(doneP.rootBranch);
    assertEqual(consoleCli(sd, ["agent", "prompt", P.id, `${PROMPTS.merge} ${one.branch}`, "--json"]).status, 0, "agent prompt P (merge)");
    await waitFor("P's second run to finish", () => nodesOf(sd, P.id).filter((n) => n.kind === "toolResult").length === 5 && nodesOf(sd, P.id).at(-1).kind === "assistantMessage", 30_000);
    await settled(P.id);
    assertEqual(finResultsOf(sd, P.id).at(-1).exit, 0, "P's merge of child one succeeded");
    const merged = await content(doneP.rootBranch);
    assertEqual(merged === beforeMerge, false, "the merge changed P's branch");
    const rolled = consoleCli(sd, ["agent", "rollback", P.id, "--to-run", "1", "--json"]);
    assertEqual(rolled.status, 0, `agent rollback (${rolled.stderr})`);
    const back = JSON.parse(rolled.stdout);
    assertEqual([back.undone.length, back.conversation], [1, "rolledBack"], "the rollback undid the merge and the conversation");
    assertEqual(await content(doneP.rootBranch), beforeMerge, "P's branch is as before the merge");
    assertEqual(nodesOf(sd, P.id).some((n) => n.kind === "notice" && n.payload?.rollback?.toRun === 1), true, "a rollback notice on P");

    step(4, "a spawn chain stops at depth 3 with DEPTH_EXCEEDED");
    const Q = start(`${PROMPTS.chain} 0`, ["--title", "chain"]);
    assertEqual((await settled(Q.id)).status, "idle", "Q after the chain unwound");
    const chain = [];
    for (let at = Q.id, depth = 1; depth <= 3; depth++) {
      const child = finResultsOf(sd, at)[0].stdout.session;
      const s = await waitSession(server, child, ["ended"]);
      assertEqual([s.depth, s.rootSession, s.parent, s.statusReason], [depth, Q.id, at, "briefDone"], `chain level ${depth}`);
      chain.push(s);
      at = child;
    }
    const deepest = finResultsOf(sd, chain[2].id);
    assertEqual(deepest.length, 1, "the third level made one call");
    assertEqual([deepest[0].exit, deepest[0].error?.code, deepest[0].error?.details?.kind, deepest[0].error?.details?.depth, deepest[0].error?.details?.max], [7, "DEPTH_EXCEEDED", "depth", 4, 3], "the third level's spawn is refused");

    step(5, "a saved prompt starts a session: make-shorts --param count=2");
    const ran = consoleCli(sd, ["agent", "run", "make-shorts", "--project", project, "--param", "count=2", "--provider", "local", "--model", MODEL, "--json"]);
    assertEqual(ran.status, 0, `agent run make-shorts (${ran.stderr})`);
    const shorts = await settled(JSON.parse(ran.stdout).session.id);
    assertEqual([shorts.status, shorts.role, shorts.mode, shorts.trigger], ["idle", "clip-finder", "draft", "savedPrompt"], "the saved prompt's session");
    const asked = nodesOf(sd, shorts.id).find((n) => n.kind === "userMessage");
    assertEqual(JSON.stringify(asked.payload).includes("Find the 2 strongest short clips"), true, "its first prompt is the expanded template");

    step(6, "a session whose second run passes 75 % of the window compacts and completes");
    const K = start(PROMPTS.compactOne, ["--title", "compaction"]);
    await settled(K.id);
    assertEqual(consoleCli(sd, ["agent", "prompt", K.id, PROMPTS.compactTwo, "--json"]).status, 0, "agent prompt K");
    await waitFor("K's second run", () => nodesOf(sd, K.id).filter((n) => n.kind === "assistantMessage").length >= 2, 30_000);
    const doneK = await settled(K.id);
    assertEqual(doneK.status, "idle", "K after its compacted run");
    const kNodes = nodesOf(sd, K.id);
    const summaries = kNodes.filter((n) => n.kind === "summary");
    assertEqual([summaries.length, summaries[0]?.payload?.text], [1, SUMMARY], "one summary node holding the model's summary");
    assertEqual(kNodes.at(-1).kind, "assistantMessage", "K's run ended with the model's answer");
    const kRequests = models.requests().filter((r) => JSON.stringify(r.body.messages).includes(PROMPTS.compactOne));
    assertEqual(kRequests.filter((r) => JSON.stringify(r.body.messages.at(-1)).includes("about to be replaced by a summary")).length, 1, "one compaction call");
    const afterCompaction = models.requests().filter((r) => JSON.stringify(r.body.messages).includes(SUMMARY));
    assertEqual(afterCompaction.length, 1, "the request after the compaction carries the summary");
    assertEqual(JSON.stringify(afterCompaction[0].body.messages).includes(PROMPTS.compactOne), false, "the summary replaced run 1's messages");

    step(7, "an auto session's review request opens an item; the auto merge trigger answers unavailable; nothing merges");
    const S = start(PROMPTS.review, ["--title", "review", "--mode", "auto"]);
    const doneS = await settled(S.id);
    assertEqual([doneS.status, doneS.mode], ["idle", "auto"], "S after its run");
    const requested = finResultsOf(sd, S.id).at(-1);
    assertEqual(requested.exit, 0, `S's fin review request (${JSON.stringify(requested)})`);
    const itemId = requested.stdout.item.id;
    const item = person(["review", "show", itemId]).item;
    assertEqual([item.kind, item.state, item.source], ["branchReview", "open", doneS.rootBranch], "the item S asked for");
    const reviews = controlDb(sd, (db) => db.prepare("SELECT reviews FROM session_state WHERE sessionId = ?").get(S.id));
    assertEqual(JSON.parse(reviews.reviews), [itemId], "the item is recorded on the session");
    const notice = nodesOf(sd, S.id).find((n) => n.kind === "notice" && n.payload?.review !== undefined);
    assertEqual([notice?.payload.review.itemId, notice?.payload.review.branch], [itemId, doneS.rootBranch], "a review notice in S's run");
    const outcomes = await waitFor("the trigger's outcome for S's item", () => {
      const all = readFileSync(mergeLog, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
      return all.some((o) => o.event.reviewId === itemId) ? all : undefined;
    }, 15_000);
    const forItem = outcomes.filter((o) => o.event.reviewId === itemId);
    assertEqual(forItem.map((o) => [o.event.sessionId, o.event.branch, o.outcome]), [[S.id, doneS.rootBranch, { merged: false, reason: "unavailable" }]], "the auto merge trigger: unavailable");
    assertEqual(outcomes.every((o) => o.outcome.merged === false), true, "the trigger merged nothing");
    // The children ended with unmerged commits and no review: `branchReady` without an item.
    for (const k of kids) assertEqual(outcomes.filter((o) => o.event.sessionId === k.id).map((o) => o.outcome.reason), ["noReview"], `the trigger on child ${k.id}'s end`);
    log(`trigger outcomes: ${outcomes.map((o) => `${o.event.sessionId}:${o.outcome.reason}`).join(", ")}`);
    const policyOn = await refusedWith(() => consoleRpc(server, "review.policy", { projectId, autoMerge: { enabled: true } }), "auto merge on");
    assertEqual(policyOn, "AUTO_MERGE_UNAVAILABLE", "turning auto merge on");
    assertEqual(person(["review", "show", itemId]).item.state, "open", "the item stays open for a person");
    assertEqual(await content("main"), mainBefore, "main is untouched by every session");
    log(`subagents scenario passed (${models.requests().length} model requests)`);
  } finally {
    ws?.close();
    if (server !== undefined) await stopServer(server);
    await models?.close();
    removeTmp(tmp);
  }
}
