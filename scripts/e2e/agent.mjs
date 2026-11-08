// The "agent" scenario (7A Task 13): a built-in agent session from prompt to merged verdict, with a
// crash and a budget stop, against a scripted model server on loopback. The session runs the
// local preset (`provider local set <url>`, `agent start --provider local`): loopback needs no
// test hooks, no key and no network, and the server is the release `finisherd` with the release
// `fin`. Model replies come from `agent-model.mjs`, chosen by what the conversation holds.
//   1. a dummy OpenRouter key and the local endpoint are set as the owner (typed code); session A
//      starts in `draft` mode with "tighten the intro"; `finisherd agent watch` renders its nodes;
//   2. the model calls `fin view --budget 2000`, `fin item trim …`, then `fin caption import …`;
//   3. its branch holds two commits by `agent:editor:<id>` and `main` is untouched; with the
//      credential `fin` cannot commit to `main` (`BRANCH_NOT_ALLOWED`, exit 7); a write with no
//      `--branch` lands on the session's branch even after a person's `fin try`; a path outside the
//      project (`caption import --words /etc/hosts`) and the `--project /` shape are refused; a
//      revoked or unusable credential is never replaced by `local`; `session_txs` lists the commits;
//   4. the exports (training on): the call join (`agent.toolResult` → `log.commit`), trace turns
//      that are deltas replaying to the exact request bodies, provenance on every record;
//   5. `finisherd` is killed while the third tool call of session C runs; after the restart the
//      session is `interrupted`; resumed and prompted, it completes and nothing ran twice;
//   6. `fin merge` as `local`: every session tx is `merged` in `session_txs`;
//   7. a token budget stops session D (`BUDGET_EXCEEDED`, a `budget` approval); the owner allows
//      more (`agent.approval.answer {add}`) and it finishes;
//   8. on a second server with training off, an empty secret store (a decoy where `secrets.json`
//      would be: any access fails) and no key: the local session completes; no content is on disk,
//      the metadata says `provider: local`, no cost, and no record holds the model server's URL;
//   9. the leak grep over both servers' files, outputs and every request the model server saw.
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { MODEL, PROMPTS, scripted, wordsJson } from "./agent-script.mjs";
import {
  contentOf,
  entriesIn,
  filesIn,
  getSession,
  grep,
  nodesOf,
  outcomesOf,
  startWatch,
  txsOf,
  waitSession,
} from "./agent-lib.mjs";
import { consoleCli } from "./auth-lib.mjs";
import { contentProblems, exportEntries, keyer, loadContentFields, parseJsonl } from "./diagnostics-lib.mjs";
import { writeFixture } from "./fixture.mjs";
import { startModelServer } from "./agent-model.mjs";
import { FIN, assertEqual, connectWs, consoleRpc, finRaw, socketCall, fin as finOk, live, log, outputs, removeTmp, scenarioTmp, startServer, stopServer, waitFor } from "./lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const SWITCHES = ["FIN_DIAGNOSTICS", "FIN_TRAINING", "DO_NOT_TRACK"];
const DUMMY_KEY = "sk-or-v1-e2edummykey0123456789abcdef";
const SRT_CUES = 400;

/** Throws unless `r` is a `fin` refusal with this exit status and error code. */
function refused(r, status, code, what) {
  if (r.status !== status || r.json?.error?.code !== code) throw new Error(`${what}: expected exit ${status} ${code}, got exit ${r.status}: ${r.text} ${r.stderr}`);
  return r.json.error;
}

export async function run() {
  const saved = Object.fromEntries(SWITCHES.map((k) => [k, process.env[k]]));
  for (const k of SWITCHES) delete process.env[k];
  const tmp = scenarioTmp("finisher-e2e-agent-");
  live.tmp = tmp;
  const sd = join(tmp, "s1");
  const sd2 = join(tmp, "s2");
  const project = join(tmp, "film one");
  const project2 = join(tmp, "film two");
  const servers = [];
  const sockets = [];
  let models;
  let key;
  try {
    // Two projects of generated media (the reel scenario's fixture writer): a clip twice on t1, a caption track.
    const ctx = { project, words: join(project, "words.json"), srt: join(project, "outro.srt") };
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    mkdirSync(sd2, { recursive: true, mode: 0o700 });
    for (const dir of [sd, sd2]) writeFileSync(join(dir, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN } })}\n`);

    step(0, "servers, projects, the scripted model server");
    let server = await startServer(sd, { FIN_TRAINING: "on", FIN_DIAGNOSTICS: "on" });
    servers.push(server);
    const person = (args, cwd = project, env = {}) => finOk(sd, cwd, args, env);
    const created = person(["project", "create", project, "--name", "Film One"], tmp);
    const projectId = created.project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    person(["asset", "add", "clip.mp4"]);
    for (const at of ["0", "end"]) person(["clip", "insert", "--no-audio", "--track", "t1", "--at", at, "--asset", "a1", "--dur", "4s"]);
    const captionTrack = person(["track", "add", "--kind", "caption"]);
    const track = Object.values(captionTrack.bindings ?? {})[0] ?? "t2";
    writeFileSync(ctx.words, wordsJson(6));
    writeFileSync(ctx.srt, Array.from({ length: SRT_CUES }, (_, i) => `${i + 1}\n00:00:${String(Math.floor(i / 20)).padStart(2, "0")},${String((i % 20) * 50).padStart(3, "0")} --> 00:00:${String(Math.floor(i / 20)).padStart(2, "0")},${String((i % 20) * 50 + 40).padStart(3, "0")}\nline ${i + 1}\n`).join("\n"));
    let ws = await connectWs(server);
    sockets.push(ws);
    const mainSnapshot = async () => ws.call("snapshot.get", { projectId, branch: "main" });
    // What an edit would change: the project's content (a branch's creation adds a record to its parent's stream, not an edit).
    const mainContent = async () => JSON.stringify((await mainSnapshot()).project);
    const mainBefore = await mainContent();

    models = await startModelServer({ model: MODEL, respond: scripted({ clip: "c2", words: ctx.words, srt: ctx.srt, track }) });
    const restart = async (env) => {
      await stopServer(server);
      server = await startServer(sd, env);
      servers.push(server);
    };
    key = await keyer(sd, join(tmp, "key-copy"));

    step(1, "owner sets a dummy OpenRouter key and the local endpoint; session A starts in draft mode");
    const set = await consoleRpc(server, "agent.key.set", { provider: "openrouter", key: DUMMY_KEY });
    assertEqual([set.key.provider, JSON.stringify(set).includes(DUMMY_KEY)], ["openrouter", false], "the key's metadata");
    const local = await consoleRpc(server, "agent.local.set", { endpoint: models.baseUrl });
    assertEqual(local.preset, "custom", "the local preset");
    const providers = await consoleRpc(server, "agent.providers", {}, { confirm: false });
    assertEqual([providers.local.configured, providers.local.reachable, providers.openrouter.keySet], [true, true, true], "agent.providers");
    assertEqual(JSON.stringify(providers).includes(String(new URL(models.baseUrl).port)), false, "agent.providers never names the address");

    const startArgs = (prompt, extra = []) => ["agent", "start", "--project", project, "--provider", "local", "--model", MODEL, "--mode", "draft", "--prompt", prompt, "--json", ...extra];
    const startSession = (prompt, extra) => {
      const r = consoleCli(sd, startArgs(prompt, extra));
      assertEqual(r.status, 0, `agent start (${r.stderr})`);
      return JSON.parse(r.stdout).session;
    };
    const A = startSession(PROMPTS.intro, ["--title", "intro"]);
    const watchA = startWatch(sd, A.id);
    const credential = (id) => join(sd, "agents", id, "credential.json");
    const agentEnv = (id) => ({ FIN_CREDENTIAL: credential(id), FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: project });
    const agentFin = (id, args, env = {}) => finRaw(sd, join(project, "workspace", id), args, { ...agentEnv(id), ...env });
    const idle = await waitSession(server, A.id, ["idle", "interrupted", "ended"]);
    assertEqual([idle.status, idle.harness, idle.provider, idle.model, idle.mode], ["idle", "builtin", "local", MODEL, "draft"], "session A after its run");

    step(2, "the model's calls: view, item trim, caption import");
    const callsOf = (id) => nodesOf(sd, id).filter((n) => n.kind === "toolCall").map((n) => n.payload.input.argv);
    const callsA = callsOf(A.id);
    assertEqual(callsA.map((a) => a.slice(0, 2).join(" ")), ["view --budget", "item trim", "caption import"], "tool calls of session A");
    const resultsA = nodesOf(sd, A.id).filter((n) => n.kind === "toolResult");
    assertEqual(resultsA.map((n) => n.payload.ok), [true, true, true], "every call of A succeeded");

    step(3, "two commits on the session branch by agent:editor:<id>, main untouched, fin confined");
    const rows = txsOf(sd, A.id);
    assertEqual(rows.length, 2, "session_txs of A lists both commits");
    assertEqual(rows.map((r) => r.attribution), ["exact", "exact"], "attribution of A's commits");
    const root = (await getSession(server, A.id)).rootBranch;
    for (const r of rows) {
      const got = (await ws.call("tx.get", { projectId, txId: r.txId })).tx;
      assertEqual([got.author, got.branch], [`agent:editor:${A.id}`, root], `tx ${r.txId}`);
    }
    assertEqual(await mainContent(), mainBefore, "main's content is untouched by the session");
    const mainHead = (await mainSnapshot()).rev;
    const onMain = refused(agentFin(A.id, ["--branch", "main", "marker", "add", "--at", "1s", "--label", "x"]), 7, "BRANCH_NOT_ALLOWED", "the credential writing main");
    log(`BRANCH_NOT_ALLOWED: ${onMain.message}`);
    assertEqual([(await mainSnapshot()).rev, await mainContent()], [mainHead, mainBefore], "main after the refused write");
    // A person's `fin try` in another shell moves the person's default, never the session's.
    const tried = person(["try"]);
    const whoami = agentFin(A.id, ["whoami"]);
    assertEqual([whoami.status, whoami.json?.principal?.kind, whoami.json?.principal?.branches], [0, "agent", [root]], "whoami of the session");
    const self = await socketCall(server, "agent.self", {}, JSON.parse(readFileSync(credential(A.id), "utf8")));
    assertEqual([self.defaultBranch, self.tries], [root, []], "the session's default branch after a person's fin try");
    const manual = agentFin(A.id, ["item", "trim", "c1", "--end", "-1s"]);
    assertEqual(manual.status, 0, `a write with no --branch (${manual.text})`);
    const landed = (await ws.call("tx.get", { projectId, txId: manual.json.txId })).tx;
    assertEqual([landed.branch, landed.author], [root, `agent:editor:${A.id}`], "the unbranched write lands on the session's branch");
    assertEqual(txsOf(sd, A.id).length, 3, "session_txs follows every write of the credential");
    person(["try", "drop"]);
    log(`a person's try branch: ${JSON.stringify(tried.branch?.display ?? tried)}`);
    assertEqual(await mainContent(), mainBefore, "main's content after the person's try");
    // Paths outside the project, and the `--project /` argv shape (review B1), are refused before any file is read.
    const importArgs = (...words) => ["caption", ...words, "--track", track, "--style", "preset:bold"];
    refused(agentFin(A.id, importArgs("import", "--words", "/etc/hosts")), 7, "PATH_NOT_ALLOWED", "caption import --words /etc/hosts");
    refused(agentFin(A.id, ["caption", "--project", "/", "import", "--words", ctx.words, "--track", track, "--style", "preset:bold"]), 3, "PROJECT_NOT_FOUND", "the B1 shape: caption --project / import");
    // A revoked or unusable credential is `UNAUTHENTICATED`-class, never `local`: server data is on offer and its key is not used.
    refused(finRaw(sd, project, ["whoami"], { ...agentEnv(A.id), FIN_CREDENTIAL: join(tmp, "no-such-credential.json") }), 7, "UNAUTHENTICATED", "a missing credential file");
    assertEqual(finRaw(sd, project, ["whoami"], { FIN_SERVER: server.info.socket }).json?.principal?.kind, "local", "without a credential fin is local (control)");

    step(4, "the journal: the call join, content as deltas that replay, provenance, no leak");
    const stored = (args) => parseJsonl(consoleCli(sd, ["diagnostics", "export", "--content", ...args]).stdout);
    const commits = await waitFor("log.commit entries of A's txs", () => {
      const all = stored(["--kind", "log.commit"]);
      return rows.every((r) => all.some((e) => e.ctx?.txId === key.idRef(r.txId))) ? all : undefined;
    }, 15_000);
    const results = stored(["--kind", "agent.toolResult"]).filter((e) => e.ctx?.actor?.session === A.id);
    const joined = results.flatMap((e) => e.payload.txIds ?? []);
    for (const r of rows) {
      assertEqual(joined.includes(key.idRef(r.txId)), true, `agent.toolResult names tx ${r.txId}`);
      assertEqual(commits.some((e) => e.ctx?.txId === key.idRef(r.txId)), true, `log.commit exists for tx ${r.txId}`);
    }
    assertEqual(results.filter((e) => (e.payload.txIds ?? []).length > 0).every((e) => e.payload.attribution === "exact"), true, "attribution exact");
    const toolCalls = stored(["--kind", "agent.toolCall"]).filter((e) => e.ctx?.actor?.session === A.id);
    assertEqual(toolCalls.length, 3, "agent.toolCall entries of A");
    const traces = entriesIn(sd, "agent-traces");
    assertEqual(traces.length > 0, true, "training on: the trace store holds entries");
    for (const e of traces.filter((t) => ["agent.toolCall", "agent.toolResult", "agent.turn"].includes(t.kind))) {
      const p = e.payload.provenance ?? e.payload;
      assertEqual([p.provider, "weights" in p], ["local", true], `provenance of ${e.kind}`);
    }
    const turnsA = traces.filter((t) => t.kind === "agent.turn" && t.ctx?.actor?.session === A.id && t.payload.messagesDelta !== undefined);
    const sentA = models.requests().filter((r) => r.body.messages?.some((m) => m.role === "user" && String(m.content).includes(PROMPTS.intro)));
    assertEqual(turnsA.length, sentA.length, "one trace turn per model call of A");
    // Loaded now, not at the top: fin-node reads FIN_NODE_BINARY_DIR when first loaded, and the fixture writer sets it.
    const { replayBodies } = await import("../../apps/finisherd/src/agent/journal-content.ts");
    const replayed = replayBodies(
      turnsA.map((t) => ({
        run: t.payload.runId,
        turnId: t.payload.turnId,
        delta: contentOf(sd, t.payload.messagesDelta),
        system: JSON.stringify(contentOf(sd, t.payload.system)),
        tools: JSON.stringify(contentOf(sd, t.payload.tools)),
        bodyBlake3: t.payload.bodyBlake3,
      })),
    );
    assertEqual(replayed, sentA.map((r) => r.raw), "the trace deltas replay to the exact request bodies");
    assertEqual(turnsA.slice(1).every((t) => contentOf(sd, t.payload.messagesDelta).from > 0), true, "later turns are deltas, not whole bodies");

    step(5, "finisherd is killed during the third tool call of session C; restart; resume; nothing runs twice");
    const C = startSession(PROMPTS.outro, ["--title", "outro"]);
    const open = (n) => nodesOf(sd, C.id).filter((x) => x.kind === n).length;
    await waitFor("the third tool call announced and unanswered", () => open("toolCall") === 3 && open("toolResult") === 2, 30_000, () => ` (calls ${open("toolCall")}, results ${open("toolResult")})`, 2);
    server.child.kill("SIGKILL");
    await server.exited;
    server = await startServer(sd, { FIN_TRAINING: "on", FIN_DIAGNOSTICS: "on" });
    servers.push(server);
    ws = await connectWs(server); // the old connection died with the old server
    sockets.push(ws);
    const hurt = await getSession(server, C.id);
    assertEqual([hurt.status, hurt.statusReason], ["interrupted", "serverRestart"], "session C after the restart");
    await new Promise((r) => setTimeout(r, 500)); // a `fin` that outlived the server settles (it finds no server)
    const callsC = nodesOf(sd, C.id).filter((n) => n.kind === "toolCall");
    const third = callsC[2];
    const thirdResult = nodesOf(sd, C.id).filter((n) => n.kind === "toolResult" && n.callId === third.callId);
    assertEqual(thirdResult.length, 1, "the third call has exactly one result after recovery");
    const commitsAtCrash = txsOf(sd, C.id).length;
    consoleCli(sd, ["agent", "resume", C.id, "--json"]);
    const prompted = consoleCli(sd, ["agent", "prompt", C.id, "carry", "on", "--json"]);
    assertEqual(prompted.status, 0, `agent prompt (${prompted.stderr})`);
    await waitSession(server, C.id, ["idle", "interrupted", "ended"]).then((c) => assertEqual([c.status, c.statusReason ?? null], ["idle", null], "session C after the resumed run"));
    assertEqual(txsOf(sd, C.id).length, commitsAtCrash, "the commit count is unchanged by the resume");
    const after = nodesOf(sd, C.id);
    const userTurns = after.filter((n) => n.kind === "userMessage");
    assertEqual([userTurns.length, after.at(-1).kind], [2, "assistantMessage"], "the resumed run answered the second prompt");
    const ids = after.filter((n) => n.kind === "toolCall").map((n) => n.callId);
    assertEqual([new Set(ids).size, ids.length], [3, 3], "no tool call ran twice (three distinct calls)");
    const caps = (await ws.call("snapshot.get", { projectId, branch: (await getSession(server, C.id)).rootBranch })).project.timelines.flatMap((t) => t.tracks.flatMap((tr) => tr.items)).filter((i) => i.kind === "caption");
    assertEqual([0, SRT_CUES].includes(caps.length), true, `the SRT was imported at most once (${caps.length} captions)`);
    log(`crash: third call ${thirdResult[0].payload.synthetic ? "was interrupted (synthetic result)" : "had finished"}; ${commitsAtCrash} commit(s) on C's branch`);

    step(6, "fin merge as local: every session tx is merged");
    const merged = person(["merge", root]);
    assertEqual(merged.merge !== undefined || merged.rev !== undefined, true, "the merge answered");
    assertEqual(txsOf(sd, A.id).map((r) => r.outcome), ["merged", "merged", "merged"], "session_txs.outcome after the merge");
    assertEqual(outcomesOf(sd, A.id).every((o) => o.via === "merge" && o.verdict === "merged" && o.by === "user"), true, "outcome rows");
    log("review.verdict is not registered until 5E: no journal entry is checked for it");

    step(7, "a token budget stops session D with BUDGET_EXCEEDED and a budget approval; allowing more finishes it");
    const D = startSession(PROMPTS.budget, ["--budget-tokens", "250"]);
    const stopped = await waitSession(server, D.id, ["interrupted", "waitingApproval"]);
    assertEqual(stopped.statusReason, "BUDGET_EXCEEDED", "session D's stop");
    const approvals = (await consoleRpc(server, "agent.approval.list", { status: "open" }, { confirm: false })).approvals.filter((a) => a.sessionId === D.id);
    assertEqual([approvals.length, approvals[0]?.kind], [1, "budget"], "D's budget approval");
    const shown = consoleCli(sd, ["agent", "show", D.id]);
    assertEqual(shown.stdout.includes(approvals[0].id), true, "agent show names the approval");
    await consoleRpc(server, "agent.approval.answer", { id: approvals[0].id, decision: "allow", add: { tokens: 1_000_000 } });
    const resumedD = await waitSession(server, D.id, ["idle"]);
    assertEqual([resumedD.status, resumedD.usage.input], ["idle", 0], "D is resumed by the approval and waits for work (the stopped call never ran)");
    assertEqual(consoleCli(sd, ["agent", "prompt", D.id, "go", "on", "--json"]).status, 0, "agent prompt after the approval");
    const finished = await waitSession(server, D.id, ["idle", "interrupted", "ended"]);
    assertEqual([finished.status, finished.usage.input > 0], ["idle", true], "D ran on within its raised budget");
    assertEqual(nodesOf(sd, D.id).at(-1).kind, "assistantMessage", "D's run ended with the model's answer");

    step(8, "a second server: training off, no key, an unopenable secret store; the local preset session completes");
    const s2 = await startServer(sd2, { FIN_TRAINING: "off", FIN_DIAGNOSTICS: "on" });
    servers.push(s2);
    // Where `secrets.json` would be there is a directory: reading, writing or listing the store fails.
    if (process.platform !== "win32") mkdirSync(join(sd2, "secrets.json"));
    finOk(sd2, tmp, ["project", "create", project2, "--name", "Film Two"], {});
    await writeFixture(join(project2, "clip.mp4"));
    finOk(sd2, project2, ["asset", "add", "clip.mp4"]);
    finOk(sd2, project2, ["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "3s"]);
    await consoleRpc(s2, "agent.local.set", { endpoint: models.baseUrl });
    const second = consoleCli(sd2, ["agent", "start", "--project", project2, "--provider", "local", "--model", MODEL, "--mode", "draft", "--prompt", PROMPTS.local, "--json"]);
    assertEqual(second.status, 0, `agent start on the second server (${second.stderr})`);
    const E = JSON.parse(second.stdout).session;
    const done2 = await waitSession(s2, E.id, ["idle"]);
    assertEqual([done2.provider, done2.weights, done2.usage.moneyMicros], ["local", "open", 0], "the local session: provider, weights, cost");
    if (process.platform !== "win32") assertEqual([statSync(join(sd2, "secrets.json")).isDirectory(), readdirSync(join(sd2, "secrets.json"))], [true, []], "the secret store's place was never opened or written");
    assertEqual(existsSync(join(sd2, "agent-traces")), false, "training off: no trace store on disk");
    const meta = await waitFor("the second server's agent entries", () => {
      const all = exportEntries(sd2, project2, []).filter((e) => e.kind.startsWith("agent."));
      return all.some((e) => e.kind === "agent.turn") && all.some((e) => e.kind === "agent.session") ? all : undefined;
    }, 15_000).catch(() => undefined);
    const events2 = meta ?? entriesIn(sd2, "events").filter((e) => e.kind.startsWith("agent."));
    const fields = loadContentFields();
    assertEqual(entriesIn(sd2, "events").flatMap((e) => contentProblems(e, fields)), [], "training off: no content in events/");
    const turns2 = events2.filter((e) => e.kind === "agent.turn");
    assertEqual(turns2.length > 0 && turns2.every((e) => e.payload.provider === "local" && ["open", "unknown"].includes(e.payload.weights) && !e.payload.costMicros && e.payload.upstream === undefined), true, "turn metadata: provider local, weights, no cost");
    // The wire form names a model from a table, else "other" with its keyed hash: the raw id is in no record.
    assertEqual(turns2.every((e) => e.payload.model !== MODEL && (e.payload.model !== "other" || String(e.payload.modelHash).startsWith("k1:"))), true, "the model is a table name or `other` with a keyed hash");
    assertEqual(grep(filesIn(join(sd2, "events")), [MODEL]), [], "the raw model id is in no record");
    const port = String(new URL(models.baseUrl).port);
    for (const dir of [sd, sd2]) for (const sub of ["events", "agent-traces"]) assertEqual(grep(filesIn(join(dir, sub)), [`127.0.0.1:${port}`, `localhost:${port}`, models.baseUrl]), [], `no record in ${sub} holds the model server's URL`);

    step(9, "ending a session revokes its credential; the leak grep over files, outputs and model requests");
    // The first watch ended with the server it was killed with (step 5): a new one replays the nodes and ends with the session.
    const lines = watchA.lines().join("\n");
    for (const w of ["toolCall", "toolResult"]) assertEqual(lines.includes(w), true, `agent watch rendered ${w} nodes`);
    watchA.kill();
    const watchEnd = startWatch(sd, A.id);
    await waitFor("the second watch to render the stored nodes", () => watchEnd.lines().some((l) => l.includes("toolResult")), 15_000);
    consoleCli(sd, ["agent", "end", A.id, "--json"]);
    assertEqual(await Promise.race([watchEnd.exited, new Promise((r) => setTimeout(() => r("timeout"), 15_000))]), 0, "agent watch exits when the session ends");
    assertEqual(watchEnd.lines().at(-1).includes("ended"), true, "agent watch's last line is the end status");
    refused(agentFin(A.id, ["whoami"]), 7, "CREDENTIAL_REVOKED", "fin with the ended session's credential (no fallback to local)");
    const secrets = [DUMMY_KEY];
    for (const id of [A.id, C.id, D.id]) {
      const file = credential(id);
      if (existsSync(file)) secrets.push(JSON.parse(readFileSync(file, "utf8")).key);
    }
    for (const name of ["local.key", "console.key"]) secrets.push(readFileSync(join(sd, "auth", `${name}`), "utf8").trim());
    const sources = [
      ...outputs,
      ...models.requests().map((r, i) => ({ name: `model request ${i}`, text: r.raw })),
      ...servers.map((s, i) => ({ name: `finisherd ${i} stderr`, text: s.stderr() })),
      ...[...filesIn(join(sd, "events")), ...filesIn(join(sd, "agent-traces")), ...filesIn(join(sd2, "events"))].map(([name, bytes]) => ({ name, text: bytes.toString("utf8") })),
      { name: "finisherd.json", text: readFileSync(join(sd, "finisherd.json"), "utf8") },
    ];
    assertEqual(grep([["control", Buffer.from(`x${DUMMY_KEY}x`)]], [DUMMY_KEY]).length, 1, "the grep finds a planted key (control)");
    const leaked = sources.flatMap((s) => secrets.filter((n) => n.length > 0 && s.text.includes(n)).map((needle) => `${s.name} holds a secret (${needle.slice(0, 6)}…)`));
    assertEqual(leaked, [], "secrets found outside their files");
    log(`agent scenario passed (${models.requests().length} model requests)`);
  } finally {
    for (const ws of sockets) ws.close();
    key?.close();
    for (const s of servers) await stopServer(s);
    await models?.close();
    for (const k of SWITCHES) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
    removeTmp(tmp);
  }
}
