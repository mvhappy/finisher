// The "sandbox" scenario (plan 7C Task 14): agent processes and warm workers under fin-sandbox, on
// a real `finisherd` with the test-hooks fin-node (`FIN_TEST_HOOKS=1`, the `agent` and `worker`
// profiles made ready by `FIN_TEST_SANDBOX_READY`, as Task 16 will make them by default).
//   1. a session of the scripted `fake` harness starts from the console; its turn runs a shell
//      script through the session sandbox (`spec.sandbox.spawn`: on Linux `fin-sandbox-exec`, the
//      subreaper and Landlock), the stand-in for a CLI agent's process until 7D/7E;
//   2. from inside it: `fin item trim` lands on the session's branch over the OS's transport
//      (Linux: the agent listener, `tcp:`), `fin whoami` is the agent; `cat auth/local.key`, a
//      write into the project folder and one outside every root fail; a write in
//      `workspace/<sessionId>/` succeeds; `fin caption import --words /etc/hosts` is
//      `PATH_NOT_ALLOWED`; `main` is untouched;
//   3. `server.status` carries no `before7C` and no `SANDBOX_DISABLED` for the agent profile;
//   4. (Linux, with a GPU adapter) a `media.look` in each of two projects runs warm under the
//      `worker` profile (its `media.capabilities` entry says `sandbox: landlock`): one warm worker per project,
//      each seeing only the jobs that visit it (Decision 19; the denial itself is proven by
//      `warm-sandbox.test.ts` and `warm_landlock.rs` with their controls).
// Skipped where the agent profile cannot be enforced (Windows: plan 7C Task 11's Windows run is
// open; a host without Landlock), unless `FIN_REQUIRE_LANDLOCK=1`.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { getSession, nodesOf, txsOf, waitSession } from "./agent-lib.mjs";
import { TEST_NATIVE_DIR } from "./fixture.mjs";
import { FIN, ROOT, assertEqual, connectWs, consoleRpc, fin, finRaw, live, log, removeTmp, scenarioTmp, startServer, stopServer, storedEntries, waitFor } from "./lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
/** A `server.status` warning's profile and reason (`details`). */
const about = (w) => ({ profile: w.details?.profile, reason: w.details?.reason });
const q = (s) => `'${s.replaceAll("'", "'\\''")}'`;

/** The children the session's turn runs, in order (`sh -c`, in `workspace/<sessionId>/`). */
export function children({ serverData, project, outside, track }) {
  const f = q(FIN);
  return {
    trim: `${f} item trim c1 --end -0.5s`,
    whoami: `${f} whoami; echo "FIN_SERVER=$FIN_SERVER"`,
    key: `cat ${q(join(serverData, "auth", "local.key"))}`,
    project: `echo planted > ${q(join(project, "planted.txt"))}`,
    outside: `echo planted > ${q(outside)}`,
    workspace: "echo hi > ok.txt && cat ok.txt",
    hosts: `${f} caption import --track ${track} --words /etc/hosts --style preset:bold`,
  };
}

/** The fake harness's script: one turn, a child per entry, then the end of the turn. */
export function script(kids) {
  return [
    {
      name: "sandbox",
      turns: [[...Object.values(kids).map((c) => ({ spawn: ["/bin/sh", "-c", c] })), { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] }]],
    },
  ];
}

/** The `child` notices of a session's nodes, by the name of the child that made each. */
export function childResults(nodes, names) {
  const results = nodes.filter((n) => n.kind === "notice" && n.payload?.child !== undefined).map((n) => n.payload.child);
  if (results.length !== names.length) throw new Error(`expected ${names.length} children, saw ${results.length}: ${JSON.stringify(results)}`);
  return Object.fromEntries(names.map((name, i) => [name, results[i]]));
}

/** A small AV1 clip (the media worker decodes AV1 in software everywhere): the look scenario's fixture writer. */
function av1Clip(path) {
  mkdirSync(dirname(path), { recursive: true });
  const args = ["run", "-q", "--release", "-p", "fin-codecs", "--features", "testing", "--example", "make_fixture", "--", path, "--size", "320x180", "--rate", "30", "--frames", "30", "--seed", "3"];
  const r = spawnSync("cargo", args, { cwd: ROOT, encoding: "utf8", maxBuffer: 16 << 20 });
  if (r.status !== 0) throw new Error(`make_fixture failed (exit ${r.status}): ${r.stderr}`);
}

/** Throws unless the child failed with an OS denial. */
function deniedChild(c, what) {
  if (c.code === 0 || !/Permission denied/.test(c.stderr)) throw new Error(`${what}: expected a denial, got ${JSON.stringify(c)}`);
}

export async function run() {
  const required = process.env.FIN_REQUIRE_LANDLOCK === "1";
  if (process.platform === "win32") {
    if (required) throw new Error("FIN_REQUIRE_LANDLOCK=1 on Windows");
    log("SKIP sandbox: the Windows session sandbox run is open (plan 7C Task 11)");
    return;
  }
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-sandbox-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "film");
  const other = join(tmp, "other");
  const outside = join(tmp, "outside.txt");
  let server;
  try {
    step(0, "server (test hooks: the fake harness, agent and worker profiles ready), two projects");
    mkdirSync(sd, { recursive: true, mode: 0o700 });
    writeFileSync(join(sd, "settings.json"), `${JSON.stringify({ agents: { finPath: FIN }, fonts: { autoFetch: false } })}\n`);
    // The caption track the children name: a new project has t1 (video) and t2 (audio).
    const kids = children({ serverData: sd, project, outside, track: "t3" });
    const scripts = join(tmp, "fake-scripts.json");
    writeFileSync(scripts, JSON.stringify(script(kids)));
    server = await startServer(sd, {
      FIN_TEST_HOOKS: "1",
      FIN_NODE_BINARY_DIR: TEST_NATIVE_DIR,
      FIN_TEST_SANDBOX_READY: "agent,worker",
      FIN_TEST_FAKE_SCRIPTS: scripts,
    });
    const status = await consoleRpc(server, "server.status", {}, { confirm: false });
    const agentWarnings = (status.warnings ?? []).filter((w) => about(w).profile === "agent" || about(w).reason === "before7C");
    // Only the host may excuse the run (Landlock disabled or too old); anything else that leaves
    // the agent unconfined (`profileNotReady`, `before7C`) is a failure: the hooks made it ready.
    const host = agentWarnings.filter((w) => about(w).reason === "landlockDisabled" || w.code === "SANDBOX_UNAVAILABLE");
    if (host.length > 0) {
      if (required) throw new Error(`FIN_REQUIRE_LANDLOCK=1 but the agent profile is not enforced: ${JSON.stringify(host)}`);
      log(`SKIP sandbox: this host cannot enforce the agent profile (${JSON.stringify(host)})`);
      return;
    }
    const unconfined = agentWarnings.filter((w) => w.code === "SANDBOX_DISABLED");
    assertEqual(unconfined, [], "server.status: the agent profile's warnings at boot");
    const person = (dir, args) => fin(sd, dir, args);
    const projectId = person(tmp, ["project", "create", project, "--name", "Film"]).project.projectId;
    av1Clip(join(project, "sources", "clip.mp4"));
    person(project, ["asset", "add", "sources/clip.mp4"]);
    person(project, ["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s"]);
    assertEqual(person(project, ["track", "add", "--kind", "caption"]).bindings.$track, "t3", "the caption track");
    const otherId = person(tmp, ["project", "create", other, "--name", "Other"]).project.projectId;
    av1Clip(join(other, "sources", "clip.mp4"));
    person(other, ["asset", "add", "sources/clip.mp4"]);
    person(other, ["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s"]);
    const ws = await connectWs(server);
    const mainOf = async () => JSON.stringify((await ws.call("snapshot.get", { projectId, branch: "main" })).project);
    const mainBefore = await mainOf();

    step(1, "a fake session from the console; its turn runs the children through the session sandbox");
    const start = await consoleRpc(server, "agent.session.start", { projectId, harness: "fake", prompt: "go" });
    const id = start.session.id;
    const codes = (start.warnings ?? []).map((w) => w.code);
    assertEqual(codes.filter((c) => c !== "SANDBOX_SOCKETS_EXPOSED"), [], "the session's sandbox warnings (only exposed sockets may remain)");
    const s = await waitSession(server, id, ["idle", "interrupted", "ended"]);
    assertEqual(s.status, "idle", `session ${id} after its turn`);
    const c = childResults(nodesOf(sd, id), Object.keys(kids));

    step(2, "inside: fin over the session's transport, no key, writes only in the workspace");
    assertEqual(c.trim.code, 0, `fin item trim (${c.trim.stdout} ${c.trim.stderr})`);
    assertEqual(c.whoami.code, 0, `fin whoami (${c.whoami.stderr})`);
    if (!/"kind":"agent"/.test(c.whoami.stdout)) throw new Error(`fin whoami is not the agent: ${c.whoami.stdout}`);
    const transport = /FIN_SERVER=(\S+)/.exec(c.whoami.stdout)?.[1] ?? "";
    if (process.platform === "linux" ? !/^tcp:127\.0\.0\.1:\d+$/.test(transport) : transport !== server.info.socket) {
      throw new Error(`the session's transport on ${process.platform}: ${transport}`);
    }
    deniedChild(c.key, "cat auth/local.key");
    if (c.key.stdout.length > 0) throw new Error("the key's bytes reached the child");
    deniedChild(c.project, "a write into the project folder");
    deniedChild(c.outside, "a write outside every root");
    assertEqual([existsSync(join(project, "planted.txt")), existsSync(outside)], [false, false], "nothing planted");
    assertEqual([c.workspace.code, c.workspace.stdout], [0, "hi\n"], "a write in the workspace");
    assertEqual(existsSync(join(project, "workspace", id, "ok.txt")), true, "the workspace file");
    if (c.hosts.code === 0 || !/PATH_NOT_ALLOWED/.test(c.hosts.stdout)) throw new Error(`caption import --words /etc/hosts: ${JSON.stringify(c.hosts)}`);
    const txs = txsOf(sd, id);
    assertEqual(txs.length, 1, "the session's commits (the trim)");
    const branch = (await getSession(server, id)).rootBranch;
    const onBranch = await ws.call("snapshot.get", { projectId, branch });
    if (JSON.stringify(onBranch.project) === mainBefore) throw new Error("the trim did not land on the session's branch");
    assertEqual(await mainOf(), mainBefore, "main untouched");

    step(3, "server.status: no before7C, the agent profile not disabled");
    const after = await consoleRpc(server, "server.status", {}, { confirm: false });
    const stale = (after.warnings ?? []).filter((w) => about(w).reason === "before7C" || (w.code === "SANDBOX_DISABLED" && about(w).profile === "agent"));
    // The hook profile is not made ready, so its warning is the proof the filter reads `details`.
    if (!(after.warnings ?? []).some((w) => w.code === "SANDBOX_DISABLED" && about(w).profile === "hook")) throw new Error(`server.status: no hook warning to compare with: ${JSON.stringify(after.warnings)}`);
    assertEqual(stale, [], "server.status warnings");

    if (process.platform === "linux") {
      step(4, "a warm look per project under the worker profile");
      await warmLooks(server, sd, [project, other], [projectId, otherId]);
    }
    ws.close?.();
    log(`PASS sandbox in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    try {
      if (server) await stopServer(server);
    } finally {
      removeTmp(tmp);
    }
  }
}

/** One `fin look` in each project: both warm, each in its own project's warm worker, sandboxed. */
async function warmLooks(server, sd, dirs, ids) {
  const caps = fin(sd, dirs[0], ["media", "caps"]);
  if (!caps.gpu) {
    if (process.env.FIN_REQUIRE_GPU === "1") throw new Error("FIN_REQUIRE_GPU=1 but the media worker opened no GPU adapter");
    log("SKIP sandbox step 4: no GPU adapter (set FIN_GPU, or FIN_REQUIRE_GPU=1 to fail)");
    return;
  }
  // The worker's own answer, journalled as `media.capabilities` (sp6 §15): its Landlock phase applied.
  const answered = await waitFor(
    "the media.capabilities entry",
    async () => (await storedEntries(server, { kinds: ["media.capabilities"] }))[0],
    20_000,
  );
  assertEqual(answered.payload.sandbox, "landlock", "media.capabilities: the worker restricted itself");
  const workers = new Set();
  const jobs = join(sd, "jobs");
  for (const dir of dirs) {
    const r = finRaw(sd, dir, ["look", "--at", "0"]);
    assertEqual(r.status, 0, `fin look in ${dir} (${r.text} ${r.stderr})`);
    for (const w of readdirSync(jobs).filter((n) => n.startsWith("warm-"))) workers.add(w);
  }
  const done = await waitFor(
    "two media.look job.done entries",
    async () => {
      const looks = (await storedEntries(server, { kinds: ["job.done"] })).filter((e) => e.payload.kind === "media.look");
      return looks.length >= dirs.length ? looks : undefined;
    },
    20_000,
  );
  assertEqual(done.map((e) => [e.payload.outcome, e.payload.warm]), dirs.map(() => ["succeeded", true]), "the looks ran warm");
  // One warm worker per project (the key names the project under an applied profile), so a
  // worker never holds another project's job; its directory holds only the job visiting it.
  if (workers.size < ids.length) throw new Error(`expected a warm worker per project, saw ${[...workers].join(", ")}`);
  for (const w of workers) {
    const visiting = existsSync(join(jobs, w, "jobs")) ? readdirSync(join(jobs, w, "jobs")) : [];
    assertEqual(visiting, [], `${w}: no job left in the worker's directory after ready`);
  }
}
