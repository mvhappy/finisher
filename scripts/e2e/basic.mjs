// The "basic" scenario: fin -> finisherd -> WebSocket subscriber sees every committed rev.
import { mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeFixture } from "./fixture.mjs";
import { assertEqual, connectWs, fin, live, log, removeTmp, startServer, stopServer, waitFor } from "./lib.mjs";

/**
 * Checks that `seen` (the subscriber's revs, in arrival order) is exactly `committed`
 * (the revs `fin` reported, in commit order). Returns an error message or `undefined`.
 */
export function checkRevs(seen, committed) {
  const want = [...committed].sort((a, b) => a - b);
  for (let i = 1; i < want.length; i++) {
    if (want[i] !== want[i - 1] + 1) return `fin's revs are not contiguous: ${JSON.stringify(committed)}`;
  }
  if (JSON.stringify(seen) !== JSON.stringify(want)) {
    return `subscriber saw revs ${JSON.stringify(seen)}, expected ${JSON.stringify(want)} in order`;
  }
  return undefined;
}

export async function run() {
  const started = Date.now();

  // realpath: macOS tmp is a symlink (/var → /private/var), and fin echoes resolved paths.
  const tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisher-e2e-")));
  live.tmp = tmp;
  const serverData = join(tmp, "server-data");
  let server;
  let ws;
  try {
    server = await startServer(serverData);
    const project = join(tmp, "мій фільм");
    const created = fin(serverData, tmp, ["project", "create", project, "--name", "E2E"]);
    const projectId = created.project.projectId;
    assertEqual(created.project.rev, 0, "new project rev");

    ws = await connectWs(server.info);
    const seen = [];
    // With the generation it read rev 0 at: a rewind or rebuild in between ends the stream.
    const { generation } = await ws.call("snapshot.get", { projectId, branch: "main" });
    const sub = ws.subscribe({ projectId, branch: "main", fromRev: 0, generation }, (tx) => seen.push(tx));
    let subExit;
    sub.done.then((exit) => (subExit = exit));

    await writeFixture(join(project, "кліп 1.mp4"));
    const writes = [
      ["asset", "add", "кліп 1.mp4"],
      ["clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"],
      ["clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--in", "4s", "--dur", "3s"],
      ["clip", "trim", "c1", "--end", "-1s"],
      ["clip", "delete", "c1"],
    ];
    const committed = writes.map((args) => fin(serverData, project, args));
    const view = fin(serverData, project, ["view"]);
    assertEqual(
      view.tracks,
      [
        // Each insert also placed its linked audio clip (plan 4C Decision 21); the trim and the
        // delete of c1 acted on its link group (c1 + c2).
        { id: "t1", kind: "video", clips: [{ id: "c3", start: "0", end: "3", asset: "a1", sourceIn: "4" }] },
        { id: "t2", kind: "audio", clips: [{ id: "c4", start: "0", end: "3", asset: "a1", sourceIn: "4" }] },
      ],
      "fin view tracks",
    );

    const revs = committed.map((w) => w.rev);
    await waitFor(
      `${revs.length} revs on the subscriber`,
      () => {
        if (subExit) throw new Error(`subscription ended early: ${JSON.stringify(subExit)}`);
        return seen.length >= revs.length;
      },
      15_000,
      () => ` (saw ${JSON.stringify(seen.map((t) => t.rev))})`,
    );
    // Anything duplicated or late would arrive within a short grace period.
    await new Promise((r) => setTimeout(r, 300));
    const problem = checkRevs(
      seen.map((t) => t.rev),
      revs,
    );
    if (problem) throw new Error(problem);
    assertEqual(
      seen.map((t) => t.tx.txId),
      committed.map((w) => w.txId),
      "subscriber txIds",
    );

    const snapshot = await ws.call("snapshot.get", { projectId, branch: "main" });
    assertEqual(snapshot.rev, seen.at(-1).rev, "snapshot.get rev vs last CommittedTx.rev");

    ws.interrupt(sub.id);
    await sub.done;
    log(`subscriber saw revs ${JSON.stringify(seen.map((t) => t.rev))}; snapshot.get rev ${snapshot.rev}`);
    log(`PASS in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    ws?.close();
    if (server) await stopServer(server);
    removeTmp(tmp);
  }
}

