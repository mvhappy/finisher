// Re-derived reviews (5E plan Task 8, Decision 22) through a real server: when a project is
// loaded, active agent branches with changes to merge and no review get a `branchReview`
// marked `rederived`; human, archived, merged and already reviewed branches do not; at most 64
// per load; after a restart, after `control.db` was reset, and idempotent.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ReviewItem } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { serve, StderrLogger } from "../src/main.ts";
import { addAsset, commit, connectProved, type NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];
let local: NdjsonClient;

async function start(): Promise<void> {
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  running = { fiber, info: await info };
  local = await connectProved({ info: running.info, serverData }, "local");
  clients.push(local);
}

async function stop(): Promise<void> {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  if (running !== undefined) await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
}

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-rederive-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  await start();
});

afterEach(async () => {
  await stop();
  rmSync(tmp, { recursive: true, force: true });
});

const call = async <A>(tag: string, payload: object): Promise<A> => ok<A>(await local.call(tag, payload));
const edit = (txId: string) => ({ ...addAsset(`$${txId}`), path: `/media/${txId}.mov`, fingerprint: txId });
const AGENT = "agent:01jc0000000000000000000agt";
const MCP = "mcp:01jc0000000000000000000mcp";

async function project(): Promise<string> {
  const { projectId } = await call<{ projectId: string }>("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" });
  ok(await commit(local, projectId, tx("m-asset", 0, [addAsset()])));
  return projectId;
}
const branches = async (projectId: string) => (await call<{ branches: BranchView[] }>("branch.list", { projectId, all: true })).branches;

/** A branch of `main` by `author` with one commit per id in `txIds`. */
async function branch(projectId: string, name: string, author: string, txIds: ReadonlyArray<string>): Promise<string> {
  const made = await call<{ branch: BranchView }>("branch.create", { projectId, txId: `b-${name}`, author, name, from: "main" });
  let base = made.branch.head;
  for (const t of txIds) {
    ok(await commit(local, projectId, { ...tx(t, base, [edit(t)]), branch: made.branch.id, author }));
    base++;
  }
  return made.branch.id;
}

const items = async (projectId: string, state?: string) => (await call<{ items: ReviewItem[] }>("review.list", { projectId, kind: "branchReview", ...(state === undefined ? {} : { state }) })).items;

/** Loads the project again (a restart or a close) and waits until the items stop changing. */
async function reload(projectId: string, how: "restart" | "close"): Promise<ReviewItem[]> {
  if (how === "restart") {
    await stop();
    await start();
  } else {
    ok(await local.call("project.close", { projectId }));
  }
  await branches(projectId);
  return settled(projectId);
}

/** The items once two reads 150 ms apart agree (the re-derivation runs in the background). */
async function settled(projectId: string): Promise<ReviewItem[]> {
  let last = JSON.stringify((await items(projectId)).map((i) => i.id).sort());
  for (let i = 0; i < 100; i++) {
    await new Promise((r) => setTimeout(r, 150));
    const now = await items(projectId);
    const key = JSON.stringify(now.map((x) => x.id).sort());
    if (key === last && (now.length > 0 || i > 8)) return now;
    last = key;
  }
  throw new Error("re-derived items did not settle");
}

describe("re-derivation", () => {
  test("rederive_on_load_for_active_agent_branches", async () => {
    const projectId = await project();
    const a = await branch(projectId, "agent-work", AGENT, ["a1", "a2"]);
    const m = await branch(projectId, "mcp-work", MCP, ["x1"]);
    const human = await branch(projectId, "human-work", "ann", ["h1"]);
    const archived = await branch(projectId, "old-work", AGENT, ["o1"]);
    ok(await local.call("branch.set", { projectId, txId: "arch", author: "ann", branch: archived, archived: true }));
    // Merged: a plain merge brought everything in, so nothing is left to review.
    const merged = await branch(projectId, "done-work", AGENT, ["d1"]);
    ok(await local.call("history.merge", { projectId, txId: "merge-d", author: "ann", from: merged, into: "main" }));
    // Already reviewed (the person dismissed it): not made again.
    const reviewed = await branch(projectId, "seen-work", AGENT, ["s1"]);
    const seen = (await call<{ item: ReviewItem }>("review.request", { projectId, source: reviewed, title: "Look" })).item;
    ok(await local.call("review.close", { itemId: seen.id, state: "dismissed" }));
    expect(await items(projectId)).toHaveLength(1);

    const made = (await reload(projectId, "restart")).filter((i) => i.id !== seen.id);
    expect(made.map((i) => i.source).sort()).toEqual([a, m].sort());
    for (const item of made) {
      expect(item).toMatchObject({ kind: "branchReview", state: "open", target: "main", rederived: true });
      expect(item.snapshot).toBeDefined();
      expect(item.title).toMatch(/\S/);
    }
    expect(made.some((i) => i.source === human || i.source === archived || i.source === merged || i.source === reviewed)).toBe(false);
    // A re-derived item is a branch review like any other.
    const view = await call<{ plan: { entries: Array<{ origin: string }> } }>("review.get", { itemId: made.find((i) => i.source === a)!.id });
    expect(view.plan.entries.map((e) => e.origin)).toEqual(expect.arrayContaining(["a1", "a2"]));
  });

  test("rederive_makes_at_most_64_per_load", async () => {
    const projectId = await project();
    for (let i = 0; i < 66; i++) await branch(projectId, `w${i}`, AGENT, [`t${i}`]);
    expect(await items(projectId)).toHaveLength(0);
    expect(await reload(projectId, "restart")).toHaveLength(64);
    // The rest on the next load.
    expect(await reload(projectId, "close")).toHaveLength(66);
  }, 120_000);

  test("rederive_after_control_reset", async () => {
    const projectId = await project();
    const a = await branch(projectId, "agent-work", AGENT, ["a1"]);
    expect(await reload(projectId, "restart")).toHaveLength(1);
    // Stop the server and wreck `control.db`: it is moved aside and starts empty (5A Decision 3).
    await stop();
    for (const suffix of ["", "-wal", "-shm"]) rmSync(join(serverData, `control.db${suffix}`), { force: true });
    writeFileSync(join(serverData, "control.db"), "this is not a database, it is a text file that is long enough to be one".repeat(40));
    await start();
    await branches(projectId);
    const again = await settled(projectId);
    expect(again.map((i) => i.source)).toEqual([a]);
    expect(again[0]).toMatchObject({ rederived: true, state: "open" });
  });

  test("rederive_after_a_partial_merge_reviews_what_is_left", async () => {
    const projectId = await project();
    const a = await branch(projectId, "agent-work", AGENT, ["a1", "a2"]);
    const item = (await call<{ item: ReviewItem }>("review.request", { projectId, source: a, title: "Look" })).item;
    const decided = ok<{ item: ReviewItem }>(await local.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [{ txId: "a1", decision: "accept" }] }));
    ok(await local.call("review.merge", { itemId: item.id, expect: decided.item.snapshot, idemKey: "pm-1" }));
    expect((await items(projectId, "merged")).map((i) => i.id)).toEqual([item.id]);
    // a2 is still unmerged on an active branch: the next load reviews it, a1 is not listed.
    const after = await reload(projectId, "restart");
    const fresh = after.filter((i) => i.id !== item.id);
    expect(fresh).toMatchObject([{ source: a, state: "open", rederived: true }]);
    const view = await call<{ plan: { entries: Array<{ origin: string; status: string }> } }>("review.get", { itemId: fresh[0]!.id });
    expect(view.plan.entries.filter((e) => e.status === "willMerge").map((e) => e.origin)).toEqual(["a2"]);
    // An open one is not doubled by another load; a rejected one is not made again.
    expect(await reload(projectId, "close")).toHaveLength(2);
    ok(await local.call("review.close", { itemId: fresh[0]!.id, state: "rejected" }));
    expect(await reload(projectId, "close")).toHaveLength(2);
  });

  test("rederive_is_idempotent", async () => {
    const projectId = await project();
    const a = await branch(projectId, "agent-work", AGENT, ["a1"]);
    const first = await reload(projectId, "restart");
    expect(first.map((i) => i.source)).toEqual([a]);
    // Loading again makes nothing new, whether the item is open or was closed.
    expect((await reload(projectId, "close")).map((i) => i.id)).toEqual([first[0]!.id]);
    ok(await local.call("review.close", { itemId: first[0]!.id, state: "dismissed" }));
    expect((await reload(projectId, "close")).map((i) => i.id)).toEqual([first[0]!.id]);
    expect((await items(projectId, "open"))).toEqual([]);
  });
});
