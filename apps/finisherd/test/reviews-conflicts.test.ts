// Merge conflicts as review items (5E plan Task 8, Decision 21) through a real server: a
// `CONFLICT` from `history.merge` or `review.merge` opens or updates the pair's `mergeConflict`
// item; a later successful merge or archiving the source supersedes it; a rebase conflict on
// `tx.commit` opens nothing; `pluginSuggestion` and `mergeConflict` items are not decided or merged.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ReviewItem, ReviewView } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { serve, StderrLogger } from "../src/main.ts";
import { addAsset, commit, connectProved, type Exit, type NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];
let local: NdjsonClient;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-conflicts-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  running = { fiber, info: await info };
  local = await connectProved({ info: running.info, serverData }, "local");
  clients.push(local);
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  if (running !== undefined) await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const code = (e: Exit): string | undefined => (e.ok ? undefined : e.error.code);
const call = async <A>(tag: string, payload: object): Promise<A> => ok<A>(await local.call(tag, payload));
const edit = (txId: string) => ({ ...addAsset(`$${txId}`), path: `/media/${txId}.mov`, fingerprint: txId });
const del = { op: "clip.delete", clip: "c1" };

/** A project whose `main` holds asset `a1` and clip `c1`. */
async function project(): Promise<string> {
  const { projectId } = await call<{ projectId: string }>("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" });
  ok(await commit(local, projectId, tx("m-asset", 0, [addAsset()])));
  ok(await commit(local, projectId, tx("m-clip", 1, [{ op: "clip.insert", id: "$c", track: "t1", at: "0", asset: "a1", sourceIn: "0", duration: "5" }])));
  return projectId;
}
const branches = async (projectId: string) => (await call<{ branches: BranchView[] }>("branch.list", { projectId, all: true })).branches;
const head = async (projectId: string, id: string) => (await branches(projectId)).find((b) => b.id === id)!.head;

async function commitOn(projectId: string, branchId: string, txId: string, ops: unknown[]): Promise<Exit> {
  return commit(local, projectId, { ...tx(txId, await head(projectId, branchId), ops), branch: branchId });
}

/** `feature` (by `ann`) deletes `c1`, and so does `main`: merging `feature` conflicts. */
async function conflicting(): Promise<{ projectId: string; feature: string }> {
  const projectId = await project();
  const made = await call<{ branch: BranchView }>("branch.create", { projectId, txId: "b-feature", author: "ann", name: "feature", from: "main" });
  const feature = made.branch.id;
  ok(await commitOn(projectId, feature, "f1", [del]));
  ok(await commitOn(projectId, "main", "m2", [del]));
  return { projectId, feature };
}

let n = 0;
const merge = (projectId: string, extra: object = {}) =>
  local.call("history.merge", { projectId, txId: `merge-${++n}`, author: "ann", from: "feature", into: "main", ...extra });
const conflictItems = async (projectId: string) => (await call<{ items: ReviewItem[] }>("review.list", { projectId, kind: "mergeConflict" })).items;

describe("merge conflicts", () => {
  test("conflict_opens_item", async () => {
    const { projectId, feature } = await conflicting();
    expect(code(await merge(projectId))).toBe("CONFLICT");
    const items = await conflictItems(projectId);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "mergeConflict", state: "open", source: feature, target: "main", rederived: false, requestedBy: { principal: "local:local" } });
    expect(JSON.stringify(items[0]!.subject)).toContain("f1");
    // A dry run writes nothing and changes nothing.
    expect(code(await merge(projectId, { dryRun: true }))).toBe("CONFLICT");
    expect(await conflictItems(projectId)).toHaveLength(1);
  });

  test("second_conflict_updates_item", async () => {
    const { projectId, feature } = await conflicting();
    await merge(projectId);
    const [first] = await conflictItems(projectId);
    ok(await commitOn(projectId, feature, "f2", [{ op: "clip.delete", clip: "c1" }]).then((e) => (e.ok ? e : commitOn(projectId, feature, "f2b", [edit("f2b")]))));
    expect(code(await merge(projectId))).toBe("CONFLICT");
    const items = await conflictItems(projectId);
    expect(items).toHaveLength(1);
    expect(items[0]!.id).toBe(first!.id);
    expect(items[0]!.updatedAt).toBeGreaterThanOrEqual(first!.updatedAt);
    expect(items[0]!.state).toBe("open");
  });

  test("successful_merge_supersedes", async () => {
    const { projectId } = await conflicting();
    await merge(projectId);
    const [item] = await conflictItems(projectId);
    // The conflicting transaction is left out, the merge succeeds: the item is done.
    ok(await merge(projectId, { skipConflicting: true }));
    expect((await conflictItems(projectId)).find((i) => i.id === item!.id)?.state).toBe("superseded");
    expect((await conflictItems(projectId)).filter((i) => i.state === "open")).toEqual([]);
  });

  test("archived_source_supersedes", async () => {
    const { projectId, feature } = await conflicting();
    await merge(projectId);
    const [item] = await conflictItems(projectId);
    ok(await local.call("branch.set", { projectId, txId: "arch", author: "ann", branch: feature, archived: true }));
    expect((await conflictItems(projectId)).find((i) => i.id === item!.id)?.state).toBe("superseded");
  });

  test("review_merge_conflict_goes_through_observer", async () => {
    const { projectId, feature } = await conflicting();
    const item = (await call<{ item: ReviewItem }>("review.request", { projectId, source: feature, title: "Please look" })).item;
    const decided = ok<ReviewView>(await local.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [{ txId: "f1", decision: "accept" }] }));
    const refused = await local.call("review.merge", { itemId: item.id, expect: decided.item.snapshot, idemKey: "rm-1" });
    expect(code(refused)).toBe("REVIEW_HAS_CONFLICTS");
    expect(await conflictItems(projectId)).toMatchObject([{ state: "open", source: feature, target: "main" }]);
    // The review itself is still open.
    expect(ok<ReviewView>(await local.call("review.get", { itemId: item.id })).item.state).toBe("open");
  });

  test("tx_commit_rebase_conflict_opens_nothing", async () => {
    const projectId = await project();
    const base = await head(projectId, "main");
    ok(await commit(local, projectId, tx("m2", base, [del])));
    // A commit based on an older head that deletes the same clip cannot rebase.
    const stale = await commit(local, projectId, tx("m3", base, [del]));
    expect(stale.ok).toBe(false);
    expect(await conflictItems(projectId)).toEqual([]);
  });
});

describe("items of other kinds", () => {
  test("decide_on_suggestion_is_invalid_request", async () => {
    const { projectId } = await conflicting();
    ok(await local.call("plugin.suggest", { id: "@acme/pack", reason: "captions", projectId }));
    await merge(projectId);
    const items = (await call<{ items: ReviewItem[] }>("review.list", { projectId })).items;
    expect(items.map((i) => i.kind).sort()).toEqual(["mergeConflict", "pluginSuggestion"]);
    const snapshot = { sourceSeq: 0, targetSeq: 0, plannedAt: 0 };
    for (const item of items) {
      for (const [tag, payload] of [
        ["review.decide", { itemId: item.id, expect: snapshot, decisions: [] }],
        ["review.merge", { itemId: item.id, expect: snapshot, idemKey: "k1" }],
        ["review.refresh", { itemId: item.id }],
      ] as const) {
        const r = await local.call(tag, payload);
        expect(code(r), `${item.kind} ${tag}`).toBe("INVALID_REQUEST");
        expect(r.ok ? undefined : r.error.details).toMatchObject({ reason: "kind", kind: item.kind });
      }
    }
    // A person closes a suggestion.
    const suggestion = items.find((i) => i.kind === "pluginSuggestion")!;
    expect(suggestion).toMatchObject({ title: "@acme/pack", summary: "captions", subject: { pluginId: "@acme/pack" } });
    expect(ok<{ item: ReviewItem }>(await local.call("review.close", { itemId: suggestion.id, state: "dismissed" })).item.state).toBe("dismissed");
  });
});
