// Parts of the "history" scenario that run after the main flow: dry runs, idempotency keys and
// a real `project recover --create`.
import { cpSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { assertEqual, connectWs, fin, log, startServer, waitFor } from "./lib.mjs";
import { clipsOf, failure, watch } from "./history-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const settle = () => new Promise((r) => setTimeout(r, 300));

/** Every write command with --dry-run answers `dryRun: true` and changes and publishes nothing. */
export async function dryRuns({ f, raw, snap, mainWatch, insert }) {
  const probe = async () => ({
    branches: f(["branch", "ls", "--all"]).branches,
    main: (await snap("main")).rev,
    status: f(["status"]).rev,
  });
  f(["branch", "new", "dry-src"]);
  const own = insert(["--branch", "dry-src"]);
  const keyed = f(["--idem-key", "e2e-merge-key", "merge", "dry-src"]).merge; // a real merge to revert
  f(["checkpoint", "dry-cp"]);
  f(["branch", "new", "dry-b"]);
  const other = insert(["--branch", "dry-b"]);
  insert([]);
  await settle();
  const before = await probe();
  const seenBefore = mainWatch.seen.length;
  const cases = [
    ["branch", "new", "fresh"],
    ["branch", "new", "fresh", "--from", "main@dry-cp"],
    ["branch", "rename", "dry-b", "renamed"],
    ["branch", "archive", "dry-b"],
    ["checkpoint", "never"],
    ["undo"],
    ["undo", "--to", "dry-cp"],
    ["merge", "dry-b"],
    ["merge", "main", "--into", "dry-b"],
    ["cherry-pick", other.txId],
    ["revert", keyed.mergeId],
    ["try", "named"],
  ];
  for (const c of cases) {
    const r = raw([...c, "--dry-run"]);
    if (r.status !== 0) throw new Error(`${c.join(" ")} --dry-run exited ${r.status}: ${r.text}`);
    const flagged = r.json.dryRun === true || r.json.merge?.dryRun === true;
    if (!flagged) throw new Error(`${c.join(" ")} --dry-run did not say dryRun: ${r.text}`);
    assertEqual(await probe(), before, `${c.join(" ")} --dry-run changed something`);
  }
  await settle();
  assertEqual(mainWatch.seen.length, seenBefore, "dry runs publish nothing to the main subscriber");

  // Idempotency keys: a retried keyed merge returns the same mergeId; another merge with that key is refused.
  const again = f(["--idem-key", "e2e-merge-key", "merge", "dry-src"]).merge;
  assertEqual(again.mergeId, keyed.mergeId, "a retried keyed merge returns the same mergeId");
  failure(raw(["--idem-key", "e2e-merge-key", "merge", "dry-b"]), "IDEM_KEY_REUSED", 3, "the key on another merge");
  void own;
}

/**
 * A real `project recover --create`: two servers on copies of one folder diverge (the copy has
 * no lease files, so B takes its own); B's tail is imported on A as a foreign group and
 * created as a `recovered-*` branch.
 */
export async function recover({ tmp, sd, server, servers, sockets, writeFixture }) {
  const sdB = join(tmp, "server-b");
  const a = join(tmp, "recover-a");
  const b = join(tmp, "recover-b");
  const fa = (args) => fin(sd, a, args);
  const fb = (args) => fin(sdB, b, args);
  step(17, "two servers diverge on copies of one folder");
  const created = fin(sd, tmp, ["project", "create", a, "--name", "Recover"]);
  const projectId = created.project.projectId;
  await writeFixture(join(a, "clip.mp4"));
  fa(["asset", "add", "clip.mp4"]);
  fa(["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s"]);
  await exported(sd, a);
  cpSync(a, b, { recursive: true });
  for (const name of readdirSync(join(b, ".finisher"))) if (name.startsWith("lease.")) rmSync(join(b, ".finisher", name));
  const serverB = await startServer(sdB);
  servers.push(serverB);
  fb(["project", "open", b]);
  await waitFor("server B to take its own lease", () => !fb(["project", "status"]).status.readOnly, 40_000);
  const bTail = [
    fb(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s"]),
    fb(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]),
  ];
  await exported(sdB, b);
  fa(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "3s"]);
  const moved = join(tmp, "moved-segments");
  cpSync(join(b, ".finisher", "segments"), moved, { recursive: true });

  step(18, "recover lists nothing, imports B's side, plans, then creates");
  const ws = await connectWs(server);
  sockets.push(ws);
  const snap = (branch) => ws.call("snapshot.get", { projectId, branch });
  const main0 = await snap("main");
  const mainWatch = watch(ws, projectId, "main", main0.rev, main0.generation);
  assertEqual(fa(["project", "recover"]).groups, [], "no groups before the import");
  assertEqual(fa(["project", "recover", "--import", moved, "--dry-run"]).imported, bTail.length, "a dry-run import reports what it would read");
  assertEqual(fa(["project", "recover"]).groups, [], "still no groups after the dry run");
  const imported = fa(["project", "recover", "--import", moved]);
  assertEqual(imported.imported, bTail.length, "the two tail records are imported");
  assertEqual(imported.groups.length, 1, "one group");
  const group = imported.groups[0];
  assertEqual([group.recoverable, group.records, group.origin, group.forkSeq], [true, 2, "foreign", 2], "group");
  const branchesBefore = fa(["branch", "ls", "--all"]).branches;
  const plan = fa(["project", "recover", "--create", "all", "--dry-run"]);
  assertEqual(plan.created.length, 1, "the dry run plans one branch");
  assertEqual(fa(["branch", "ls", "--all"]).branches, branchesBefore, "the dry run created nothing");

  const done = fa(["project", "recover", "--create", "all"]);
  assertEqual(done.created.length, 1, "one recovered branch");
  const rec = done.created[0];
  if (!rec.display.startsWith("recovered-")) throw new Error(`unexpected name ${rec.display}`);
  assertEqual(clipsOf(await snap(rec.id)).t1.map((c) => c.split("+")[1]), ["4", "2", "1"], "recovered branch holds B's tail");
  assertEqual(clipsOf(await snap("main")).t1.map((c) => c.split("+")[1]), ["4", "3"], "main keeps A's tail only");
  const recWatch = watch(ws, projectId, rec.id, 0, (await snap(rec.id)).generation);
  await recWatch.waitFor("the branch's records from rev 0", (s) => s.some((t) => t.tx.branch === rec.id));
  await settle();
  assertEqual(mainWatch.seen.every((t) => t.tx.branch === "main"), true, "records published to main stay on main");
  assertEqual(fa(["project", "recover"]).groups, [], "nothing left to recover");
  assertEqual(fa(["project", "verify"]).report.ok, true, "verify ok after recover");
  mainWatch.stop();
  recWatch.stop();
}

/** Waits until `fin project status` shows every record exported. */
async function exported(sd, cwd) {
  await waitFor(
    "the export to catch up",
    () => {
      const m = fin(sd, cwd, ["project", "status"]).status.branches.find((x) => x.branch === "main");
      return m.exportedThrough === m.head;
    },
    15_000,
  );
}
