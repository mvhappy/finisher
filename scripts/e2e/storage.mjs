// The "storage" scenario: edit, export, delete the database, rebuild from the folder, and a
// second server following the same folder read-only.
import { existsSync, mkdtempSync, readdirSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertEqual, connectWs, fin, finRaw, live, log, removeTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { writeFixture } from "./fixture.mjs";

/** The model of a `snapshot.get` answer: without the generation and incarnation, which a rebuild changes. */
function model(snapshot) {
  const { generation, incarnation, ...rest } = snapshot;
  return rest;
}

const step = (n, text) => log(`step ${n}: ${text}`);

export async function run() {
  const started = Date.now();
  const tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisher-e2e-storage-")));
  live.tmp = tmp;
  const dataA = join(tmp, "server-a");
  const dataB = join(tmp, "server-b");
  const project = join(tmp, "storage project");
  const servers = [];
  const sockets = [];
  try {
    let a = await startServer(dataA);
    servers.push(a);
    const created = fin(dataA, tmp, ["project", "create", project, "--name", "Storage"]);
    const projectId = created.project.projectId;

    step(1, "five writes, then wait for the export to catch up");
    await writeFixture(join(project, "clip.mp4"));
    const writes = [
      ["asset", "add", "clip.mp4"],
      ["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"],
      ["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur", "3s"],
      ["clip", "trim", "c1", "--end", "-1s"],
      ["clip", "delete", "c1"],
    ];
    const committed = writes.map((args) => fin(dataA, project, args));
    const head = committed.at(-1).rev;
    const mainBranch = (st) => st.branches.find((b) => b.branch === "main");
    const exported = await pollStatus(dataA, project, (st) => mainBranch(st).exportedThrough === mainBranch(st).head, 10_000);
    assertEqual(mainBranch(exported).head, head, "status head");
    assertEqual(mainBranch(exported).exportedThrough, head, "status exportedThrough");

    let ws = await connectWs(a.info);
    sockets.push(ws);
    const before = await ws.call("snapshot.get", { projectId, branch: "main" });
    assertEqual(before.rev, head, "snapshot rev before delete");
    ws.close();

    step(2, "stop finisherd, delete project.fdb*");
    await stopServer(a);
    const dbDir = join(dataA, "projects", projectId);
    const dbFiles = readdirSync(dbDir).filter((f) => f.startsWith("project.fdb"));
    if (!dbFiles.includes("project.fdb")) throw new Error(`no project.fdb in ${dbDir}: ${readdirSync(dbDir)}`);
    for (const f of dbFiles) rmSync(join(dbDir, f), { force: true });
    assertEqual(existsSync(join(dbDir, "project.fdb")), false, "database removed");
    log(`deleted ${dbFiles.join(", ")}`);

    step(3, "restart, open the project, compare the snapshot");
    a = await startServer(dataA);
    servers.push(a);
    fin(dataA, project, ["project", "open", project]);
    ws = await connectWs(a.info);
    sockets.push(ws);
    const after = await ws.call("snapshot.get", { projectId, branch: "main" });
    assertEqual(model(after), model(before), "snapshot after rebuild");
    log(`snapshot after rebuild equals the one before (rev ${after.rev}); generation ${JSON.stringify(before.generation)} → ${JSON.stringify(after.generation)}`);

    step(4, "fin project verify");
    const verified = fin(dataA, project, ["project", "verify"]);
    assertEqual(verified.report.ok, true, "verify ok");

    step(5, "a second finisherd follows the same folder");
    const b = await startServer(dataB);
    servers.push(b);
    fin(dataB, project, ["project", "open", project]);
    const follower = fin(dataB, project, ["project", "status"]).status;
    assertEqual(follower.readOnly?.reason, "LEASE_HELD", "second server readOnly.reason");
    const refused = finRaw(dataB, project, ["clip", "insert", "--track", "t2", "--at", "0", "--asset", "a1", "--dur", "1s"]);
    assertEqual(refused.status, 4, "commit through the read-only server exits 4");

    const wsB = await connectWs(b.info);
    sockets.push(wsB);
    const snapB = await wsB.call("snapshot.get", { projectId, branch: "main" });
    assertEqual(snapB.rev, head, "follower snapshot rev");
    const seen = [];
    const sub = wsB.subscribe({ projectId, branch: "main", fromRev: snapB.rev, generation: snapB.generation }, (tx) => seen.push(tx));
    let subExit;
    sub.done.then((exit) => (subExit = exit));
    const edit = fin(dataA, project, ["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s"]);
    await waitFor(
      "the follower to stream the edit made through the first server",
      () => {
        if (subExit) throw new Error(`follower subscription ended early: ${JSON.stringify(subExit)}`);
        return seen.some((t) => t.rev === edit.rev);
      },
      5_000,
      () => ` (saw ${JSON.stringify(seen.map((t) => t.rev))}, want ${edit.rev})`,
    );
    assertEqual(seen.find((t) => t.rev === edit.rev).tx.txId, edit.txId, "follower txId");
    wsB.interrupt(sub.id);
    log(`PASS storage in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const ws of sockets) ws.close();
    for (const s of servers.reverse()) await stopServer(s);
    removeTmp(tmp);
  }
}

/** Polls `fin project status` until `ok(status)`; fails after `timeoutMs`. */
async function pollStatus(serverData, cwd, ok, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const { status } = fin(serverData, cwd, ["project", "status"]);
    if (ok(status)) return status;
    if (Date.now() > deadline) throw new Error(`status never satisfied the condition: ${JSON.stringify(status)}`);
    await new Promise((r) => setTimeout(r, 500));
  }
}
