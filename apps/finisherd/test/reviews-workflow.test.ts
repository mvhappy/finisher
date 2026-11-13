// The branch review workflow through a real server (5E plan Task 7; Decisions 9, 12–15): the
// snapshot and its change list, decisions per origin transaction, rejections as `drop` records,
// merges limited to what was reviewed, closing, changes requested, the watch.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CredentialFile, ReviewItem, ReviewView, Snapshot } from "@finisher/contracts";
import type { BranchView, CommittedTx, MergeReport, ReviewPlan } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, onTestFinished, test } from "vitest";
import { b64u, canonicalEndpoint, proveMac } from "../src/auth/crypto.ts";
import { openControlStore } from "../src/control/db.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { LOCAL } from "../src/principal.ts";
import { mergeReview } from "../src/reviews/merge.ts";
import { Planner, type ReviewEngine } from "../src/reviews/plan.ts";
import { ReviewStore } from "../src/reviews/store.ts";
import { callerOf, ReviewWorkflow } from "../src/reviews/workflow.ts";
import { addAsset, commit, confirmed, connectProved, type Exit, hello, NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];
let local: NdjsonClient;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-reviews-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  running = { fiber, info: await info };
  local = await connect("local");
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  if (running !== undefined) await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function connect(name: "local" | "console"): Promise<NdjsonClient> {
  const c = await connectProved({ info: running!.info, serverData }, name);
  clients.push(c);
  return c;
}

const code = (e: Exit): string | undefined => (e.ok ? undefined : e.error.code);
const call = async <A>(tag: string, payload: object, c: NdjsonClient = local): Promise<A> => ok<A>(await c.call(tag, payload));

/** An edit that depends on nothing: a new asset. */
const edit = (txId: string) => ({ ...addAsset(`$${txId}`), path: `/media/${txId}.mov`, fingerprint: txId });

/** A project whose `main` holds asset `a1` and clip `c1`. */
async function project(): Promise<string> {
  const { projectId } = await call<{ projectId: string }>("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" });
  ok(await commit(local, projectId, tx("m-asset", 0, [addAsset()])));
  ok(await commit(local, projectId, tx("m-clip", 1, [{ op: "clip.insert", id: "$c", track: "t1", at: "0", asset: "a1", sourceIn: "0", duration: "5" }])));
  return projectId;
}

async function branches(projectId: string): Promise<ReadonlyArray<BranchView>> {
  return (await call<{ branches: BranchView[] }>("branch.list", { projectId, all: true })).branches;
}
const head = async (projectId: string, id: string) => (await branches(projectId)).find((b) => b.id === id)!.head;

/** A branch of `main` made by `author`, with the commits `txIds` (independent edits). */
async function branch(projectId: string, name: string, author: string, txIds: ReadonlyArray<string>): Promise<string> {
  const made = await call<{ branch: BranchView }>("branch.create", { projectId, txId: `b-${name}`, author, name, from: "main" });
  for (const t of txIds) await commitOn(projectId, made.branch.id, t, [edit(t)]);
  return made.branch.id;
}

async function commitOn(projectId: string, branchId: string, txId: string, ops: unknown[], author = "test"): Promise<CommittedTx> {
  return ok<CommittedTx>(await commit(local, projectId, { ...tx(txId, await head(projectId, branchId), ops), branch: branchId, author }));
}

async function request(projectId: string, source: string, title = "Please look"): Promise<ReviewItem> {
  return (await call<{ item: ReviewItem; existing: boolean }>("review.request", { projectId, source, title })).item;
}
const get = (itemId: string) => call<ReviewView & { plan: ReviewPlan }>("review.get", { itemId });
const decide = (item: ReviewItem, decisions: ReadonlyArray<[string, "accept" | "reject" | "pending"]>, extra: object = {}, expect: Snapshot = item.snapshot!) =>
  local.call("review.decide", { itemId: item.id, expect, decisions: decisions.map(([txId, decision]) => ({ txId, decision })), ...extra });
const merge = (item: ReviewItem, idemKey: string, extra: object = {}) => local.call("review.merge", { itemId: item.id, expect: item.snapshot!, idemKey, ...extra });

/** What a plain merge of `from` into main would do now: the origins it copies and those it drops. */
async function plainMerge(projectId: string, from: string): Promise<{ merged: string[]; dropped: string[] }> {
  const r = await call<MergeReport>("history.merge", { projectId, txId: `plain-${Math.random()}`, author: "ann", from, into: "main", dryRun: true, skipConflicting: true });
  return { merged: r.merged, dropped: r.excluded.filter((e) => e.reason === "DROPPED").map((e) => e.txId) };
}

describe("request, get, refresh", () => {
  test("request_plans_snapshot", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    expect(item).toMatchObject({ kind: "branchReview", state: "open", source: feature, target: "main", rederived: false });
    expect(item.snapshot).toMatchObject({ sourceSeq: await head(projectId, feature), targetSeq: await head(projectId, "main") });
    expect(item.snapshot?.sourceHash).toMatch(/.+/);
    const view = await get(item.id);
    expect(view.stale).toBe(false);
    expect(view.plan.entries.filter((e) => e.branch === feature).map((e) => [e.origin, e.status])).toEqual([
      ["f1", "willMerge"],
      ["f2", "willMerge"],
    ]);
  });

  test("request_again_returns_existing", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const first = await request(projectId, "feature", "First");
    const again = await call<{ item: ReviewItem; existing: boolean }>("review.request", { projectId, source: "feature", into: "main", title: "Second", summary: "x" });
    expect(again.existing).toBe(true);
    expect(again.item).toMatchObject({ id: first.id, title: "First" });
    expect(again.item.summary).toBeUndefined();
    // The same branch, and an archived source, are refused.
    expect(code(await local.call("review.request", { projectId, source: "main", title: "t" }))).toBe("INVALID_REQUEST");
    const other = await branch(projectId, "other", "ann", ["o1"]);
    ok(await local.call("branch.set", { projectId, txId: "arch", author: "ann", branch: other, archived: true }));
    const archived = await local.call("review.request", { projectId, source: other, title: "t" });
    expect(code(archived)).toBe("INVALID_REQUEST");
    expect(archived.ok ? undefined : archived.error.details).toMatchObject({ reason: "archived" });
  });

  test("stale_expect_is_review_stale_with_fresh_list", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    await commitOn(projectId, feature, "f2", [edit("f2")]);
    const refreshed = await call<ReviewView>("review.refresh", { itemId: item.id });
    expect(refreshed.item.snapshot?.sourceSeq).toBe(item.snapshot!.sourceSeq + 1);
    for (const refused of [await decide(item, [["f1", "accept"]]), await merge(item, "m1")]) {
      expect(code(refused)).toBe("REVIEW_STALE");
      const details = (refused.ok ? {} : refused.error.details) as { snapshot: Snapshot; plan: ReviewPlan };
      expect(details.snapshot).toEqual(refreshed.item.snapshot);
      expect(details.plan.entries.map((e) => e.origin)).toContain("f2");
    }
    // Nothing was decided.
    expect((await get(item.id)).decisions).toEqual([]);
  });

  test("get_reports_stale_on_head_or_hash_move", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    expect((await get(item.id)).stale).toBe(false);
    await commitOn(projectId, "main", "m2", [edit("m2")]);
    const onTarget = await get(item.id);
    expect(onTarget.stale).toBe(true);
    // The change list is still the snapshot's.
    expect(onTarget.plan.target.seq).toBe(item.snapshot!.targetSeq);
    const fresh = await call<ReviewView>("review.refresh", { itemId: item.id });
    expect(fresh.stale).toBe(false);
    await commitOn(projectId, feature, "f2", [edit("f2")]);
    expect((await get(item.id)).stale).toBe(true);
  });

  test("get_reports_stale_on_a_hash_move_at_the_same_seq (a rewind), from a fresh plan", async () => {
    const plan = (hash: string): ReviewPlan => ({ source: { branch: "s", seq: 4, hash }, target: { branch: "main", seq: 2, hash: "t" }, entries: [], conflicts: [], truncated: false });
    let hash = "h1";
    const engine: ReviewEngine = {
      resolveBranch: (_, r) => Effect.succeed(r),
      branches: () => Effect.succeed([]),
      plan: () => Effect.succeed(plan(hash)),
      records: () => Effect.succeed([]),
      write: () => Effect.die("no writes"),
    };
    const item = {
      id: "i1", projectId: "p", kind: "branchReview", state: "open", source: "s", target: "main", title: "t", requestedBy: { principal: "local:local" },
      createdAt: 0, updatedAt: 0, rederived: false, snapshot: { sourceSeq: 4, sourceHash: "h1", targetSeq: 2, targetHash: "t", plannedAt: 0 },
    } as const satisfies ReviewItem;
    expect((await Effect.runPromise(new Planner(engine).atSnapshot(item))).stale).toBe(false);
    hash = "h2";
    expect((await Effect.runPromise(new Planner(engine).atSnapshot(item))).stale).toBe(true);
  });
});

describe("decide", () => {
  test("reject_writes_drop_at_once", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const before = await head(projectId, "main");
    const view = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    // The drop record is on main, and a plain merge leaves the origin out.
    expect(await head(projectId, "main")).toBe(before + 1);
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f2"], dropped: ["f1"] });
    expect(view.decisions.map((d) => [d.txId, d.decision])).toEqual([["f1", "reject"]]);
    // The review's own drop does not make it stale: the snapshot follows it.
    expect(view.stale).toBe(false);
    expect(view.item.snapshot?.targetSeq).toBe(before + 1);
    // A decision on a transaction the review does not list is refused, and writes nothing.
    const refused = await decide(view.item, [["nope", "reject"]]);
    expect(code(refused)).toBe("INVALID_REQUEST");
    expect(await head(projectId, "main")).toBe(before + 1);
  });

  test("unreject_writes_undrop", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    expect((await plainMerge(projectId, "feature")).dropped).toEqual(["f1"]);
    const accepted = ok<ReviewView>(await decide(rejected.item, [["f1", "accept"]]));
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f1"], dropped: [] });
    expect(accepted.decisions.map((d) => d.decision)).toEqual(["accept"]);
    // Rejected, then pending: undropped and unmarked.
    const again = ok<ReviewView>(await decide(accepted.item, [["f1", "reject"]]));
    const pending = ok<ReviewView>(await decide(again.item, [["f1", "pending"]]));
    expect(pending.decisions).toEqual([]);
    expect((await plainMerge(projectId, "feature")).dropped).toEqual([]);
  });

  test("decisions_survive_refresh", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "reject"]]));
    await commitOn(projectId, feature, "f3", [edit("f3")]);
    const refreshed = await call<ReviewView & { plan: ReviewPlan }>("review.refresh", { itemId: item.id });
    expect(refreshed.decisions.map((d) => [d.txId, d.decision])).toEqual([
      ["f1", "accept"],
      ["f2", "reject"],
    ]);
    expect(refreshed.item.snapshot?.sourceSeq).toBe(decided.item.snapshot!.sourceSeq + 1);
    expect(refreshed.plan.entries.filter((e) => e.branch === feature).map((e) => [e.origin, e.status])).toEqual([
      ["f1", "willMerge"],
      ["f2", "dropped"],
      ["f3", "willMerge"],
    ]);
  });
});

describe("merge", () => {
  test("pending_never_accept_outside_snapshot", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    // f3 lands on the source after the snapshot: an accept-pending merge does not take it.
    await commitOn(projectId, feature, "f3", [edit("f3")]);
    const merged = ok<{ item: ReviewItem; merge: MergeReport; targetMoved: boolean }>(await merge(decided.item, "m1", { acceptPending: true }));
    expect([...merged.merge.merged].sort()).toEqual(["f1", "f2"]);
    expect(merged.merge.notIncluded).toEqual(["f3"]);
    expect(merged.item).toMatchObject({ state: "merged", mergeId: "m1" });
    expect((await plainMerge(projectId, "feature")).merged).toEqual(["f3"]);
  });

  test("without acceptPending only the accepted transactions are merged", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2", "f3"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "reject"]]));
    const merged = ok<{ merge: MergeReport }>(await merge(decided.item, "m1"));
    expect(merged.merge.merged).toEqual(["f1"]);
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f3"], dropped: ["f2"] });
  });

  test("require_review_refuses_pending", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    ok(await confirmed(await connect("console"), "review.policy", { projectId, requireReview: true }));
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    const before = await head(projectId, "main");
    for (const acceptPending of [false, true]) {
      const refused = await merge(decided.item, "m1", { acceptPending });
      expect(code(refused)).toBe("REVIEW_UNDECIDED");
      expect(refused.ok ? undefined : refused.error.details).toMatchObject({ pending: 1 });
    }
    expect(await head(projectId, "main")).toBe(before);
    // Every entry decided: it merges.
    const all = ok<ReviewView>(await decide(decided.item, [["f2", "reject"]]));
    expect(ok<{ merge: MergeReport }>(await merge(all.item, "m1")).merge.merged).toEqual(["f1"]);
  });

  test("conflicts_refuse_merge_and_write_nothing", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", []);
    await commitOn(projectId, feature, "f1", [{ op: "clip.delete", clip: "c1" }]);
    await commitOn(projectId, "main", "m2", [{ op: "clip.delete", clip: "c1" }]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    const before = await head(projectId, "main");
    const refused = await merge(decided.item, "m1");
    expect(code(refused)).toBe("REVIEW_HAS_CONFLICTS");
    const conflicts = (refused.ok ? {} : refused.error.details) as { conflicts: Array<{ txId: string }> };
    expect(conflicts.conflicts.map((c) => c.txId)).toContain("f1");
    expect(await head(projectId, "main")).toBe(before);
    expect((await get(item.id)).item.state).toBe("open");
  });

  test("an accepted transaction depending on a pending one is a conflict (DEPENDS_ON_SKIPPED)", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", []);
    const made = await commitOn(projectId, feature, "f1", [edit("f1")]);
    const asset = Object.values(made.bindings)[0]!;
    await commitOn(projectId, feature, "f2", [{ op: "clip.insert", id: "$c", track: "t1", at: "10", asset, sourceIn: "0", duration: "5" }]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f2", "accept"]]));
    const before = await head(projectId, "main");
    const refused = await merge(decided.item, "m1");
    expect(code(refused)).toBe("REVIEW_HAS_CONFLICTS");
    expect(JSON.stringify(refused.ok ? {} : refused.error.details)).toContain("DEPENDS_ON_SKIPPED");
    expect(await head(projectId, "main")).toBe(before);
  });

  test("merge_archives_finished_agent_branch", async () => {
    const projectId = await project();
    const agentBranch = await branch(projectId, "work", "agent:01jc0000000000000000000agt", ["f1", "f2", "f3"]);
    const item = await request(projectId, agentBranch);
    // Accepted and rejected: nothing is left to merge once it is merged.
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "accept"], ["f3", "reject"]]));
    const merged = ok<{ merge: MergeReport; targetMoved: boolean; archived: boolean }>(await merge(decided.item, "m1"));
    expect(merged.targetMoved).toBe(false);
    expect(merged.archived).toBe(true);
    expect((await branches(projectId)).find((b) => b.id === agentBranch)?.state).toBe("archived");
  });

  test("merge_keeps_unfinished_or_human_branch", async () => {
    const projectId = await project();
    const agentBranch = await branch(projectId, "work", "mcp:01jc0000000000000000000mcp", ["f1", "f2"]);
    const item = await request(projectId, agentBranch);
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    expect(ok<{ archived: boolean }>(await merge(decided.item, "m1")).archived).toBe(false);
    expect((await branches(projectId)).find((b) => b.id === agentBranch)?.state).not.toBe("archived");
    const human = await branch(projectId, "mine", "ann", ["h1"]);
    const second = await request(projectId, human);
    const all = ok<ReviewView>(await decide(second, [["h1", "accept"]]));
    ok(await merge(all.item, "m2"));
    expect((await branches(projectId)).find((b) => b.id === human)?.state).not.toBe("archived");
  });

  test("target_moved_merges_with_flag", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "accept"]]));
    await commitOn(projectId, "main", "m2", [edit("m2")]);
    expect((await get(item.id)).stale).toBe(true);
    const merged = ok<{ merge: MergeReport; targetMoved: boolean }>(await merge(decided.item, "m1"));
    expect(merged.targetMoved).toBe(true);
    expect([...merged.merge.merged].sort()).toEqual(["f1", "f2"]);
  });

  test("merge_is_idempotent_on_idem_key", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    const first = ok<{ item: ReviewItem; merge: MergeReport; targetMoved: boolean }>(await merge(decided.item, "m1"));
    const after = await head(projectId, "main");
    const console = await connect("console");
    const audited = async () => (await call<{ entries: unknown[] }>("auth.audit.list", { kind: "review.merge" }, console)).entries.length;
    expect(await audited()).toBe(1);
    const again = ok<{ item: ReviewItem; merge: MergeReport; targetMoved: boolean }>(await merge(decided.item, "m1"));
    expect(again.merge.merged).toEqual(first.merge.merged);
    expect(again.merge.committed.map((c) => c.rev)).toEqual(first.merge.committed.map((c) => c.rev));
    expect(again.targetMoved).toBe(first.targetMoved);
    expect(again.item).toEqual(first.item);
    expect(await head(projectId, "main")).toBe(after);
    expect(await audited()).toBe(1);
    // Another key on the merged item is REVIEW_CLOSED; the key on another item's merge is refused.
    expect(code(await merge(decided.item, "m2"))).toBe("REVIEW_CLOSED");
    await branch(projectId, "other", "ann", ["o1"]);
    const other = await request(projectId, "other");
    const otherDecided = ok<ReviewView>(await decide(other, [["o1", "accept"]]));
    expect(code(await merge(otherDecided.item, "m1"))).toBe("IDEM_KEY_REUSED");
  });
});

describe("review follow-ups (Task 7 review)", () => {
  test("B1: a rejection taken back after the source moved is merged", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    expect(rejected.stale).toBe(false);
    // The source moves, so the undrop below does not move the snapshot.
    await commitOn(projectId, feature, "f3", [edit("f3")]);
    const accepted = ok<ReviewView>(await decide(rejected.item, [["f1", "accept"], ["f2", "accept"]]));
    expect(accepted.stale).toBe(true);
    const merged = ok<{ item: ReviewItem; merge: MergeReport }>(await merge(accepted.item, "m1"));
    expect([...merged.merge.merged].sort()).toEqual(["f1", "f2"]);
    expect(merged.merge.notIncluded).toEqual(["f3"]);
  });

  test("B1: accepting a transaction someone else dropped undrops it", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    ok(await local.call("history.drop", { projectId, txId: "x-drop", author: "ann", from: "feature", into: "main", drops: ["f1"] }));
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "accept"]]));
    expect((await plainMerge(projectId, "feature")).dropped).toEqual([]);
    const merged = ok<{ merge: MergeReport }>(await merge(accepted.item, "m1"));
    expect([...merged.merge.merged].sort()).toEqual(["f1", "f2"]);
  });

  test("S1: a rejection undropped elsewhere can still be taken back", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"], ["f2", "reject"]]));
    ok(await local.call("history.drop", { projectId, txId: "x-undrop", author: "ann", from: "feature", into: "main", undrops: ["f1", "f2"] }));
    const before = await head(projectId, "main");
    const back = ok<ReviewView>(await decide(rejected.item, [["f1", "accept"], ["f2", "pending"]]));
    // Nothing was dropped, so nothing is undropped (no `notDropped`), and no record is written.
    expect(await head(projectId, "main")).toBe(before);
    expect(back.decisions.map((d) => [d.txId, d.decision])).toEqual([["f1", "accept"]]);
    // Rejecting again drops again.
    ok(await decide(back.item, [["f2", "reject"]]));
    expect((await plainMerge(projectId, "feature")).dropped).toEqual(["f2"]);
  });

  test("S2: a merge with nothing accepted is REVIEW_UNDECIDED and keeps the item open", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    const before = await head(projectId, "main");
    const refused = await merge(item, "m1");
    expect(code(refused)).toBe("REVIEW_UNDECIDED");
    expect(refused.ok ? undefined : refused.error.details).toMatchObject({ reason: "nothingAccepted", pending: 1 });
    expect(await head(projectId, "main")).toBe(before);
    expect((await get(item.id)).item.state).toBe("open");
    // Everything rejected: nothing is left open, so merging nothing is allowed.
    const all = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    expect(ok<{ item: ReviewItem; merge: MergeReport }>(await merge(all.item, "m1")).merge.merged).toEqual([]);
  });
});

describe("drops someone else made (Task 7 review, round 2)", () => {
  const drop = (projectId: string, txId: string, lists: object) => local.call("history.drop", { projectId, txId, author: "bob", from: "feature", into: "main", ...lists });

  /** The review rejects f1 (its own drop), then bob drops f1 again: the newest drop is bob's (round 3, B1). */
  async function rejectedThenBob() {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    ok(await drop(projectId, "bob-later", { drops: ["f1"] }));
    return { projectId, item: (await get(rejected.item.id)).item };
  }

  const auditRows = async () =>
    (await call<{ entries: Array<{ details: Record<string, unknown> }> }>("auth.audit.list", { kind: "history.drop" }, await connect("console"))).entries.filter((e) => e.details.dropTx !== undefined).map((e) => e.details);

  test("probe E: bob drops, the review rejects (joining it), then accepts: bob's record is named, not the review's", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    const accepted = ok<ReviewView>(await decide((await get(rejected.item.id)).item, [["f1", "accept"]]));
    expect(accepted.undropped).toEqual([{ txId: "f1", dropTx: "bob-1", droppedBy: "bob" }]);
    expect((await auditRows()).map((d) => [d.txId, d.dropTx, d.droppedBy])).toEqual([["f1", "bob-1", "other"]]);
    expect((await plainMerge(projectId, "feature")).dropped).toEqual([]);
  });

  test("probe A: bob drops twice: both records are listed and audited, newest first", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    ok(await drop(projectId, "bob-2", { drops: ["f1"] }));
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    expect(accepted.undropped?.map((d) => d.dropTx)).toEqual(["bob-2", "bob-1"]);
    expect((await auditRows()).map((d) => d.dropTx).sort()).toEqual(["bob-1", "bob-2"]);
  });

  test("probe F: a drop inherited from main is named on a review into a branch of main", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const dev = await call<{ branch: BranchView }>("branch.create", { projectId, txId: "b-dev", author: "ann", name: "dev", from: "main" });
    const { item } = await call<{ item: ReviewItem }>("review.request", { projectId, source: "feature", into: dev.branch.id, title: "Into dev" });
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    expect(accepted.undropped).toEqual([{ txId: "f1", dropTx: "bob-1", droppedBy: "bob" }]);
    expect((await auditRows()).map((d) => d.dropTx)).toEqual(["bob-1"]);
  });

  test("B1r3: an accept after a later drop by bob goes through the owner's rule: listed and audited", async () => {
    const { projectId, item } = await rejectedThenBob();
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    expect(accepted.undropped).toEqual([{ txId: "f1", dropTx: "bob-later", droppedBy: "bob" }]);
    expect((await plainMerge(projectId, "feature")).dropped).toEqual([]);
    const audit = await call<{ entries: Array<{ details: Record<string, unknown> }> }>("auth.audit.list", { kind: "history.drop" }, await connect("console"));
    expect(audit.entries.some((e) => e.details.dropTx === "bob-later" && e.details.txId === "f1")).toBe(true);
  });

  test("B1r3: pending after a later drop by bob writes no undrop; f1 stays dropped", async () => {
    const { projectId, item } = await rejectedThenBob();
    const before = await head(projectId, "main");
    const pending = ok<ReviewView>(await decide(item, [["f1", "pending"]]));
    expect(pending.undropped).toBeUndefined();
    expect(await head(projectId, "main")).toBe(before);
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f2"], dropped: ["f1"] });
  });

  test("B1r3: reject again after bob's drop, then pending: the review joined bob's drop, so f1 stays dropped", async () => {
    const { projectId, item } = await rejectedThenBob();
    const again = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    const pending = ok<ReviewView>(await decide(again.item, [["f1", "pending"]]));
    expect(pending.undropped).toBeUndefined();
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f2"], dropped: ["f1"] });
  });

  test("P1: pending after a rejection of something bob dropped keeps bob's drop", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const before = await head(projectId, "main");
    const rejected = ok<ReviewView>(await decide(item, [["f1", "reject"]]));
    // The review writes its own drop record even though f1 is dropped already.
    expect(await head(projectId, "main")).toBe(before + 1);
    ok(await decide(rejected.item, [["f1", "pending"]]));
    // Pending takes back the review's drop only: bob's still holds.
    expect((await plainMerge(projectId, "feature")).dropped).toEqual(["f1"]);
  });

  test("P2: a rejection bob undrops afterwards refuses the review merge until it is rejected again", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const decided = ok<ReviewView>(await decide(item, [["f1", "reject"], ["f2", "accept"]]));
    ok(await drop(projectId, "bob-2", { undrops: ["f1"] }));
    const before = await head(projectId, "main");
    const refused = await merge(decided.item, "m1");
    expect(code(refused)).toBe("REVIEW_UNDECIDED");
    expect(refused.ok ? undefined : refused.error.details).toMatchObject({ reason: "rejectedUndropped", txIds: ["f1"] });
    expect(await head(projectId, "main")).toBe(before);
    const again = ok<ReviewView>(await decide(decided.item, [["f1", "reject"]], {}, (await get(item.id)).item.snapshot!));
    expect(ok<{ merge: MergeReport }>(await merge(again.item, "m1")).merge.merged).toEqual(["f2"]);
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: [], dropped: ["f1"] });
  });

  test("P3: an accepted transaction dropped since refuses the merge (agent branch: nothing written, not archived)", async () => {
    const projectId = await project();
    const work = await branch(projectId, "feature", "agent:01jc0000000000000000000agt", ["f1"]);
    const item = await request(projectId, work);
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const before = await head(projectId, "main");
    const refused = await merge(accepted.item, "m1");
    expect(code(refused)).toBe("REVIEW_UNDECIDED");
    expect(refused.ok ? undefined : refused.error.details).toMatchObject({ reason: "acceptedDropped", txIds: ["f1"] });
    expect(await head(projectId, "main")).toBe(before);
    expect((await get(item.id)).item.state).toBe("open");
    expect((await branches(projectId)).find((b) => b.id === work)?.state).not.toBe("archived");
  });

  test("P6: accepted f1 dropped elsewhere, f2 rejected: refused, the item stays open", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    const decided = ok<ReviewView>(await decide(item, [["f1", "accept"], ["f2", "reject"]]));
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    const refused = await merge(decided.item, "m1");
    expect(refused.ok ? undefined : refused.error.details).toMatchObject({ reason: "acceptedDropped", txIds: ["f1"] });
    expect((await get(item.id)).item.state).toBe("open");
    // Accepting it again undrops it (and says so); the merge then copies it.
    const fresh = (await get(item.id)).item;
    const back = ok<ReviewView>(await decide(fresh, [["f1", "accept"]]));
    expect(back.undropped).toEqual([{ txId: "f1", dropTx: "bob-1", droppedBy: "bob" }]);
    expect(ok<{ merge: MergeReport }>(await merge(back.item, "m1")).merge.merged).toEqual(["f1"]);
  });

  test("P4: an accept that lifts someone else's drop says so in `undropped`; lifting the review's own does not", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2"]);
    const item = await request(projectId, "feature");
    ok(await drop(projectId, "bob-1", { drops: ["f1"] }));
    // The cached list (planned before bob's drop) still shows f1 to merge.
    expect((await get(item.id)).plan.entries.find((e) => e.origin === "f1")?.status).toBe("willMerge");
    const accepted = ok<ReviewView>(await decide(item, [["f1", "accept"]]));
    // `local` holds `review.decide` on main: it may cancel bob's drop, and it is told and audited.
    expect(accepted.undropped).toEqual([{ txId: "f1", dropTx: "bob-1", droppedBy: "bob" }]);
    expect((await plainMerge(projectId, "feature")).dropped).toEqual([]);
    const audit = await call<{ entries: Array<{ kind: string; principal: string; details: Record<string, unknown> }> }>("auth.audit.list", { kind: "history.drop" }, await connect("console"));
    const row = audit.entries.find((e) => e.details.dropTx !== undefined);
    expect(row?.details).toMatchObject({ item: item.id, txId: "f1", dropTx: "bob-1", droppedBy: "other", count: 1 });
    expect(JSON.stringify(row)).toContain("local");
    const fresh = (await get(item.id)).item;
    const own = ok<ReviewView>(await decide(fresh, [["f2", "reject"]]));
    const backAgain = ok<ReviewView>(await decide(own.item, [["f2", "accept"]]));
    expect(backAgain.undropped).toBeUndefined();
  });
});

describe("the workflow on a fake engine", () => {
  const snapshot = { sourceSeq: 4, sourceHash: "h1", targetSeq: 2, targetHash: "t", plannedAt: 0 };
  const entry = (origin: string, seq: number) => ({ origin, txId: origin, branch: "s", seq, kind: "edit" as const, author: "a", opsCount: 1, committedAt: 0, status: "willMerge" as const });
  function setup(plan: ReviewPlan, failDecisions = false) {
    mkdirSync(join(tmp, "control"), { recursive: true, mode: 0o700 });
    const control = openControlStore(join(tmp, "control"), { serverId: "sid-1", checkpoint: false });
    onTestFinished(() => control.close());
    const store = new ReviewStore(control);
    const writes: string[] = [];
    const engine: ReviewEngine = {
      resolveBranch: (_, r) => Effect.succeed(r),
      branches: () => Effect.succeed([]),
      plan: () => Effect.succeed(plan),
      records: () => Effect.succeed([]),
      write: (_, op) => {
        writes.push(op);
        return Effect.succeed({ rev: 3, tx: { txId: "d", branch: "main" }, bindings: {} });
      },
    };
    if (failDecisions) {
      store.setDecisions = () => {
        throw new Error("control.db is gone");
      };
    }
    const w = new ReviewWorkflow({ store, engine });
    const item = store.open({ projectId: "p", kind: "branchReview", source: "s", target: "main", snapshot, title: "t", requestedBy: { principal: "local:local" } });
    return { w, item, writes, caller: callerOf(LOCAL, { actor: { kind: "user" }, guard: { denyBranches: [] } }, "pipe") };
  }

  for (const [name, head, entries] of [
    // Rewound and re-resolved to the same seq with another record (hash h2): `r4` was never in the snapshot.
    ["to the snapshot's seq with another hash", { seq: 4, hash: "h2" }, [entry("f1", 3), entry("r4", 4)]],
    // Rewound below the snapshot's seq: `r3` replaced the snapshot's seq-3 record.
    ["below the snapshot's seq", { seq: 3, hash: "h3" }, [entry("r3", 3)]],
  ] as const) {
    test(`S3: after a cold cache and a source rewound ${name}, acceptPending is REVIEW_STALE`, async () => {
      const plan: ReviewPlan = { source: { branch: "s", ...head }, target: { branch: "main", seq: 2, hash: "t" }, entries: [...entries], conflicts: [], truncated: false };
      const { w, item, writes, caller } = setup(plan);
      const exit = await Effect.runPromiseExit(mergeReview(w, caller, { itemId: item.id, expect: snapshot, acceptPending: true, idemKey: "m1" }));
      expect(JSON.stringify(exit)).toContain("REVIEW_STALE");
      expect(JSON.stringify(exit)).toContain("replan");
      expect(writes).toEqual([]);
    });
  }

  test("a truncated fresh plan is REVIEW_STALE {reason: truncated}, never origins counted as not open", async () => {
    const plan: ReviewPlan = { source: { branch: "s", seq: 4, hash: "h1" }, target: { branch: "main", seq: 2, hash: "t" }, entries: [entry("f1", 3)], conflicts: [], truncated: true };
    const { w, item, writes, caller } = setup(plan);
    const runs: ReadonlyArray<Effect.Effect<unknown, unknown>> = [
      w.decide(caller, { itemId: item.id, expect: snapshot, decisions: [{ txId: "f1", decision: "reject" }] }),
      mergeReview(w, caller, { itemId: item.id, expect: snapshot, acceptPending: true, idemKey: "m1" }),
    ];
    for (const run of runs) {
      const exit = JSON.stringify(await Effect.runPromiseExit(run));
      expect(exit).toContain("REVIEW_STALE");
      expect(exit).toContain("truncated");
    }
    expect(writes).toEqual([]);
  });

  test("N2: a dropped origin with no drop record to name is REVIEW_STALE {dropUnknown}; nothing is written", async () => {
    const plan: ReviewPlan = { source: { branch: "s", seq: 4, hash: "h1" }, target: { branch: "main", seq: 2, hash: "t" }, entries: [{ ...entry("f1", 3), status: "dropped" }], conflicts: [], truncated: false };
    const { w, item, writes, caller } = setup(plan);
    for (const decision of ["accept", "pending"] as const) {
      const exit = JSON.stringify(await Effect.runPromiseExit(w.decide(caller, { itemId: item.id, expect: snapshot, decisions: [{ txId: "f1", decision }] })));
      expect(exit).toContain("REVIEW_STALE");
      expect(exit).toContain("dropUnknown");
    }
    expect(writes).toEqual([]);
    expect(w.store.decisions(item.id).size).toBe(0);
  });

  test("an accept against another's drop by a principal whose reach leaves out the target is CONFLICT {droppedByOther}; nothing is written", async () => {
    const plan: ReviewPlan = { source: { branch: "s", seq: 4, hash: "h1" }, target: { branch: "main", seq: 2, hash: "t" }, entries: [{ ...entry("f1", 3), status: "dropped" }], conflicts: [], truncated: false };
    const { w, item, writes } = setup(plan);
    const dropRecord = { rev: 2, tx: { txId: "bob-1", author: "agent:01jc0000000000000000000bob", branch: "main" }, link: { kind: "drop", from: "s", drops: ["f1"] }, bindings: {} };
    (w.engine as { records: ReviewEngine["records"] }).records = () => Effect.succeed([dropRecord as never]);
    const reviewer = { ...LOCAL, kind: "agent" as const, id: "01jc0000000000000000000rev", scopes: [...LOCAL.scopes] };
    for (const guard of [{ denyBranches: [], allowBranches: ["s"] }, { denyBranches: ["main"] }]) {
      const caller = callerOf(reviewer, { actor: { kind: "agent" }, guard }, "pipe");
      const exit = JSON.stringify(await Effect.runPromiseExit(w.decide(caller, { itemId: item.id, expect: snapshot, decisions: [{ txId: "f1", decision: "accept" }] })));
      expect(exit).toContain("CONFLICT");
      expect(exit).toContain("droppedByOther");
      expect(exit).toContain("agent:01jc0000000000000000000bob");
    }
    // Without the scope too, whatever the reach.
    const unscoped = callerOf({ ...reviewer, scopes: reviewer.scopes.filter((x) => x !== "review.decide") }, { actor: { kind: "agent" }, guard: { denyBranches: [] } }, "pipe");
    expect(JSON.stringify(await Effect.runPromiseExit(w.decide(unscoped, { itemId: item.id, expect: snapshot, decisions: [{ txId: "f1", decision: "accept" }] })))).toContain("droppedByOther");
    expect(writes).toEqual([]);
    expect(w.store.decisions(item.id).size).toBe(0);
    expect(w.store.get(item.id)?.state).toBe("open");
  });

  test("N6: the drop is durable before the item changes", async () => {
    const plan: ReviewPlan = { source: { branch: "s", seq: 4, hash: "h1" }, target: { branch: "main", seq: 2, hash: "t" }, entries: [entry("f1", 3)], conflicts: [], truncated: false };
    const { w, item, writes, caller } = setup(plan, true);
    const exit = await Effect.runPromiseExit(w.decide(caller, { itemId: item.id, expect: snapshot, decisions: [{ txId: "f1", decision: "reject" }] }));
    expect(exit._tag).toBe("Failure");
    // The drop record was written; the item's decisions, which failed after it, are not.
    expect(writes).toEqual(["drop"]);
    expect(w.store.decisions(item.id).size).toBe(0);
  });
});

describe("close", () => {
  test("close_rejected_drops_every_unmerged_entry", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1", "f2", "f3"]);
    const item = await request(projectId, "feature");
    ok(await decide(item, [["f1", "accept"], ["f3", "reject"]]));
    const closed = await call<{ item: ReviewItem }>("review.close", { itemId: item.id, state: "rejected" });
    expect(closed.item).toMatchObject({ state: "rejected", closedBy: "local:local" });
    // Accepted, pending and rejected alike: none comes back through a plain merge.
    const plain = await plainMerge(projectId, "feature");
    expect(plain.merged).toEqual([]);
    expect([...plain.dropped].sort()).toEqual(["f1", "f2", "f3"]);
  });

  test("close_dismissed_writes_nothing", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    const before = await head(projectId, "main");
    const closed = await call<{ item: ReviewItem }>("review.close", { itemId: item.id, state: "dismissed" });
    expect(closed.item.state).toBe("dismissed");
    expect(await head(projectId, "main")).toBe(before);
    expect(await plainMerge(projectId, "feature")).toEqual({ merged: ["f1"], dropped: [] });
  });

  test("closed_item_refuses_writes", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    ok(await local.call("review.close", { itemId: item.id, state: "dismissed" }));
    for (const exit of [
      await decide(item, [["f1", "accept"]]),
      await merge(item, "m1"),
      await local.call("review.close", { itemId: item.id, state: "rejected" }),
      await local.call("review.comment", { itemId: item.id, body: "late" }),
      await local.call("review.refresh", { itemId: item.id }),
    ]) {
      expect(code(exit)).toBe("REVIEW_CLOSED");
    }
    // A new request opens a new item for the pair.
    expect((await request(projectId, "feature")).id).not.toBe(item.id);
  });
});

describe("changes requested and the watch", () => {
  test("changes_requested_published_and_reopened", async () => {
    const projectId = await project();
    const feature = await branch(projectId, "feature", "ann", ["f1"]);
    const item = await request(projectId, "feature");
    const seen: Array<{ type: string; item?: ReviewItem }> = [];
    const watcher = await connect("local");
    const watch = watcher.stream("review.watch", { projectId }, (v) => seen.push(v as { type: string; item?: ReviewItem }));
    await waitFor(() => seen.length >= 1);
    const asked = ok<ReviewView>(await decide(item, [], { requestChanges: { reason: "too loud" } }));
    expect(asked.item.state).toBe("changesRequested");
    await waitFor(() => seen.some((e) => e.item?.state === "changesRequested"));
    // Someone else's refresh keeps it handed back; the requester's hands it in again.
    const console = await connect("console");
    expect((await call<ReviewView>("review.refresh", { itemId: item.id }, console)).item.state).toBe("changesRequested");
    await commitOn(projectId, feature, "f2", [edit("f2")]);
    const reopened = await call<ReviewView>("review.refresh", { itemId: item.id });
    expect(reopened.item.state).toBe("open");
    await waitFor(() => seen.some((e) => e.item?.state === "open"));
    watcher.interrupt(watch.id);
  });

  test("watch_snapshot_first_and_closed_on_revocation", async () => {
    const projectId = await project();
    await branch(projectId, "feature", "ann", ["f1"]);
    const first = await request(projectId, "feature");
    const console = await connect("console");
    const { credential, file } = ok<{ credential: { id: string }; file: CredentialFile }>(await confirmed(console, "auth.credential.mint", { kind: "agent", label: "test" }));
    const agent = await agentClient(file);
    const seen: Array<{ type: string; items?: ReviewItem[]; item?: ReviewItem }> = [];
    const watch = agent.stream("review.watch", { projectId }, (v) => seen.push(v as (typeof seen)[number]));
    await waitFor(() => seen.length >= 1);
    expect(seen[0]).toMatchObject({ type: "snapshot" });
    expect(seen[0]!.items?.map((i) => i.id)).toEqual([first.id]);
    await call("review.comment", { itemId: first.id, body: "a note" });
    await waitFor(() => seen.some((e) => e.type === "item" && e.item?.id === first.id));
    ok(await confirmed(console, "auth.credential.revoke", { id: credential.id }));
    expect(code(await watch.done)).toBe("CREDENTIAL_REVOKED");
  });
});

async function waitFor(f: () => boolean, ms = 5000): Promise<void> {
  const until = Date.now() + ms;
  while (!f()) {
    if (Date.now() > until) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** Connects and proves `file` as `fin` does. */
async function agentClient(file: CredentialFile): Promise<NdjsonClient> {
  const info = running!.info;
  const client = await NdjsonClient.connect(info.socket);
  clients.push(client);
  const key = b64u.decode(file.key)!;
  const { clientNonce, exit } = await hello(client, { env: file.env, endpoint: canonicalEndpoint(info.socket), credential: { kind: file.kind, id: file.id } });
  const answer = ok<{ envId: string; endpoint: string; serverNonce: string }>(exit);
  const mac = proveMac(key, answer.envId, answer.endpoint, b64u.decode(answer.serverNonce)!, clientNonce);
  ok(await client.call("auth.prove", { mac: b64u.encode(mac) }));
  return client;
}
