// The "plugin-code" scenario (plan 8B Task 18 with its 2026-11-09 authority-isolation
// amendment): code plugins end to end over a real finisherd, `fin` and `fin-plugin`, with both
// SDK examples and the test probe published into a local registry made with generated keys.
// finisherd runs through `plugin-code-server.mjs` (`serve` with the egress broker's test hooks;
// fin-node's `test-hooks` build). Nothing touches the network.
//   1. a registry with publishers `example` and `e2e`; the console adds it by its root's hash;
//   2. a person plans and installs the TS example (`thirdPartyCode {confined: false}`, accepted),
//      the Rust one and the probe; the owner enables them; `server.status` counts them under
//      `PLUGINS_UNCONFINED` (none before);
//   3. `fin @example/ts-tools trim-silence --dry-run`, then for real: the commit's author is the
//      invoker and `via` names the plugin and command (`tx.get`); the Rust example's too;
//   4. `fetch-title` through the broker to the loopback origin, then with the name resolving to
//      a private address: `EGRESS_PRIVATE_ADDRESS`;
//   5. an agent session on another project cannot reach the first (`PROJECT_NOT_FOUND`);
//   6. authority isolation: the person's marker (memory, temp, pluginData) is invisible to the
//      agent session and to its child, still the person's afterwards; the child's start bumps
//      the tree's generation and retires the root's older process, temp and pluginData (the root
//      then sees none of its own marker); the session's end does the same for the new ones;
//   7. the probe's process killed during three runs: `crashLoop` and its audit row;
//   8. disable with work running and queued: the queued run fails `PLUGIN_DISABLED`;
//   9. `PLUGINS_UNCONFINED` counts what is still enabled and active;
//  10. the journal holds `plugin.command`, `plugin.exit` and `worker.exit` kv 2 with a keyed
//      `plugin`, and no plugin id, path or stderr text.
// Not here: `PLUGIN_TREE_CHANGED` (Decision 16: no store re-hash before 7C) and the enforced
// profile's denials (Task 16).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { FIN, ROOT, assertEqual, consoleCall, consoleRpc, errorCode, fin, finRaw, live, log, removeTmp, scenarioTmp, socketCall, startServer, stopServer, storedEntries, waitFor } from "./lib.mjs";
import { filesUnder, leaks } from "./diagnostics-lib.mjs";
import { auditRows, makeRegistry } from "./plugins-lib.mjs";
import { finAsync } from "./auth-lib.mjs";
import { waitSession } from "./agent-lib.mjs";
import { TEST_NATIVE_DIR, writeFixture } from "./fixture.mjs";
import { API, PLATFORM, TITLE, alive, examplesRunHere, filesHolding, stageExamples, stageProbe, startOrigin } from "./plugin-code-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const SWITCHES = ["FIN_DIAGNOSTICS", "FIN_TRAINING", "DO_NOT_TRACK"];
const STDERR_TEXT = "probe-stderr-e2e-marker-text";
const need = (cond, what) => {
  if (!cond) throw new Error(what);
};
/** The error code of a failed `fin` run; throws when it succeeded. */
const failed = (r, what) => {
  if (r.status === 0) throw new Error(`${what}: expected a failure, got ${r.text}`);
  return r.json?.error?.code ?? errorCode(`${r.text}${r.stderr}`);
};
/** Polls the async `predicate` every 100 ms until it holds; throws after `ms`. */
async function until(what, predicate, ms) {
  const deadline = Date.now() + ms;
  while (!(await predicate())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

export async function run() {
  if (!examplesRunHere()) {
    log(`SKIP plugin-code: the example plugins do not list ${PLATFORM}`);
    return;
  }
  const started = Date.now();
  const saved = Object.fromEntries(SWITCHES.map((k) => [k, process.env[k]]));
  for (const k of SWITCHES) delete process.env[k];
  const tmp = scenarioTmp("finisher-e2e-pcode-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const work = join(tmp, "w");
  const film = join(tmp, "film");
  const other = join(tmp, "other");
  const resolveFile = join(tmp, "resolve.json");
  mkdirSync(work, { recursive: true });
  mkdirSync(sd, { recursive: true, mode: 0o700 });
  writeFileSync(join(sd, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN }, fonts: { autoFetch: false } })}\n`);
  const resolveTo = (addresses) => writeFileSync(resolveFile, JSON.stringify(addresses));
  resolveTo(["127.0.0.1"]);
  // The fake harness: every run waits for an answer that never comes, so the root session can
  // spawn (children are spawned from a run) and its child stays live while it runs commands.
  const scripts = join(tmp, "fake-scripts.json");
  writeFileSync(scripts, JSON.stringify([{ name: "hold", turns: [[{ awaitAnswer: "hold" }]] }]));
  let server;
  let origin;
  try {
    origin = await startOrigin();
    const { ts, rust } = await stageExamples(work);
    const probe = stageProbe(work);
    server = await startServer(
      sd,
      { FIN_TEST_HOOKS: "1", FIN_TEST_FAKE_SCRIPTS: scripts, FIN_NODE_BINARY_DIR: TEST_NATIVE_DIR, FIN_E2E_EGRESS: JSON.stringify({ host: API, port: origin.port, ca: origin.ca, resolveFile }) },
      { entry: join(ROOT, "scripts", "e2e", "plugin-code-server.mjs") },
    );
    const console_ = (tag, payload) => socketCall(server, tag, payload, "console");
    const asPerson = (tag, payload) => consoleCall(server, tag, payload);
    const row = async (id) => (await console_("plugin.list", {})).plugins.find((p) => p.id === id);
    const pluginWarnings = async () => (await consoleRpc(server, "server.status", {}, { confirm: false })).warnings.filter((w) => w.details?.profile === "plugin");
    const filmId = fin(sd, tmp, ["project", "create", film, "--name", "Film"]).project.projectId;
    const otherId = fin(sd, tmp, ["project", "create", other, "--name", "Other"]).project.projectId;
    const f = (args, env) => fin(sd, film, args, env);
    const raw = (dir, args, env) => finRaw(sd, dir, args, env);

    step(1, "a registry by `fin-plugin` with generated keys; the console adds it by its root's hash");
    const regDir = join(work, "registry");
    const reg = makeRegistry(work, "code", regDir, ["example", "e2e"]);
    for (const p of [ts, rust, probe]) reg.publishDir(p.src, p.id);
    await asPerson("registry.add", { cfg: { id: "code", path: regDir }, root: { kind: "sha256", sha256: reg.rootSha256 } });
    assertEqual((await console_("registry.refresh", { id: "code" })).results.map((r) => r.ok), [true], "the refresh");

    step(2, "a person installs the code plugins (third-party code accepted); the owner enables them");
    assertEqual(await pluginWarnings(), [], "server.status with no code plugin enabled");
    for (const p of [ts, rust, probe]) {
      const plan = await console_("plugin.plan", { id: p.id });
      const warning = plan.warnings.find((w) => w.code === "thirdPartyCode");
      assertEqual([plan.warnings[0]?.code, warning?.details?.confined], ["thirdPartyCode", false], `${p.id}'s plan warning`);
      const done = await asPerson("plugin.install", { planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } });
      assertEqual([done.plugin.kind, done.plugin.enabled], ["code", false], `${p.id} installed switched off`);
      const on = await asPerson("plugin.enable", { id: p.id });
      assertEqual([on.plugin.enabled, on.plugin.state], [true, "active"], `${p.id} enabled`);
      assertEqual((await row(p.id)).confined, false, `${p.id} unconfined`);
    }
    const unconfined = await pluginWarnings();
    assertEqual(unconfined.map((w) => [w.code, w.details.enabled]), [["PLUGINS_UNCONFINED", 3]], "server.status counts the enabled code plugins");

    step(3, "trim-silence: a dry run, then a commit by the invoker with `via`");
    await writeFixture(join(film, "clip.mp4"), 6);
    f(["asset", "add", "clip.mp4"]);
    f(["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s"]);
    // Unlinked: the Rust example assumes unlinked items (a group's delete takes its members).
    f(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "2s", "--dur", "2s"]);
    f(["track", "set", "t2", "mute", "true"]);
    const head = async () => (await socketCall(server, "snapshot.get", { projectId: filmId, branch: "main" })).rev;
    const before = await head();
    const dry = f([ts.id, "trim-silence", "--dry-run"]);
    assertEqual([dry.outcome, dry.dryRun, dry.ops.map((o) => o.op)], ["data", true, ["item.delete"]], "the dry run");
    assertEqual(await head(), before, "a dry run commits nothing");
    // The Rust example lifts the items under a `silence` marker (the second clip); the TS one
    // deletes the muted audio (with its linked video: a link group goes whole).
    f(["marker", "add", "--at", "2s", "--dur", "2s", "--label", "silence"]);
    const rustRun = f([rust.id, "trim-silence", "--label", "silence", "--mode", "lift"]);
    assertEqual([rustRun.outcome, rustRun.ops?.length ?? null], ["accepted", null], "the Rust example's commit");
    const rustTx = await socketCall(server, "tx.get", { projectId: filmId, txId: rustRun.txId });
    assertEqual([rustTx.tx.author, rustTx.tx.via, rustTx.tx.ops.length], ["local:local", { plugin: `${rust.id}@0.1.0`, command: "trim-silence" }, 1], "the Rust commit");
    const trimmed = f([ts.id, "trim-silence"]);
    assertEqual(trimmed.outcome, "accepted", "the TS example's commit");
    const tx = await socketCall(server, "tx.get", { projectId: filmId, txId: trimmed.txId });
    assertEqual([tx.tx.author, tx.tx.via], ["local:local", { plugin: `${ts.id}@0.1.0`, command: "trim-silence" }], "the commit's author and via");
    assertEqual(f(["view"]).lines.filter((l) => / clip /.test(l)), [], "no clip is left on the timeline");

    step(4, "fetch-title through the broker; a private answer for the name is refused");
    // Asynchronously: the origin is served by this process, which a synchronous `fin` would block.
    const ok = async (args) => {
      const r = await finAsync(FIN, sd, film, args);
      need(r.status === 0, `fin ${args.join(" ")}: ${r.text} ${r.stderr}`);
      return r.json;
    };
    const page = await ok([ts.id, "fetch-title", "--url", `https://${API}/`]);
    assertEqual([page.outcome, page.data], ["data", { status: 200, title: TITLE }], "the TS example's fetch");
    const rustPage = await ok([rust.id, "fetch-title", "--path", "/"]);
    assertEqual(rustPage.outcome, "data", "the Rust example's fetch");
    need(JSON.stringify(rustPage.data).includes(TITLE), `the Rust example's title: ${JSON.stringify(rustPage.data)}`);
    resolveTo(["10.1.2.3"]);
    assertEqual(failed(raw(film, [ts.id, "fetch-title", "--url", `https://${API}/`]), "a private address"), "EGRESS_PRIVATE_ADDRESS", "a name resolving to a private address");
    resolveTo(["127.0.0.1"]);

    step(5, "an agent session on another project cannot reach the first");
    const session = (await consoleRpc(server, "agent.session.start", { projectId: otherId, harness: "fake", prompt: "go" })).session;
    await waitSession(server, session.id, ["running"]); // its run waits: children are spawned from a run
    const agentEnv = (id) => ({ FIN_CREDENTIAL: join(sd, "agents", id, "credential.json"), FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: other });
    const asAgent = (id, args) => raw(join(other, "workspace", id), args, agentEnv(id));
    mkdirSync(join(other, "workspace", session.id), { recursive: true });
    assertEqual(failed(asAgent(session.id, [ts.id, "trim-silence", "--project", filmId, "--dry-run"]), "the agent on Film"), "PROJECT_NOT_FOUND", "an agent of Other on Film");

    step(6, "authority isolation: memory, temp and pluginData are each caller's own");
    const marker = (out, what) => {
      need(out.status === 0, `${what}: ${out.text} ${out.stderr}`);
      return out.json.data;
    };
    const asPersonMarker = (set) => marker(raw(other, [probe.id, "run", "--op", "marker", ...(set ? ["--set", set] : []), "--say", STDERR_TEXT]), `the person's marker ${set ?? ""}`);
    const nothing = { memory: null, tmp: null, data: null, hasData: true };
    const first = asPersonMarker("person-one");
    assertEqual({ ...first, pid: undefined }, { ...nothing, pid: undefined }, "the person's first run sees nothing");
    const agentSaw = marker(asAgent(session.id, [probe.id, "run", "--op", "marker", "--set", "agent-one"]), "the agent's marker");
    assertEqual({ ...agentSaw, pid: undefined }, { ...nothing, pid: undefined }, "the agent sees none of the person's marker");
    need(agentSaw.pid !== first.pid, "the agent ran in the person's process");
    const generation = (cls) => {
      const db = new DatabaseSync(join(sd, "control.db"), { readOnly: true });
      try {
        return Number(db.prepare("SELECT generation FROM plugin_authority WHERE class = ?").get(cls)?.generation ?? 0);
      } finally {
        db.close();
      }
    };
    const tree = `agent:${session.id}`;
    const markerFiles = (text) => filesHolding(join(sd, "plugins"), text);
    assertEqual(markerFiles("agent-one").length, 2, "the agent's temp and pluginData markers");
    // A child's start changes the session tree: the tree's generation goes up, and the root's
    // older-generation process, temp and pluginData are retired (the as-built stand-in for a
    // narrowed credential, which is never narrowed in place).
    const spawnedAt = generation(tree);
    const child = asAgent(session.id, ["agent", "spawn", "--role", "fixer", "--brief", "look at the cut"]);
    need(child.status === 0, `fin agent spawn: ${child.text} ${child.stderr}`);
    const childId = child.json.session.id ?? child.json.session;
    await waitSession(server, childId, ["running"]);
    need(generation(tree) > spawnedAt, "the child's start did not bump its tree's generation");
    await waitFor("the root's older-generation process, temp and pluginData to go", () => !alive(agentSaw.pid) && markerFiles("agent-one").length === 0, 70_000, () => `: alive ${alive(agentSaw.pid)}, files ${JSON.stringify(markerFiles("agent-one"))}`, 100);
    mkdirSync(join(other, "workspace", childId), { recursive: true });
    const childSaw = marker(asAgent(childId, [probe.id, "run", "--op", "marker"]), "the child's marker");
    assertEqual({ ...childSaw, pid: undefined }, { ...nothing, pid: undefined }, "the child sees none of the person's marker");
    need(![first.pid, agentSaw.pid].includes(childSaw.pid), "the child ran in another caller's process");
    // The root again, under the new generation: nothing of its retired state, a process of its own.
    const rootAgain = marker(asAgent(session.id, [probe.id, "run", "--op", "marker", "--set", "agent-two"]), "the root's marker after the bump");
    assertEqual({ ...rootAgain, pid: undefined }, { ...nothing, pid: undefined }, "the root's retired memory, temp and pluginData are gone");
    need(![first.pid, agentSaw.pid, childSaw.pid].includes(rootAgain.pid), "the root ran in a retired or another caller's process");
    assertEqual(markerFiles("agent-two").length, 2, "the root's new temp and pluginData markers");
    const again = asPersonMarker();
    assertEqual([again.memory, again.tmp, again.data], ["person-one", "person-one", "person-one"], "the person still sees its own marker");
    const endedAt = generation(tree);
    await consoleCall(server, "agent.session.end", { sessionId: session.id });
    await waitFor("the agent's process, temp and pluginData to go", () => !alive(rootAgain.pid) && markerFiles("agent-two").length === 0, 70_000, () => `: alive ${alive(rootAgain.pid)}, files ${JSON.stringify(markerFiles("agent-two"))}`, 100);
    need(generation(tree) > endedAt, "the session's end did not bump its tree's generation");
    assertEqual(markerFiles("person-one").length, 2, "the person's markers stay");
    need(alive(again.pid), "the person's process was retired with the agent's");

    step(7, "the probe killed during three runs: crash loop and its audit row");
    for (let i = 1; i <= 3; i++) {
      const pid = asPersonMarker().pid;
      const running = finAsync(FIN, sd, other, [probe.id, "run", "--op", "command", "--sleep-ms", "30000"]);
      await until(`run ${i} to start`, async () => (await socketCall(server, "jobs.list", { kind: "plugin.command", state: ["running"] })).jobs.length > 0, 20_000);
      process.kill(pid, "SIGKILL");
      const r = await running;
      assertEqual(failed(r, `killed run ${i}`), "PLUGIN_CRASHED", `killed run ${i}`);
    }
    const looped = await row(probe.id);
    assertEqual([looped.state, looped.problem], ["crashLoop", "PLUGIN_CRASH_LOOP"], "the crash-looped probe");
    need((await auditRows(server, "plugin.crashLoop")).some((r) => r.details.pluginId === probe.id), "no plugin.crashLoop audit row");

    step(8, "disable with work running and queued: the queued run fails PLUGIN_DISABLED");
    const slow = () => finAsync(FIN, sd, film, [ts.id, "fetch-title", "--url", `https://${API}/slow`]);
    const commandJobs = async (state) => (await socketCall(server, "jobs.list", { kind: "plugin.command", state: [state] })).jobs.length;
    const running = slow();
    await until("a run waiting on the origin", async () => (await commandJobs("running")) === 1 && origin.held() === 1, 20_000);
    // The owner pauses the queue, so the next run stays queued behind it.
    await console_("jobs.pause", { all: true });
    const queued = slow();
    await until("a queued run", async () => (await commandJobs("queued")) === 1, 20_000);
    await asPerson("plugin.disable", { id: ts.id });
    const [ranCode, queuedCode] = [failed(await running, "the running run"), failed(await queued, "the queued run")];
    await console_("jobs.resume", { all: true });
    assertEqual([ranCode, queuedCode], ["JOB_CANCELLED", "PLUGIN_DISABLED"], "the disabled plugin's running and queued runs");
    assertEqual(failed(raw(film, [ts.id, "fetch-title", "--url", `https://${API}/`]), "a disabled plugin's command"), "PLUGIN_NOT_FOUND", "a disabled plugin's command");
    log("no PLUGIN_TREE_CHANGED step: Decision 16 (no store re-hash before 7C)");

    step(9, "PLUGINS_UNCONFINED counts what is still enabled and active");
    assertEqual((await pluginWarnings()).map((w) => [w.code, w.details.enabled]), [["PLUGINS_UNCONFINED", 1]], "server.status after the disable and the crash loop");

    step(10, "the journal: plugin.command, plugin.exit, worker.exit kv 2 with a keyed plugin; no free text");
    const entries = await storedEntries(server);
    const kinds = new Set(entries.map((e) => e.kind));
    for (const k of ["plugin.command", "plugin.exit", "worker.exit"]) need(kinds.has(k), `no ${k} entry in the journal`);
    const exits = entries.filter((e) => e.kind === "worker.exit" && e.payload?.plugin !== undefined);
    need(exits.length > 0 && exits.every((e) => e.kv === 2), `worker.exit entries of plugins: ${JSON.stringify(exits.slice(0, 2))}`);
    const keyed = (ref) => JSON.stringify(ref).includes("hash") && !JSON.stringify(ref).includes("@");
    need(exits.every((e) => keyed(e.payload.plugin)), `a worker.exit's plugin is not keyed: ${JSON.stringify(exits[0]?.payload.plugin)}`);
    need(entries.filter((e) => e.kind === "plugin.exit" || e.kind === "plugin.command").every((e) => keyed(e.payload.plugin)), "a plugin entry names its plugin unkeyed");
    const needles = ["@example", "ts-tools", "rust-tools", "@e2e", "e2e/probe", "person-one", "agent-one", "agent-two", STDERR_TEXT, "silent items", TITLE, API, tmp, sd, work];
    const found = leaks(filesUnder(join(sd, "events")), needles);
    need(found.length === 0, `free text in the journal: ${JSON.stringify(found.slice(0, 3))}`);
    need(!needles.some((n) => JSON.stringify(entries).includes(n)), "free text in the stored entries");
    log(`plugin-code: ok in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    if (server) await stopServer(server);
    await origin?.close();
    for (const k of SWITCHES) if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
    removeTmp(tmp);
  }
}
