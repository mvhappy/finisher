// Branch and history RPCs through the ProjectRegistry (plan 4B Task 18): hubs are keyed by
// branch id, every record of a result reaches its own branch's hub in rev order, dry runs
// publish nothing.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { BranchCreated, CommittedTx, HistoryResult, MergeReport, RecoverReport } from "@finisher/core-types";
import { Effect, Exit, Fiber, Scope, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { publishResult } from "../src/history.ts";
import { Hubs } from "../src/hubs.ts";
import { makeProjectRegistry, type ProjectRegistryShape } from "../src/projects.ts";
import { addAsset, insert, tx } from "./support/ndjson-client.ts";

let tmp: string;
let scope: Scope.Closeable | undefined;
let r: ProjectRegistryShape;
let projectId: string;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-history-"));
  scope = Effect.runSync(Scope.make());
  r = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(tmp, "sd"))));
});

afterEach(async () => {
  if (scope) await Effect.runPromise(Scope.close(scope, Exit.void));
  scope = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = (e: Effect.Effect<unknown, FinError>): Promise<FinError> => Effect.runPromise(Effect.flip(e));
const author = "ann";

/** A project with the asset (rev 1) and one clip `c1` (rev 2) on main. */
async function project(dir = join(tmp, "p")): Promise<string> {
  projectId = (await run(r.create(dir, "Demo", "30"))).projectId;
  await run(r.commit(projectId, tx("t-asset", 0, [addAsset()])));
  await run(r.commit(projectId, tx("t-c1", 1, [insert(0, 5)])));
  return projectId;
}

const branchTx = (id: string, branch: string, baseRev: number, ops: unknown[]) => ({ ...tx(id, baseRev, ops), branch });
const edit = (at: number) => ({ op: "clip.insert", id: "$c", track: "t1", at: String(at), asset: "a1", sourceIn: "0", duration: "5" });

const create = (txId: string, name: string, extra: object = {}) =>
  run(r.history(projectId, "branchCreate", { projectId, txId, author, name, from: "main", ...extra })) as Promise<BranchCreated>;

/** Collects the revs a subscription delivers. */
function watch(branch: string, fromRev: number) {
  const seen: CommittedTx[] = [];
  const fiber = Effect.runFork(Stream.runForEach(r.subscribe(projectId, branch, fromRev), (c) => Effect.sync(() => seen.push(c))));
  return {
    seen,
    revs: () => seen.map((c) => c.rev),
    stop: () => Effect.runPromise(Fiber.interrupt(fiber)),
    exit: () => Effect.runPromise(Fiber.await(fiber)),
  };
}
const settle = () => new Promise((res) => setTimeout(res, 150));
const until = async (f: () => boolean, what: string) => {
  const end = Date.now() + 5_000;
  while (!f()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((res) => setTimeout(res, 25));
  }
};

describe("branch and history RPCs", () => {
  test("branch.create then branch.list (all)", async () => {
    await project();
    const made = await create("b1", "feature");
    expect(made.branch).toMatchObject({ id: "br1", name: "feature", parent: "main", forkSeq: 2 });
    expect((await run(r.branches(projectId, false))).branches.map((b) => b.id)).toEqual(["main", "br1"]);
    await run(r.history(projectId, "branchSet", { projectId, txId: "s1", author, branch: "feature", archived: true }));
    expect((await run(r.branches(projectId, false))).branches.map((b) => b.id)).toEqual(["main"]);
    const all = (await run(r.branches(projectId, true))).branches;
    expect(all.map((b) => [b.id, b.state])).toEqual([
      ["main", "active"],
      ["br1", "archived"],
    ]);
  });

  test("commit on a branch publishes only to that branch", async () => {
    await project();
    await create("b1", "feature");
    const onMain = watch("main", 3);
    const onBranch = watch("br1", 2);
    await settle();
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [edit(10)])));
    await until(() => onBranch.revs().length === 1, "the branch's rev");
    await settle();
    expect(onBranch.revs()).toEqual([3]);
    expect(onBranch.seen[0]!.tx.branch).toBe("br1");
    expect(onMain.revs()).toEqual([]);
    await onMain.stop();
    await onBranch.stop();
  });

  test("merge publishes each record to the target hub in rev order", async () => {
    await project();
    await create("b1", "feature");
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [edit(10)])));
    await run(r.commit(projectId, branchTx("t-b2", "br1", 3, [edit(20)])));
    const onMain = watch("main", 3);
    const onBranch = watch("br1", 4);
    await settle();
    const report = (await run(
      r.history(projectId, "merge", { projectId, txId: "m1", author, from: "feature", into: "main" }),
    )) as MergeReport;
    expect(report.committed.map((c) => c.tx.txId)).toEqual(["m1-1", "m1-2"]);
    await until(() => onMain.revs().length === 2, "the merge records");
    expect(onMain.revs()).toEqual([4, 5]);
    expect(onMain.seen.map((c) => c.tx.txId)).toEqual(["m1-1", "m1-2"]);
    expect(onBranch.revs()).toEqual([]);
    await onMain.stop();
    await onBranch.stop();
  });

  test("undo conflict is CONFLICT with details.conflicts", async () => {
    await project();
    await create("b1", "feature");
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [{ op: "clip.delete", clip: "c1" }])));
    await run(r.commit(projectId, { ...tx("t-m", 3, [{ op: "clip.delete", clip: "c1" }]), author: "bob" }));
    const err = await failure(r.history(projectId, "merge", { projectId, txId: "m1", author, from: "br1", into: "main" }));
    expect(err.code).toBe("CONFLICT");
    expect(Array.isArray((err.details as { conflicts?: unknown }).conflicts)).toBe(true);
    // Undoing an insert whose clip another author removed meanwhile: refused, nothing published.
    const onMain = watch("main", 4);
    await settle();
    const undo = await failure(r.history(projectId, "undo", { projectId, txId: "u1", author: "test", branch: "main" }));
    expect(undo.code).toBe("CONFLICT");
    expect(Array.isArray((undo.details as { conflicts?: unknown }).conflicts)).toBe(true);
    await settle();
    expect(onMain.revs()).toEqual([]);
    await onMain.stop();
  });

  test("dryRun publishes nothing", async () => {
    await project();
    await create("b1", "feature");
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [edit(10)])));
    const onMain = watch("main", 3);
    await settle();
    const dry = (await run(
      r.history(projectId, "merge", { projectId, txId: "m1", author, from: "feature", into: "main", dryRun: true }),
    )) as MergeReport;
    expect(dry.dryRun).toBe(true);
    expect(dry.committed).toHaveLength(1);
    const cp = await run(r.history(projectId, "checkpoint", { projectId, txId: "c0", author, branch: "main", name: "v0", dryRun: true }));
    expect((cp as CommittedTx).tx.txId).toBe("c0");
    const branch = (await run(
      r.history(projectId, "branchCreate", { projectId, txId: "b2", author, name: "later", from: "main", dryRun: true }),
    )) as BranchCreated;
    expect(branch.dryRun).toBe(true);
    await settle();
    expect(onMain.revs()).toEqual([]);
    expect(await Effect.runPromise(Effect.map(r.snapshot(projectId, "main"), (s) => s.rev))).toBe(3);
    // The real one publishes.
    await run(r.history(projectId, "merge", { projectId, txId: "m1", author, from: "feature", into: "main" }));
    await until(() => onMain.revs().length === 1, "the merge");
    await onMain.stop();
  });

  test("ops.subscribe by branch name resolves to the id", async () => {
    await project();
    await create("b1", "feature");
    const byName = watch("feature", 2);
    const byId = watch("br1", 2);
    await settle();
    await run(r.commit(projectId, branchTx("t-b1", "feature", 2, [edit(10)])));
    await until(() => byName.revs().length === 1 && byId.revs().length === 1, "the rev on both");
    expect(byName.revs()).toEqual([3]);
    expect((await run(r.snapshot(projectId, "feature"))).rev).toBe(3);
    expect(r.hubCount().ops).toBe(1);
    const missing = await Effect.runPromise(Effect.flip(Stream.runCollect(r.subscribe(projectId, "nope", 0))));
    expect((missing as FinError).code).toBe("BRANCH_NOT_FOUND");
    await byName.stop();
    await byId.stop();
  });

  test("a cascaded branch's rewind ends its subscribers", async () => {
    await project();
    await create("b1", "feature");
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [edit(10)])));
    const sub = watch("feature", 0);
    const main = watch("main", 0);
    await until(() => sub.revs().length === 3 && main.revs().length === 3, "the backlogs");
    // The events an import's cascade emits name the branch by id.
    r.engineOf(projectId)!.emitRewoundForTest("br1", 3);
    const exit = await sub.exit();
    expect(Exit.isFailure(exit)).toBe(true);
    const error = Exit.isFailure(exit) ? (exit.cause.reasons.find((x) => x._tag === "Fail") as { error: FinError }).error : undefined;
    expect(error?.code).toBe("HISTORY_REWOUND");
    expect(error?.details).toMatchObject({ branch: "br1", rev: 3 });
    expect(Exit.isFailure(await Promise.race([main.exit(), settle().then(() => Exit.void)]))).toBe(false);
    await main.stop();
  });

  test("undo, redo and revert publish their records", async () => {
    await project();
    await create("b1", "feature");
    await run(r.commit(projectId, branchTx("t-b1", "br1", 2, [edit(10)])));
    await run(r.history(projectId, "merge", { projectId, txId: "m1", author, from: "feature", into: "main" }));
    const onMain = watch("main", 3);
    await until(() => onMain.revs().length === 1, "the merge in the backlog");
    const undo = (await run(r.history(projectId, "undo", { projectId, txId: "u1", author, branch: "main" }))) as HistoryResult;
    const redo = (await run(r.history(projectId, "redo", { projectId, txId: "r1", author, branch: "main" }))) as HistoryResult;
    const revert = (await run(r.history(projectId, "revert", { projectId, txId: "v1", author, mergeId: "m1", branch: "main" }))) as HistoryResult;
    expect([undo.committed.length, redo.committed.length, revert.committed.length]).toEqual([1, 1, 1]);
    await until(() => onMain.revs().length === 4, "undo, redo, revert");
    expect(onMain.seen.map((c) => c.tx.txId)).toEqual(["m1-1", "u1", "r1", "v1"]);
    await onMain.stop();
  });

  test("project.recover lists, then a create publishes every committed record to its own branch, in rev order", async () => {
    await project();
    const listed = (await run(r.history(projectId, "recover", { projectId }))) as RecoverReport;
    expect(listed).toMatchObject({ groups: [], created: [], committed: [], imported: 0, dryRun: false });
    expect((await failure(r.history(projectId, "recover", { projectId, txId: "rec", author, create: "nope" }))).code).toBe("NOT_FOUND");

    // What a recovery of two groups writes: `branch` records on their parents and the copies on
    // the new branches, in write order. Each goes to its branch's hub, lowest rev first.
    const rec = (branch: string, rev: number): CommittedTx =>
      ({ rev, tx: { txId: `rec-${branch}-${rev}`, branch }, bindings: {}, diff: {} }) as unknown as CommittedTx;
    const report = { groups: [], created: [], imported: 0, manual: null, dryRun: false, committed: [rec("main", 9), rec("br7", 3), rec("br7", 4), rec("main", 10), rec("br8", 5)] };
    const hubs = new Hubs(16);
    const published: string[] = [];
    hubs.publishTx = (_projectId, c) => void published.push(`${c.tx.branch}:${c.rev}`);
    publishResult(hubs, "p", report, false);
    expect(published).toEqual(["br7:3", "br7:4", "br8:5", "main:9", "main:10"]);
    // A dry run wrote nothing, whatever it lists; a bare record follows the request's flag.
    publishResult(hubs, "p", { ...report, dryRun: true }, false);
    publishResult(hubs, "p", rec("main", 11), true);
    expect(published).toHaveLength(5);
    publishResult(hubs, "p", rec("main", 11), false);
    expect(published.at(-1)).toBe("main:11");
  });
});
