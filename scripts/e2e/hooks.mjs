// The "hooks" scenario (7F plan Task 11): hooks end to end on a real `finisherd` with the
// test-hooks fin-node (`FIN_TEST_HOOKS=1`), sessions of the scripted `fake` harness whose turns run
// `fin item trim` (the stand-in for a CLI agent's tool call), the project's `finisher.hooks.json`
// (a `PreCommand` hook blocking `item.trim`, a `UserPrompt` hook adding context) and a user-layer
// `PreMerge` hook in `settings.json`. See STEPS (hooks-lib.mjs) for the order:
//   1. untrusted, the trim lands, the prompt carries no context, the start warns
//      `PROJECT_HOOKS_UNTRUSTED`; 2. `agent.hooks.list` shows the hooks, trust binds their hash;
//   3. the trim is `HOOK_BLOCKED` (exit 7) with one `hook.block` row; 4. the context is a
//      `notice {hookContext}` in `agent.watch`; 5. a person's `fin merge` is refused by `PreMerge`;
//   6. editing the file turns the block off until trusted again; 7. `server.status.hooks`.
// On Linux, with the `hook` profile ready (`FIN_TEST_SANDBOX_READY=hook`), the user-layer probe hook
// runs under the launcher: its write into the project folder fails and the profile is not disabled.
// Observed through that probe and `server.status`, not a journal entry: the engine journals no
// hook run. Skipped as to the launcher where the host cannot enforce the profile, unless
// `FIN_REQUIRE_LANDLOCK=1`.
import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { getSession, nodesOf, startWatch, waitSession } from "./agent-lib.mjs";
import { TEST_NATIVE_DIR, writeFixture } from "./fixture.mjs";
import { auditOf, branchState, childrenOf, contextsOf, hookStats, projectHooks, script, STEPS, trustProject, userHooks } from "./hooks-lib.mjs";
import { FIN, ROOT, assertEqual, connectWs, consoleRpc, fin, finRaw, live, log, removeTmp, scenarioTmp, startServer, stopServer } from "./lib.mjs";

export { STEPS };

const step = (n) => log(`step ${n}: ${STEPS[n - 1]}`);
const HOOK = join(ROOT, "scripts", "e2e", "fixtures", "hooks", "hook.mjs");
const CONTEXT = "use the house style";
const need = (ok, what) => {
  if (!ok) throw new Error(what);
};

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-hooks-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "film");
  const linux = process.platform === "linux";
  const hooksFile = join(project, "finisher.hooks.json");
  let server;
  let watch;
  let ws;
  try {
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    copyFileSync(HOOK, join(sd, "hook.mjs"));
    writeFileSync(join(sd, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN }, fonts: { autoFetch: false }, hooks: userHooks({ project, probe: linux }) })}\n`);
    const scripts = join(tmp, "fake-scripts.json");
    writeFileSync(scripts, JSON.stringify(script(4)));
    server = await startServer(sd, {
      FIN_TEST_HOOKS: "1",
      FIN_NODE_BINARY_DIR: TEST_NATIVE_DIR,
      FIN_TEST_FAKE_SCRIPTS: scripts,
      ...(linux ? { FIN_TEST_SANDBOX_READY: "hook" } : {}),
    });
    // The launcher: enforced unless the host cannot (Landlock disabled or too old).
    const warnings = ((await consoleRpc(server, "server.status", {}, { confirm: false })).warnings ?? []).filter((w) => w.details?.profile === "hook");
    // SANDBOX_SOCKETS_EXPOSED only says the sockets are not hidden: Landlock still confines the files.
    const unenforced = warnings.filter((w) => w.code !== "SANDBOX_SOCKETS_EXPOSED");
    // Only a host limit excuses the launcher checks; any other disabled state (`profileNotReady`
    // with the profile made ready by FIN_TEST_SANDBOX_READY) is a regression.
    const hostLimit = (w) => w.code === "SANDBOX_UNAVAILABLE" || w.details?.reason === "landlockDisabled";
    const regressions = unenforced.filter((w) => !hostLimit(w));
    need(regressions.length === 0, `the hook profile is not enforced for a reason other than the host: ${JSON.stringify(regressions)}`);
    const enforced = linux && unenforced.length === 0;
    if (linux && !enforced) {
      need(process.env.FIN_REQUIRE_LANDLOCK !== "1", `FIN_REQUIRE_LANDLOCK=1 but the hook profile is not enforced: ${JSON.stringify(unenforced)}`);
      log(`SKIP hooks under the launcher: the hook profile is not enforced here (${JSON.stringify(unenforced)})`);
    }

    const created = fin(sd, tmp, ["project", "create", project, "--name", "Film"]);
    const projectId = created.project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    fin(sd, project, ["asset", "add", "clip.mp4"]);
    fin(sd, project, ["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"]);
    copyFileSync(HOOK, join(project, "hook.mjs"));
    const writeHooks = (extra = {}) => writeFileSync(hooksFile, `${JSON.stringify({ ...projectHooks({ context: CONTEXT }), ...extra }, null, 2)}\n`);
    writeHooks();

    step(1);
    ws = await connectWs(server);
    // The session's branch forks from main: the first trim must move it away from main's state.
    const beforeFirst = await branchState(ws, projectId, "main");
    const start = await consoleRpc(server, "agent.session.start", { projectId, harness: "fake", prompt: "one" });
    const id = start.session.id;
    need((start.warnings ?? []).some((w) => w.code === "PROJECT_HOOKS_UNTRUSTED"), `agent.session.start did not warn PROJECT_HOOKS_UNTRUSTED: ${JSON.stringify(start.warnings)}`);
    assertEqual((await waitSession(server, id, ["idle", "interrupted", "ended"])).status, "idle", "session after turn 1");
    const first = childrenOf(nodesOf(sd, id));
    assertEqual([first.length, first[0]?.code], [1, 0], `the first trim lands (${first[0]?.stdout})`);
    const own = (nodes) => contextsOf(nodes).filter((c) => c.hooks.includes("style"));
    assertEqual(own(nodesOf(sd, id)), [], "no context from the untrusted project hook");
    const ranBefore = (await hookStats(server)).events;
    assertEqual([ranBefore.PreCommand.runs, ranBefore.UserPrompt.runs], [0, linux ? 1 : 0], "no project hook ran (only the user layer's probe)");
    const branch = (await getSession(server, id)).rootBranch;
    const state = () => branchState(ws, projectId, branch);
    const afterFirst = await state();
    need(afterFirst !== beforeFirst, "the first trim changed neither the revision nor the clip");
    const mainBefore = JSON.stringify(fin(sd, project, ["view"]));

    step(2);
    const { listed, trusted } = await trustProject(server, projectId);
    const shown = listed.hooks.filter((h) => h.layer === "project");
    assertEqual(shown.map((h) => [h.id, h.event, h.applies]), [["no-trim", "PreCommand", false], ["style", "UserPrompt", false]], "agent.hooks.list shows the project's hooks before trust");
    need(typeof listed.hooksHash === "string" && listed.hooksHash !== "none" && trusted.hooksHash === listed.hooksHash, `trust answered ${JSON.stringify(trusted)}`);
    assertEqual((await consoleRpc(server, "agent.hooks.list", { projectId }, { confirm: false })).projectHooks, "trusted", "the hooks file is trusted");

    step(3);
    watch = startWatch(sd, id);
    await consoleRpc(server, "agent.session.prompt", { sessionId: id, text: "two" });
    await waitTurn(server, sd, id, 2);
    const second = childrenOf(nodesOf(sd, id))[1];
    assertEqual([second.code, second.error], [7, "HOOK_BLOCKED"], `the trim after trust: ${second.stdout} ${JSON.stringify(nodesOf(sd, id).filter((n) => n.kind === "notice" && n.payload?.hook !== undefined).map((n) => n.payload))}`);
    assertEqual(await state(), afterFirst, "the blocked trim left the session branch unchanged");
    need(second.reason?.includes("trims are frozen"), `the block's reason: ${second.reason}`);
    // The blocking project hook itself tries to write the project folder: denied under the launcher.
    if (enforced) need(/\[write=(EACCES|EPERM)\]/.test(second.reason), `the project hook's write into the project folder: ${second.reason}`);
    const blocks = (await auditOf(server, "hook.block")).filter((r) => r.details.event === "PreCommand");
    assertEqual(blocks.length, 1, "one PreCommand hook.block row");
    assertEqual([blocks[0].details.hookId, blocks[0].details.layer, blocks[0].details.session, blocks[0].details.projectId], ["no-trim", "project", id, projectId], "the row's details");
    need(!JSON.stringify(blocks[0]).includes("frozen"), "the row holds the reason");
    assertEqual((await hookStats(server)).events.PreCommand.runs, 1, "the blocking hook ran once");

    step(4);
    // The user layer's probe adds its own context first (joined with a blank line).
    const styled = own(nodesOf(sd, id));
    assertEqual([styled.length, styled[0]?.event, styled[0]?.text.endsWith(CONTEXT)], [1, "UserPrompt", true], `the context reached the session as a notice: ${JSON.stringify(styled)}`);
    if (enforced) {
      const probe = contextsOf(nodesOf(sd, id)).filter((c) => c.hooks.includes("probe"));
      need(probe.length === 2, `the probe ran for two prompts: ${JSON.stringify(probe)}`);
      for (const c of probe) assertEqual(JSON.parse(c.text.split("\n\n")[0]).write, "EACCES", "a hook's write into the project folder under the launcher");
      assertEqual(existsSync(join(project, "planted.txt")), false, "nothing planted in the project");
      assertEqual((await hookStats(server)).events.UserPrompt.failures, 0, "the sandboxed hooks did not fail");
    } else rmSync(join(project, "planted.txt"), { force: true });

    step(5);
    const merge = finRaw(sd, project, ["merge", branch]);
    assertEqual([merge.status, merge.json?.error?.code], [7, "HOOK_BLOCKED"], `fin merge: ${merge.text}`);
    assertEqual(JSON.stringify(fin(sd, project, ["view"])), mainBefore, "main is untouched");
    assertEqual((await auditOf(server, "hook.block")).filter((r) => r.details.event === "PreMerge").length, 1, "one PreMerge hook.block row");

    step(6);
    writeHooks({ note: "edited" });
    assertEqual((await consoleRpc(server, "agent.hooks.list", { projectId }, { confirm: false })).projectHooks, "changed", "the edited hooks file is no longer trusted");
    await consoleRpc(server, "agent.session.prompt", { sessionId: id, text: "three" });
    await waitTurn(server, sd, id, 3);
    const third = childrenOf(nodesOf(sd, id))[2];
    assertEqual(third.code, 0, `the trim after the edit lands (${third.stdout})`);
    const afterThird = await state();
    need(afterThird !== afterFirst, "the trim after the edit changed neither the revision nor the clip");
    assertEqual(own(nodesOf(sd, id)).length, 1, "no new context while untrusted");
    await trustProject(server, projectId);
    await consoleRpc(server, "agent.session.prompt", { sessionId: id, text: "four" });
    await waitTurn(server, sd, id, 4);
    const fourth = childrenOf(nodesOf(sd, id))[3];
    assertEqual([fourth.code, fourth.error], [7, "HOOK_BLOCKED"], `the trim after trusting again: ${fourth.stdout}`);
    assertEqual(await state(), afterThird, "the blocked trim left the session branch unchanged");

    step(7);
    const stats = (await hookStats(server)).events;
    // PreCommand: the two trims after trust. UserPrompt: the style hook for prompts two and four, the probe for each of the four.
    assertEqual([stats.PreCommand.runs, stats.PreCommand.blocks], [2, 2], "PreCommand counters");
    assertEqual([stats.PreMerge.runs, stats.PreMerge.blocks], [1, 1], "PreMerge counters");
    assertEqual(stats.UserPrompt.runs, 2 + (linux ? 4 : 0), "UserPrompt runs");
    assertEqual(stats.UserPrompt.blocks, 0, "UserPrompt blocks");

    await consoleRpc(server, "agent.session.end", { sessionId: id });
    need((await Promise.race([watch.exited, new Promise((r) => setTimeout(() => r("timeout"), 15_000))])) === 0, "agent watch did not exit when the session ended");
    need(watch.lines().join("\n").includes(CONTEXT), "agent watch did not render the hook context");
    log(`PASS hooks in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    try {
      watch?.kill();
      ws?.close?.();
      if (server) await stopServer(server);
    } finally {
      removeTmp(tmp);
    }
  }
}

/** Waits until the session's `n`-th turn has run its child and the session is idle again. */
async function waitTurn(server, sd, id, n) {
  const deadline = Date.now() + 60_000;
  for (;;) {
    if (childrenOf(nodesOf(sd, id)).length >= n && (await getSession(server, id)).status === "idle") return;
    if (Date.now() > deadline) throw new Error(`timed out waiting for turn ${n} of ${id}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}
