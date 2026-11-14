// The "claude" scenario (7D Task 16): Claude Code sessions from the console, on the release
// `finisherd` and `fin` with `fake-claude` as the CLI (`agents.claudeCode.path`, the setting on).
// Nothing reaches the network and no real sign-in is touched. The fake plays one script per CLI
// process (`claude-script.mjs`), and runs its commands through the CLI's `Bash` tool, so every
// edit is a `fin` call the runtime has to allow.
//   1. `finisherd agent providers` shows Claude Code installed and signed in; `finisherd provider
//      signin claudeCode` prints the sign-in command and runs nothing (no CLI process, nothing under
//      `providers/`);
//   2. a person adds `ask tool:Bash(curl *)` and starts an `editor` session A on `claudeCode`; the
//      fake edits through `fin` on A's branch (`main` untouched, commits `agent:editor:<id>`,
//      attributed to their calls) and asks for `curl`: the person answers from the console and the
//      call proceeds;
//   3. a steer reaches the CLI as the next turn; an interrupt ends a held turn;
//   4. `finisherd` is killed while a shell call holds its commit open; the account label changes
//      meanwhile; after the restart A is `interrupted`, resumes (a resume is never refused) and the
//      call the CLI asks again is refused: no second commit;
//   5. the account change refuses the next start (`PROVIDER_ACCOUNT_CHANGED`, in `agent providers`
//      too) until the person confirms it; session B then starts;
//   6. B's token budget stops it mid-turn (`BUDGET_EXCEEDED`, a `budget` approval); allowed, it runs on;
//   7. ending A revokes its credential (`CREDENTIAL_REVOKED`, no fallback to `local`); the leak grep.
// On Linux it runs once more with the `agent` profile made ready by `FIN_TEST_SANDBOX_READY=agent`
// (the test-hooks fin-node): the CLI then runs under the launcher, `fin` and read-only calls pass by
// policy, and only the rule's `curl` and the budget open cards.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { getSession, nodesOf, startWatch, txsOf, waitSession } from "./agent-lib.mjs";
import { consoleCli } from "./auth-lib.mjs";
import { FAKE_CLAUDE, attend, cardsOf, installFake, listing, signIn, subjectText, until, userTexts } from "./claude-lib.mjs";
import { CURL, LABELS, PROMPTS, sessionA, sessionACrash, sessionAResumed, sessionB, sessionBMore } from "./claude-script.mjs";
import { TEST_NATIVE_DIR } from "./fixture.mjs";
import { FIN, assertEqual, connectWs, consoleRpc, finRaw, live, log, outputs, removeTmp, scenarioTmp, startServer, stopServer, waitFor, fin as finOk } from "./lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const FIRST = "first.account@example.invalid";
const SECOND = "second.account@example.invalid";

/** Throws unless `r` is a `fin` refusal with this exit status and error code. */
function refused(r, status, code, what) {
  if (r.status !== status || r.json?.error?.code !== code) throw new Error(`${what}: expected exit ${status} ${code}, got exit ${r.status}: ${r.text} ${r.stderr}`);
  return r.json.error;
}

export async function run() {
  if (process.platform === "win32") {
    log("SKIP claude: the fake CLI runs its commands through /bin/sh");
    return;
  }
  if (!existsSync(FAKE_CLAUDE)) throw new Error(`fake-claude is not built (${FAKE_CLAUDE}): cargo build --release -p fin-agent-fakes --bin fake-claude`);
  await scenario("direct", {});
  if (process.platform !== "linux") return;
  const launcher = { FIN_TEST_HOOKS: "1", FIN_NODE_BINARY_DIR: TEST_NATIVE_DIR, FIN_TEST_SANDBOX_READY: "agent" };
  await scenario("launcher", launcher);
}

/** One whole run of the scenario on a server of its own; `env` is what the server starts with. */
async function scenario(mode, env) {
  const started = Date.now();
  const direct = mode === "direct";
  log(`claude scenario, ${mode} mode`);
  const tmp = scenarioTmp("finisher-e2e-claude-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const project = join(tmp, "film");
  const servers = [];
  const sockets = [];
  const watches = [];
  try {
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    const fake = installFake(join(tmp, "cli"), { script: sessionA() });
    signIn(sd, FIRST);
    writeFileSync(join(sd, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN, claudeCode: { enabled: true, path: fake.binary } }, fonts: { autoFetch: false } })}\n`);
    let server = await startServer(sd, env);
    servers.push(server);
    if (!direct) {
      const status = await consoleRpc(server, "server.status", {}, { confirm: false });
      const host = (status.warnings ?? []).filter((w) => w.code === "SANDBOX_UNAVAILABLE" || w.details?.reason === "landlockDisabled");
      if (host.length > 0) {
        if (process.env.FIN_REQUIRE_LANDLOCK === "1") throw new Error(`FIN_REQUIRE_LANDLOCK=1 but the agent profile is not enforced: ${JSON.stringify(host)}`);
        log(`SKIP claude launcher run: this host cannot enforce the agent profile (${JSON.stringify(host)})`);
        return;
      }
      assertEqual((status.warnings ?? []).filter((w) => w.code === "SANDBOX_DISABLED" && w.details?.profile === "agent"), [], "the agent profile is enforced");
    }
    const person = (args, cwd = project) => finOk(sd, cwd, args, {});
    const projectId = person(["project", "create", project, "--name", "Film"], tmp).project.projectId;
    let ws = await connectWs(server);
    sockets.push(ws);
    const mainContent = async () => JSON.stringify((await ws.call("snapshot.get", { projectId, branch: "main" })).project);
    const mainBefore = await mainContent();

    step(1, "agent providers shows Claude Code installed and signed in; provider signin prints and runs nothing");
    const providers = consoleCli(sd, ["agent", "providers", "--json"]);
    assertEqual(providers.status, 0, `agent providers (${providers.stderr})`);
    const line = JSON.parse(providers.stdout).providers.find((p) => p.harness === "claudeCode");
    assertEqual([line.detected, line.version, line.signedIn, line.enabled, line.problem ?? null], [true, "2.1.321", true, true, null], "the claudeCode line of agent providers");
    const text = consoleCli(sd, ["agent", "providers"]).stdout;
    if (!/claudeCode\s+available\s+2\.1\.321\s+signed in/.test(text)) throw new Error(`agent providers (text): ${text}`);
    const processesBefore = fake.invocations();
    // Under the launcher the CLI's host runs are sandboxed and cannot write the wrapper's log: only direct mode counts them.
    if (direct) assertEqual(processesBefore > 0, true, "agent providers ran the CLI (the invocation count works)");
    const providersBefore = listing(join(sd, "providers"));
    const signin = consoleCli(sd, ["provider", "signin", "claudeCode"]);
    assertEqual(signin.status, 0, `provider signin (${signin.stderr})`);
    if (!/claude/.test(signin.stdout) || !/login/.test(signin.stdout)) throw new Error(`provider signin printed no sign-in command: ${signin.stdout}`);
    assertEqual([fake.invocations(), listing(join(sd, "providers"))], [processesBefore, providersBefore], "provider signin ran no CLI process and wrote nothing under providers/");

    step(2, "a rule asks for curl; an editor session on claudeCode edits through fin on its branch; the person answers curl");
    const { rule } = await consoleRpc(server, "agent.rules.add", { effect: "ask", command: "tool:Bash(curl *)", projectId });
    assertEqual([rule.effect, rule.command], ["ask", "tool:Bash(curl *)"], "the person's rule");
    const startA = consoleCli(sd, ["agent", "start", "--project", project, "--harness", "claudeCode", "--role", "editor", "--mode", "draft", "--title", "intro", "--prompt", PROMPTS.work, "--json"]);
    assertEqual(startA.status, 0, `agent start (${startA.stderr})`);
    const A = JSON.parse(startA.stdout).session;
    assertEqual([A.harness, A.role, A.provider], ["claudeCode", "editor", "anthropic"], "session A");
    const records = () => fake.records(sd, A.id);
    const watchA = startWatch(sd, A.id);
    watches.push(watchA);
    // Every card but the curl one is the person's quick yes (direct mode: each shell call asks); curl is answered by hand.
    const isCurl = (card) => subjectText(card).includes("curl");
    const attendA = attend(server, A.id, (card) => !isCurl(card));
    const open = await until("an open curl card", async () => (await cardsOf(server, A.id, "open")).find(isCurl), 60_000);
    log(`the curl card: ${open.kind} ${subjectText(open).slice(0, 120)}`);
    assertEqual((await getSession(server, A.id)).status, "waitingApproval", "A waits on the person while the curl call is asked");
    assertEqual(txsOf(sd, A.id).length, 1, "the intro marker is committed before curl is asked");
    await consoleRpc(server, "agent.approval.answer", { id: open.id, decision: "allow" });
    await waitFor("the curl call's result", () => nodesOf(sd, A.id).filter((n) => n.kind === "toolResult").length >= 2, 30_000);
    const results = nodesOf(sd, A.id).filter((n) => n.kind === "toolResult");
    assertEqual(results.map((n) => n.payload.ok), [true, true], "both calls of turn 1 succeeded (curl proceeded)");
    const cardsSeen = [...attendA.answered, open];
    assertEqual(cardsSeen.filter(isCurl).length, 1, "exactly one curl card");
    if (!direct) assertEqual(cardsSeen.length, 1, "launcher mode: fin passes by policy, only curl asked");
    log(`${direct ? "direct mode: every shell call asked" : "launcher mode: only curl asked"} (${cardsSeen.length} card(s))`);

    step(3, "a steer reaches the CLI; an interrupt ends a held turn");
    const steer = consoleCli(sd, ["agent", "steer", A.id, PROMPTS.steer, "--json"]);
    assertEqual(steer.status, 0, `agent steer (${steer.stderr})`);
    await waitFor("A idle after the steer's turn", () => nodesOf(sd, A.id).filter((n) => n.kind === "toolResult").length >= 3 && txsOf(sd, A.id).length === 2, 60_000, () => ` (${txsOf(sd, A.id).length} commits)`);
    const idleA = await waitSession(server, A.id, ["idle"]);
    assertEqual(idleA.status, "idle", "A after the steered turn");
    const root = (await getSession(server, A.id)).rootBranch;
    const rows = txsOf(sd, A.id);
    assertEqual(rows.map((r) => r.attribution), ["inferred", "inferred"], "A's commits are attributed to their shell calls");
    assertEqual(rows.every((r) => r.callId !== null), true, "each commit names its call");
    for (const r of rows) {
      const got = (await ws.call("tx.get", { projectId, txId: r.txId })).tx;
      assertEqual([got.author, got.branch], [`agent:editor:${A.id}`, root], `tx ${r.txId}: the editor's commit on the session branch`);
    }
    assertEqual(await mainContent(), mainBefore, "main is untouched while A edits");
    const steered = nodesOf(sd, A.id).filter((n) => n.kind === "userMessage");
    assertEqual(steered.some((n) => n.payload.trigger === "steer"), true, "the steer is a userMessage node with trigger steer");
    assertEqual(userTexts(records()).some((t) => t.includes(PROMPTS.steer)), true, "the CLI received the steer's text");
    consoleCli(sd, ["agent", "prompt", A.id, PROMPTS.hold, "--json"]);
    await waitFor("the held turn at the CLI", () => userTexts(records()).some((t) => t.includes(PROMPTS.hold)), 30_000);
    const t0 = Date.now();
    const interrupt = consoleCli(sd, ["agent", "interrupt", A.id, "--json"]);
    assertEqual(interrupt.status, 0, `agent interrupt (${interrupt.stderr})`);
    const after = await waitSession(server, A.id, ["idle", "interrupted"]);
    log(`interrupt: ${after.status} ${after.statusReason ?? ""} in ${Date.now() - t0} ms`);
    if (Date.now() - t0 > 20_000) throw new Error("the interrupt took as long as the turn it ended");
    assertEqual(records().at(-1).controlRequests.some((c) => c.subtype === "interrupt"), true, "the CLI received an interrupt request");

    step(4, "finisherd is killed while a call holds its commit open; the account changes; restart and resume: no second commit");
    assertEqual([after.status, after.statusReason], ["interrupted", "userInterrupt"], "A after the person's interrupt");
    // The CLI process ended with the interrupt; the next one resumes its transcript and plays the script as it is then.
    fake.setScript(sessionACrash());
    assertEqual(consoleCli(sd, ["agent", "resume", A.id, "--json"]).status, 0, "agent resume after the interrupt");
    assertEqual(consoleCli(sd, ["agent", "prompt", A.id, PROMPTS.crash, "--json"]).status, 0, "agent prompt after the interrupt");
    await waitFor("the credits marker committed", () => txsOf(sd, A.id).length === 3, 60_000, () => ` ${JSON.stringify({ procs: records().map((r) => ({ argv: r.argv.filter((a) => a.startsWith("--resume") || a.startsWith("--session")), users: (r.userLines ?? []).length, spawned: r.spawned })), nodes: nodesOf(sd, A.id).slice(-8).map((n) => [n.kind, JSON.stringify(n.payload).slice(0, 150)]) })}`);
    await attendA.stop();
    const commits = txsOf(sd, A.id).map((t) => t.txId);
    server.child.kill("SIGKILL");
    await server.exited;
    fake.setScript(sessionAResumed());
    signIn(sd, SECOND);
    server = await startServer(sd, env);
    servers.push(server);
    ws = await connectWs(server);
    sockets.push(ws);
    const hurt = await getSession(server, A.id);
    assertEqual([hurt.status, hurt.statusReason], ["interrupted", "serverRestart"], "A after the restart");
    assertEqual(txsOf(sd, A.id).map((t) => t.txId), commits, "the restart added no commit");
    assertEqual(consoleCli(sd, ["agent", "resume", A.id, "--json"]).status, 0, "agent resume (a resume is never refused for an account change)");
    const carried = consoleCli(sd, ["agent", "prompt", A.id, PROMPTS.carryOn, "--json"]);
    assertEqual(carried.status, 0, `agent prompt after the restart (${carried.stderr})`);
    await waitFor("the resumed process to start", () => records().length >= 3 && records().at(-1).resume !== undefined, 60_000);
    await waitSession(server, A.id, ["idle", "interrupted"]).then((s) => assertEqual([s.status, s.statusReason ?? null], ["idle", null], "A after the resumed turn"));
    const resumed = records().at(-1);
    assertEqual(resumed.argv.some((a) => a.startsWith("--resume")), true, "the second process resumed the CLI's transcript");
    assertEqual([resumed.resume.found, resumed.resume.dangling.length], [true, 1], "the CLI found the call it left open");
    assertEqual([resumed.reran, (resumed.rerunDenied ?? []).length], [undefined, 1], "the CLI asked the open call again and was refused");
    assertEqual(txsOf(sd, A.id).map((t) => t.txId), commits, "no repeated commit after the resume");

    step(5, "the account change refuses the next start until the person confirms it");
    const changed = JSON.parse(consoleCli(sd, ["agent", "providers", "--json"]).stdout).providers.find((p) => p.harness === "claudeCode");
    assertEqual(changed.problem?.code, "PROVIDER_ACCOUNT_CHANGED", "agent providers names the change");
    const refusedStart = consoleCli(sd, ["agent", "start", "--project", project, "--harness", "claudeCode", "--budget-tokens", "1000", "--prompt", PROMPTS.budget, "--json"]);
    assertEqual([refusedStart.status, refusedStart.code], [5, "PROVIDER_ACCOUNT_CHANGED"], "a new start after the change");
    assertEqual(typeof changed.accountHash, "string", "the line shows the account's hash, never its label");
    await consoleRpc(server, "agent.account.confirm", { harness: "claudeCode", accountHash: changed.accountHash });
    const clear = JSON.parse(consoleCli(sd, ["agent", "providers", "--json"]).stdout).providers.find((p) => p.harness === "claudeCode");
    assertEqual(clear.problem, undefined, "agent providers after the confirmation");

    step(6, "a budget stop opens an approval; allowed, the session runs on");
    fake.setScript(sessionB());
    const startB = consoleCli(sd, ["agent", "start", "--project", project, "--harness", "claudeCode", "--budget-tokens", "1000", "--prompt", PROMPTS.budget, "--json"]);
    assertEqual(startB.status, 0, `agent start B (${startB.stderr})`);
    const B = JSON.parse(startB.stdout).session;
    const stopped = await waitSession(server, B.id, ["interrupted", "waitingApproval"]);
    assertEqual(stopped.statusReason, "BUDGET_EXCEEDED", "session B's stop");
    const budgetCard = await until("B's budget card", async () => (await cardsOf(server, B.id, "open")).find((c) => c.kind === "budget"), 30_000);
    assertEqual(consoleCli(sd, ["agent", "show", B.id]).stdout.includes(budgetCard.id), true, "agent show names the approval");
    await consoleRpc(server, "agent.approval.answer", { id: budgetCard.id, decision: "allow", add: { tokens: 1_000_000 } });
    fake.setScript(sessionBMore()); // the stop closed B's CLI: the next process resumes its transcript
    const attendB = attend(server, B.id);
    const resumedB = await waitSession(server, B.id, ["idle"]);
    assertEqual(resumedB.status, "idle", "B waits for work after the approval");
    assertEqual(consoleCli(sd, ["agent", "prompt", B.id, PROMPTS.budgetMore, "--json"]).status, 0, "agent prompt after the approval");
    await waitFor("B's next turn", () => nodesOf(sd, B.id).some((n) => n.kind === "toolResult" && n.payload.ok === true), 60_000, () => ` ${JSON.stringify(nodesOf(sd, B.id).slice(-6).map((n) => [n.kind, JSON.stringify(n.payload).slice(0, 120)]))}`);
    await attendB.stop();
    assertEqual((await waitSession(server, B.id, ["idle"])).status, "idle", "B after its turn within the raised budget");
    assertEqual(txsOf(sd, B.id).length, 1, "B's one marker is committed on its own branch");

    step(7, "ending a session revokes its credential; the leak grep");
    const credential = (id) => join(sd, "agents", id, "credential.json");
    const agentEnv = (id) => ({ FIN_CREDENTIAL: credential(id), FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: project });
    const keyA = existsSync(credential(A.id)) ? JSON.parse(readFileSync(credential(A.id), "utf8")).key : undefined;
    const agentFin = (id, args) => finRaw(sd, join(project, "workspace", id), args, agentEnv(id));
    assertEqual(agentFin(A.id, ["whoami"]).json?.principal?.kind, "agent", "A's credential works while A lives");
    const watchEnd = startWatch(sd, A.id);
    watches.push(watchEnd);
    consoleCli(sd, ["agent", "end", A.id, "--json"]);
    consoleCli(sd, ["agent", "end", B.id, "--json"]);
    assertEqual(await Promise.race([watchEnd.exited, new Promise((r) => setTimeout(() => r("timeout"), 15_000))]), 0, "agent watch exits when the session ends");
    refused(agentFin(A.id, ["whoami"]), 7, "CREDENTIAL_REVOKED", "fin with the ended session's credential");
    assertEqual(await mainContent(), mainBefore, "main's content is untouched by every session");
    const secrets = [keyA, ...["local.key", "console.key"].map((n) => readFileSync(join(sd, "auth", n), "utf8").trim())];
    const leaked = [...outputs, ...servers.map((s, i) => ({ name: `finisherd ${i} stderr`, text: s.stderr() }))].flatMap((s) => secrets.filter((n) => n && s.text.includes(n)).map(() => `${s.name} holds a secret`));
    assertEqual(leaked, [], "secrets found in outputs");
    const labels = [...outputs, ...servers.map((s, i) => ({ name: `finisherd ${i} stderr`, text: s.stderr() }))].filter((s) => s.text.includes(FIRST) || s.text.includes(SECOND)).map((s) => s.name);
    assertEqual(labels, [], "the account label is in no console or fin output nor server log");
    log(`claude scenario (${mode}) passed in ${((Date.now() - started) / 1000).toFixed(1)} s`);
  } finally {
    for (const w of watches) w.kill();
    for (const s of sockets) s.close();
    for (const s of servers) await stopServer(s);
    removeTmp(tmp);
  }
}
