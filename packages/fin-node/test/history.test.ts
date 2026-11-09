// Branch and history bindings (plan 4B Task 17): branches, undo/redo, checkpoint, merge,
// cherry-pick, revert and recover through the camelCase request JSON. Followers poll a local
// folder every 2 s, so the follower case waits for the leader's records to arrive.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError } from "../index.js";

let tmp: string;
let projectDir: string;
let engines: Engine[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-history-"));
  projectDir = join(tmp, "My Project");
});

afterEach(async () => {
  for (const e of engines) await e.close().catch(() => undefined);
  engines = [];
  rmSync(tmp, { recursive: true, force: true });
});

function keep(e: Engine): Engine {
  engines.push(e);
  return e;
}

function tx(txId: string, branch: string, baseRev: number, ops: unknown[]): string {
  return JSON.stringify({ txId, author: "test", branch, baseRev, schemaVersion: 2, ops });
}

const addAsset = {
  op: "asset.add",
  id: "$a",
  path: "/media/a.mov",
  duration: "600",
  video: { width: 1920, height: 1080, rate: "30" },
  audio: null,
  fingerprint: "fp",
};

function insert(at: number): object {
  return { op: "clip.insert", id: "$c", track: "t1", at: String(at), asset: "a1", sourceIn: "0", duration: "5" };
}

const json = async <T = any>(p: Promise<string>): Promise<T> => JSON.parse(await p) as T; // eslint-disable-line @typescript-eslint/no-explicit-any

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

const req = (o: object) => JSON.stringify({ author: "test", ...o });

/** A project with the asset (rev 1) and one clip (rev 2) on main. */
async function project(serverData = join(tmp, "server-data")): Promise<Engine> {
  const e = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
  await e.commit(tx("t-asset", "main", 0, [addAsset]));
  await e.commit(tx("t-c1", "main", 1, [insert(0)]));
  return e;
}

const clips = async (e: Engine, branch = "main"): Promise<number> => {
  const snap = await json(e.snapshot(branch));
  return snap.project.timelines.flatMap((tl: { tracks: { items: unknown[] }[] }) => tl.tracks.flatMap((t) => t.items)).length;
};

describe("branches", () => {
  test("create, list, resolve, commit on a branch and set", async () => {
    const e = await project();
    const made = await json(e.branchCreate(req({ txId: "b1", name: "feature", from: "main" })));
    expect(made.branch).toMatchObject({ id: "br1", name: "feature", parent: "main", forkSeq: 2, head: 2 });
    expect(made.record.tx.branch).toBe("main");
    expect(made.record.rev).toBe(3);
    expect(made.dryRun).toBe(false);

    expect((await json<{ id: string }[]>(e.branches(false))).map((b) => b.id)).toEqual(["main", "br1"]);
    expect(await json(e.resolveBranch("feature"))).toBe("br1");
    expect((await rejection(e.resolveBranch("nope"))).code).toBe("BRANCH_NOT_FOUND");

    await e.commit(tx("t-b1", "br1", 2, [insert(10)]));
    expect(await e.head("br1")).toBe(3);
    expect(await e.head("main")).toBe(3);
    expect(await clips(e, "br1")).toBe(2);
    expect(await clips(e)).toBe(1);

    const renamed = await json(e.branchSet(req({ txId: "b2", branch: "feature", name: "polish" })));
    expect(renamed.tx.txId).toBe("b2");
    await e.branchSet(req({ txId: "b3", branch: "br1", archived: true }));
    expect((await json<unknown[]>(e.branches(false))).length).toBe(1);
    const all = await json<{ id: string; state: string; display: string }[]>(e.branches(true));
    expect(all.map((b) => [b.id, b.state])).toEqual([
      ["main", "active"],
      ["br1", "archived"],
    ]);
  });

  test("argsHash: a retry with the same hash answers, another is IDEM_KEY_REUSED", async () => {
    const e = await project();
    const h1 = `blake3:${"a".repeat(64)}`;
    const h2 = `blake3:${"b".repeat(64)}`;
    const first = await json(e.branchCreate(req({ txId: "b1", name: "x", from: "main", argsHash: h1 })));
    const again = await json(e.branchCreate(req({ txId: "b1", name: "x", from: "main", argsHash: h1 })));
    expect(again.branch.id).toBe(first.branch.id);
    expect((await rejection(e.branchCreate(req({ txId: "b1", name: "x", from: "main", argsHash: h2 })))).code).toBe(
      "IDEM_KEY_REUSED",
    );
    await e.branchSet(req({ txId: "s1", branch: "x", name: "y", argsHash: h1 }));
    expect((await rejection(e.branchSet(req({ txId: "s1", branch: "y", name: "y", argsHash: h2 })))).code).toBe(
      "IDEM_KEY_REUSED",
    );
    expect((await rejection(e.branchCreate(req({ txId: "b9", from: "main", argsHash: "nope" })))).code).toBe(
      "INVALID_OP",
    );
  });

  test("bad requests are INVALID_OP naming the problem", async () => {
    const e = await project();
    const err = await rejection(e.undo("{not json"));
    expect(err.code).toBe("INVALID_OP");
    expect(err.finNode).toBe(true);
    const unknown = await rejection(e.branchCreate(req({ txId: "b1", from: "main", bogus: 1 })));
    expect(unknown.code).toBe("INVALID_OP");
    expect(unknown.message).toContain("bogus");
  });

  test("at: a seq or a checkpoint", async () => {
    const e = await project();
    await e.checkpoint(req({ txId: "c1", branch: "main", name: "v1" }));
    await e.commit(tx("t-3", "main", 3, [insert(10)]));
    const bySeq = await json(e.branchCreate(req({ txId: "b1", name: "old", from: "main", at: { seq: 1 } })));
    expect(bySeq.branch.forkSeq).toBe(1);
    expect(await clips(e, "br1")).toBe(0);
    const byCp = await json(e.branchCreate(req({ txId: "b2", name: "cp", from: "main", at: { checkpoint: "v1" } })));
    expect(byCp.branch.forkSeq).toBe(3);
  });
});

describe("history", () => {
  test("history calls are named, non-enumerable methods", () => {
    for (const name of ["drop", "merge", "undo", "branchCreate"] as const) {
      const d = Object.getOwnPropertyDescriptor(Engine.prototype, name);
      expect(d?.enumerable, name).toBe(false);
      expect((d?.value as { name: string }).name).toBe(name);
    }
  });

  test("drop and undrop (plan 5E Task 2)", async () => {
    const e = await project();
    await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await e.commit(tx("t-b1", "br1", 2, [insert(10)]));
    await e.commit(tx("t-b2", "br1", 3, [insert(20)]));

    const planned = await json(e.drop(req({ txId: "d1", into: "main", from: "feature", drops: ["t-b2"], dryRun: true })));
    expect(planned.link).toEqual({ kind: "drop", from: "br1", drops: ["t-b2"] });
    const dropped = await json(e.drop(req({ txId: "d1", into: "main", from: "feature", drops: ["t-b2"] })));
    expect(dropped.tx.branch).toBe("main");
    expect(dropped.link).toEqual({ kind: "drop", from: "br1", drops: ["t-b2"] });
    expect(await json(e.drop(req({ txId: "d1", into: "main", from: "feature", drops: ["t-b2"] })))).toEqual(dropped);

    const merged = await json(e.merge(req({ txId: "m1", from: "feature", into: "main" })));
    expect(merged.merged).toEqual(["t-b1"]);
    expect(merged.excluded.map((c: { txId: string; reason: string }) => [c.txId, c.reason])).toEqual([["t-b2", "DROPPED"]]);

    const bad = await rejection(e.drop(req({ txId: "d2", into: "main", from: "feature", drops: ["t-c1"] })));
    expect(bad.code).toBe("INVALID_OP");
    expect(bad.details).toMatchObject({ reason: "notOnSource", txId: "t-c1" });
    const unknown = await rejection(e.drop(req({ txId: "d2", into: "main", from: "feature", drops: ["t-b1"], extra: 1 })));
    expect(unknown.code).toBe("INVALID_OP");

    await e.drop(req({ txId: "d3", into: "main", from: "feature", undrops: ["t-b2"] }));
    const again = await json(e.merge(req({ txId: "m2", from: "feature", into: "main" })));
    expect(again.merged).toEqual(["t-b2"]);
  });

  test("a merge limited by include and sourceUpTo; the review plan (plan 5E Task 3)", async () => {
    const e = await project();
    await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await e.commit(tx("t-b1", "br1", 2, [insert(10)]));
    await e.commit(tx("t-b2", "br1", 3, [insert(20)]));
    await e.commit(tx("t-b3", "br1", 4, [insert(30)]));

    const plan = await json(e.reviewPlan(JSON.stringify({ from: "feature", into: "main" })));
    expect(plan.source).toMatchObject({ branch: "br1", seq: 5 });
    expect(typeof plan.source.hash).toBe("string");
    expect(plan.target.branch).toBe("main");
    expect(plan.entries.map((x: { txId: string; status: string }) => [x.txId, x.status])).toEqual([
      ["t-b1", "willMerge"],
      ["t-b2", "willMerge"],
      ["t-b3", "willMerge"],
    ]);
    expect(plan.entries[0]).toMatchObject({ origin: "t-b1", branch: "br1", kind: "edit", opsCount: 1 });
    expect(plan).toMatchObject({ conflicts: [], truncated: false });

    const merged = await json(e.merge(req({ txId: "m1", from: "feature", into: "main", include: ["t-b1", "t-b3"], sourceUpTo: 4 })));
    expect(merged.merged).toEqual(["t-b1"]);
    expect(merged.notIncluded).toEqual(["t-b2", "t-b3"]);
    const after = await json(e.reviewPlan(JSON.stringify({ from: "feature", into: "main" })));
    expect(after.entries.map((x: { status: string }) => x.status)).toEqual(["alreadyPresent", "willMerge", "willMerge"]);

    const bad = await rejection(e.reviewPlan(JSON.stringify({ from: "feature" })));
    expect(bad.code).toBe("INVALID_OP");
    expect((await rejection(e.reviewPlan(JSON.stringify({ from: "nope", into: "main" })))).code).toBe("BRANCH_NOT_FOUND");
  });

  test("merge, undo, redo, revert", async () => {
    const e = await project();
    await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await e.commit(tx("t-b1", "br1", 2, [insert(10)]));

    const merged = await json(e.merge(req({ txId: "m1", from: "feature", into: "main" })));
    expect(merged.committed.map((c: { tx: { txId: string } }) => c.tx.txId)).toEqual(["m1-1"]);
    expect(merged.skipped).toEqual([]);
    expect(await clips(e)).toBe(2);

    const undone = await json(e.undo(req({ txId: "u1", branch: "main" })));
    expect(undone.committed).toHaveLength(1);
    expect(undone.undone).toEqual(["m1"]);
    expect(undone.dryRun).toBe(false);
    expect(await clips(e)).toBe(1);

    const redone = await json(e.redo(req({ txId: "r1", branch: "main" })));
    expect(redone.committed).toHaveLength(1);
    expect(await clips(e)).toBe(2);

    const reverted = await json(e.revert(req({ txId: "v1", mergeId: "m1", branch: "main" })));
    expect(reverted.undone).toEqual(["m1"]);
    expect(await clips(e)).toBe(1);
    const twice = await json(e.revert(req({ txId: "v2", mergeId: "m1", branch: "main" })));
    expect(twice.committed).toEqual([]);
    expect(twice.noOp).toBe("ALREADY_REVERTED");

    const nothing = await rejection(e.undo(req({ txId: "u9", branch: "main", author: "someone-else" })));
    expect(nothing.code).toBe("NOTHING_TO_UNDO");
  });

  test("checkpoint and undo --to", async () => {
    const e = await project();
    const cp = await json(e.checkpoint(req({ txId: "c1", branch: "main", name: "v1" })));
    expect(cp.tx.ops).toEqual([]);
    expect((await rejection(e.checkpoint(req({ txId: "c2", branch: "main", name: "V1" })))).code).toBe("NAME_TAKEN");
    await e.commit(tx("t-3", "main", 3, [insert(10)]));
    await e.commit(tx("t-4", "main", 4, [insert(20)]));
    expect(await clips(e)).toBe(3);
    const back = await json(e.undo(req({ txId: "u1", branch: "main", to: "v1" })));
    expect(back.committed).toHaveLength(1);
    expect(back.undone).toHaveLength(2);
    expect(await clips(e)).toBe(1);
    expect((await rejection(e.undo(req({ txId: "u2", branch: "main", to: "nope" })))).code).toBe("NOT_FOUND");
    expect((await rejection(e.redo(req({ txId: "r0", branch: "main", to: "v1" })))).code).toBe("INVALID_OP");
  });

  test("cherry-pick copies a record and repeats as a no-op", async () => {
    const e = await project();
    await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await e.commit(tx("t-b1", "br1", 2, [insert(10)]));
    const pick = await json(e.cherryPick(req({ txId: "p1", origin: "t-b1", into: "main" })));
    expect(pick.committed).toHaveLength(1);
    expect(pick.committed[0].link.origin).toBe("t-b1");
    expect(await clips(e)).toBe(2);
    const again = await json(e.cherryPick(req({ txId: "p2", origin: "t-b1", into: "main" })));
    expect(again.committed).toEqual([]);
    expect(again.noOp).toBe("ALREADY_PRESENT");
  });

  test("a merge conflict is CONFLICT with details.conflicts; skipConflicting leaves it", async () => {
    const e = await project();
    await e.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await e.commit(tx("t-b1", "br1", 2, [{ op: "clip.delete", clip: "c1" }]));
    await e.commit(tx("t-m", "main", 3, [{ op: "clip.delete", clip: "c1" }]));
    const err = await rejection(e.merge(req({ txId: "m1", from: "br1", into: "main" })));
    expect(err.code).toBe("CONFLICT");
    expect(Array.isArray(err.details.conflicts)).toBe(true);
    const dry = await rejection(e.merge(req({ txId: "m1", from: "br1", into: "main", dryRun: true })));
    expect(dry.code).toBe("CONFLICT");
    const skipped = await json(e.merge(req({ txId: "m1", from: "br1", into: "main", skipConflicting: true })));
    expect(skipped.committed).toEqual([]);
    expect(skipped.skipped.length).toBeGreaterThan(0);
  });

  test("recover lists groups (none yet) and refuses an unknown group", async () => {
    const e = await project();
    const listed = await json(e.recover(req({})));
    expect(listed).toMatchObject({ groups: [], created: [], committed: [], imported: 0, dryRun: false });
    const err = await rejection(e.recover(req({ txId: "r1", create: "nope" })));
    expect(err.code).toBe("NOT_FOUND");
    const all = await json(e.recover(req({ txId: "r2", create: "all", dryRun: true })));
    expect(all.created).toEqual([]);
  });

  test("a dry run on a follower plans and keeps nothing; a real write is refused", async () => {
    const leader = await project();
    await leader.branchCreate(req({ txId: "b1", name: "feature", from: "main" }));
    await leader.commit(tx("t-b1", "br1", 2, [insert(10)]));
    await leader.merge(req({ txId: "m1", from: "feature", into: "main" }));
    const planned = await json(leader.undo(req({ txId: "u-plan", branch: "main", dryRun: true })));
    expect(planned.dryRun).toBe(true);
    expect(await leader.head("main")).toBe(4);

    const follower = keep(await Engine.open(join(tmp, "server-data-2"), projectDir));
    const status = await json(follower.status());
    expect(status.readOnly.reason).toBe("LEASE_HELD");
    const deadline = Date.now() + 30_000;
    while ((await follower.head("main")) < 4 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 250));
    expect(await follower.head("main")).toBe(4);

    const dry = await json(follower.undo(req({ txId: "u-plan", branch: "main", dryRun: true })));
    expect(dry.dryRun).toBe(true);
    expect(dry.undone).toEqual(["m1"]);
    expect(dry.committed).toHaveLength(1);
    expect(await follower.head("main")).toBe(4);
    const dryBranch = await json(follower.branchCreate(req({ txId: "b-dry", name: "later", from: "main", dryRun: true })));
    expect(dryBranch.dryRun).toBe(true);
    expect((await json<unknown[]>(follower.branches(true))).length).toBe(2);

    const real = await rejection(follower.undo(req({ txId: "u-real", branch: "main" })));
    expect(real.code).toBe("PROJECT_READ_ONLY");
  }, 60_000);
});
