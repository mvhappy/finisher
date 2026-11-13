// 7A Task 11: the outcome tracker (Decision 27; spec §12.5). Against a real engine on a server:
// merges, cherry-picks, undos, reverts, archives and deletes of agent transactions become
// outcomes in `session_txs` (one per `(txId, via)`), and 5E's `review.verdict` is emitted
// once that kind is registered (5E Task 4: v1).
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Principal, Session } from "@finisher/contracts";
import { kindVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { journalVerdicts, type OutcomeVerdict, OutcomeTracker, type VerdictSink } from "../src/agent/outcomes.ts";
import { committedOf } from "../src/history.ts";
import { LOCAL } from "../src/principal.ts";
import { AgentServer, open } from "./support/agent-server.ts";
import { addAsset, commit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-outcomes-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

async function setUp() {
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

async function session(local: NdjsonClient, projectId: string, o: { role?: string; base?: string } = {}) {
  const s = await open(server.core, local, projectId, o);
  return { session: s.session, client: await server.agent(s.credentialPath) };
}

/** A commit on `branch` as `c`; its txId. */
async function edit(c: NdjsonClient, projectId: string, branch: string, at = 0): Promise<string> {
  const txId = id();
  ok(await commit(c, projectId, { ...tx(txId, 1, [insert(at)]), branch }));
  return txId;
}

const call = (c: NdjsonClient, tag: string, payload: Record<string, unknown>) => c.call(tag, { txId: id(), author: "someone", ...payload });

const outcomes = (txId: string) => server.core.store.outcomes.of(txId).map((o) => ({ verdict: o.verdict, via: o.via, by: o.by }));
const latest = (txId: string) => server.core.store.sessionOfTx([txId]).get(txId)?.outcome;

/** A sink that records what it was given; `registered` says whether 5E's kind exists. */
function spySink(registered: boolean): VerdictSink & { readonly got: Array<{ v: OutcomeVerdict; projectId?: string; version: number }> } {
  const got: Array<{ v: OutcomeVerdict; projectId?: string; version: number }> = [];
  return {
    got,
    version: () => (registered ? 1 : null),
    emit: (v, projectId, version) => {
      got.push({ v, ...(projectId === undefined ? {} : { projectId }), version });
      return true;
    },
  };
}

describe("outcomes on a server", () => {
  test("undone_by_agent, merged_by_a_person, then reverted_by_user, kept in sequence", async () => {
    const { local, projectId } = await setUp();
    const { session: s, client: a } = await session(local, projectId);
    const root = s.rootBranch!;
    const txs = [await edit(a, projectId, root, 0), await edit(a, projectId, root, 1), await edit(a, projectId, root, 2)];
    // The agent undoes its newest on its own branch (4B: before a merge carries it away).
    const u = ok<{ origins: string[] }>(await call(a, "history.undo", { projectId, branch: root }));
    expect(u.origins).toEqual([txs[2]]);
    expect(outcomes(txs[2]!)).toEqual([{ verdict: "undone", via: "undo", by: "agent" }]);
    // A plain `fin merge` (no review item) by a person.
    const m = ok<{ mergeId: string; origins: string[] }>(await call(local, "history.merge", { projectId, from: root, into: "main" }));
    expect(m.origins).toEqual(expect.arrayContaining([txs[0], txs[1]]));
    for (const t of txs.slice(0, 2)) {
      expect(outcomes(t)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
      expect(latest(t)).toBe("merged");
    }
    // The merge carried the undone edit and its undo together: the sequence says both.
    expect(outcomes(txs[2]!)).toEqual([
      { verdict: "undone", via: "undo", by: "agent" },
      { verdict: "merged", via: "merge", by: "user" },
    ]);
    // A person reverts the merge on main: every transaction it carried is reverted.
    ok(await call(local, "history.revert", { projectId, mergeId: m.mergeId, branch: "main" }));
    for (const t of txs.slice(0, 2)) {
      expect(outcomes(t)).toEqual([
        { verdict: "merged", via: "merge", by: "user" },
        { verdict: "reverted", via: "revert", by: "user" },
      ]);
      expect(latest(t)).toBe("reverted");
    }
    // 5E's kind exists (v1): fin-core validated every outcome as `review.verdict` and the
    // journal took it.
    expect(kindVersion("review.verdict", "agent")).toBe(1);
    const all = server.core.store.outcomes.bySession(s.id);
    expect(all.length).toBeGreaterThan(0);
    expect(all.every((o) => o.emitted)).toBe(true);
  });

  test("abandoned_when_archived_unmerged: a merged one is not; another session's are not", async () => {
    const { local, projectId } = await setUp();
    const one = await session(local, projectId);
    const two = await session(local, projectId);
    const merged = await edit(one.client, projectId, one.session.rootBranch!);
    ok(await call(local, "history.merge", { projectId, from: one.session.rootBranch!, into: "main" }));
    const open1 = await edit(one.client, projectId, one.session.rootBranch!, 3);
    const open2 = await edit(two.client, projectId, two.session.rootBranch!);
    ok(await call(local, "branch.set", { projectId, branch: one.session.rootBranch!, archived: true }));
    expect(outcomes(open1)).toEqual([{ verdict: "abandoned", via: "archive", by: "user" }]);
    expect(outcomes(merged)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
    expect(outcomes(open2)).toEqual([]);
    // Restoring and archiving again records nothing new (one per (txId, via)).
    ok(await call(local, "branch.set", { projectId, branch: one.session.rootBranch!, archived: false }));
    ok(await call(local, "branch.set", { projectId, branch: one.session.rootBranch!, archived: true }));
    expect(outcomes(open1)).toHaveLength(1);
  });

  test("cherry_pick_into_main_is_merged; a child merged into its own root is not, until the root is merged", async () => {
    const { local, projectId } = await setUp();
    const { session: s, client: a } = await session(local, projectId);
    const root = s.rootBranch!;
    const picked = await edit(a, projectId, root);
    ok(await call(local, "history.cherryPick", { projectId, origin: picked, into: "main" }));
    expect(outcomes(picked)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
    // Inside the subtree: the agent merges a child into its root.
    const child = ok<{ branch: { id: string } }>(await call(a, "branch.create", { projectId, from: root, name: "try-1" })).branch.id;
    const inChild = await edit(a, projectId, child, 5);
    const g = ok<{ committed: Array<{ tx: { txId: string } }> }>(await call(a, "history.merge", { projectId, from: child, into: root }));
    const copy = g.committed[0]!.tx.txId;
    expect(outcomes(inChild)).toEqual([]);
    expect(outcomes(copy)).toEqual([]);
    // The root into main: the copy and the edit it copies are both merged.
    ok(await call(local, "history.merge", { projectId, from: root, into: "main" }));
    expect(outcomes(copy)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
    expect(outcomes(inChild)).toEqual([{ verdict: "merged", via: "merge", by: "user" }]);
  });

  test("a_critic_sessions_fix_is_by_critic", async () => {
    const { local, projectId } = await setUp();
    const { session: s, client: a } = await session(local, projectId);
    const root = s.rootBranch!;
    const child = ok<{ branch: { id: string } }>(await call(a, "branch.create", { projectId, from: root, name: "try-1" })).branch.id;
    const edited = await edit(a, projectId, child);
    const g = ok<{ mergeId: string }>(await call(a, "history.merge", { projectId, from: child, into: root }));
    // A critic session on a branch of the editor's root reverts the editor's merge there.
    const critic = await session(local, projectId, { role: "critic", base: root });
    ok(await call(critic.client, "history.revert", { projectId, mergeId: g.mergeId, branch: critic.session.rootBranch! }));
    expect(outcomes(edited)).toEqual([{ verdict: "reverted", via: "critic", by: "critic" }]);
  });

  test("a_deleted_sessions_unmerged_are_abandoned; a merge after the delete is still judged", async () => {
    const { local, projectId } = await setUp();
    const { session: s, client: a } = await session(local, projectId);
    const root = s.rootBranch!;
    const t = await edit(a, projectId, root);
    await a.close();
    server.core.outcomes.observeSessionDeleted(server.core.store.get(s.id)!);
    await server.core.store.delete(s.id);
    expect(server.core.store.get(s.id)).toBeUndefined();
    expect(outcomes(t)).toEqual([{ verdict: "abandoned", via: "archive", by: "user" }]);
    // The row outlived the session, with its root: a late merge into main counts.
    ok(await call(local, "history.merge", { projectId, from: root, into: "main" }));
    expect(outcomes(t)).toEqual([
      { verdict: "abandoned", via: "archive", by: "user" },
      { verdict: "merged", via: "merge", by: "user" },
    ]);
  });

  test("idempotent, dry runs and other tags; emitted once 5E's kind exists, with openS from the clock", async () => {
    const { local, projectId } = await setUp();
    const { session: s, client: a } = await session(local, projectId);
    const root = s.rootBranch!;
    const t = await edit(a, projectId, root);
    server.time.advance(7_500);
    // A dry run records nothing.
    ok(await call(local, "history.merge", { projectId, from: root, into: "main", dryRun: true }));
    expect(outcomes(t)).toEqual([]);
    const result = ok(await call(local, "history.merge", { projectId, from: root, into: "main" }));
    expect(committedOf(result)).toHaveLength(1);
    // A second tracker over the same store with 5E's kind registered: the outcome is in the
    // ledger already, so nothing is emitted again.
    const sink = spySink(true);
    const tracker = new OutcomeTracker({ store: server.core.store, lineage: server.core.lineage, verdicts: sink, clock: server.time.now, warn: () => {} });
    const projects = {
      isOpen: () => true,
      dirOf: () => undefined,
      resolveBranch: (_: string, r: string) => Effect.succeed(r),
      branches: () => Effect.succeed([{ id: "main", parent: null, createdBy: "x" }, { id: root, parent: "main", createdBy: "x" }]),
    };
    const run = (tag: string, payload: unknown, res: unknown, p: Principal = LOCAL) => Effect.runPromise(tracker.observeHistory(tag, payload, res, p, projects));
    await run("history.merge", { projectId }, result);
    expect(sink.got).toEqual([]);
    expect(outcomes(t)).toHaveLength(1);
    // 5E's own merge is not a history RPC: the tracker records nothing for it.
    await run("review.merge", { projectId }, result);
    expect(outcomes(t)).toHaveLength(1);
    // A new outcome on that tracker is emitted, once, and marked.
    const archived = ok(await call(local, "branch.set", { projectId, branch: root, archived: true, dryRun: true }));
    await run("branch.set", { projectId, dryRun: true }, archived);
    expect(sink.got).toEqual([]);
    const persons = await edit(local, projectId, "main", 4);
    expect(outcomes(persons)).toEqual([]); // a person's commit is not a session's
    const later = await edit(a, projectId, root, 6);
    const u = ok(await call(a, "history.undo", { projectId, branch: root }));
    // The server's own tracker stored it without emitting; this one sees a repeat.
    expect(outcomes(later)).toEqual([{ verdict: "undone", via: "undo", by: "agent" }]);
    await run("history.undo", { projectId }, u, { kind: "agent", id: s.id, scopes: [], projects: [projectId], branches: [root], human: false } as Principal);
    expect(sink.got).toEqual([]);
    // A fresh outcome through this tracker: emitted with 5E's fields.
    await run("history.revert", { projectId }, { committed: [], undone: [], dryRun: false, origins: [t] });
    expect(sink.got).toEqual([
      { v: { verdict: "reverted", txIds: [t], by: "user", via: "revert", itemKind: "none", sessionId: s.id, openS: 7 }, projectId, version: 1 },
    ]);
    expect(server.core.store.outcomes.of(t).find((o) => o.via === "revert")?.emitted).toBe(true);
    await run("history.revert", { projectId }, { committed: [], undone: [], dryRun: false, origins: [t] });
    expect(sink.got).toHaveLength(1);
  });
});

describe("verdict sink", () => {
  test("journalVerdicts: the kind's version is looked up each time; off records nothing", () => {
    const recorded: unknown[][] = [];
    let enabled = false;
    let registered: number | null = null;
    const journal = {
      get enabled() {
        return enabled;
      },
      record: (...args: unknown[]) => void recorded.push(args),
    } as unknown as Parameters<typeof journalVerdicts>[0];
    const sink = journalVerdicts(journal, () => registered);
    expect(sink.version()).toBeNull();
    registered = 1;
    expect(sink.version()).toBe(1);
    const v: OutcomeVerdict = { verdict: "merged", txIds: ["T1"], by: "user", via: "merge", itemKind: "none", sessionId: "S1", openS: 3 };
    expect(sink.emit(v, "P1", 1)).toBe(false);
    expect(recorded).toEqual([]);
    enabled = true;
    expect(sink.emit(v, "P1", 1)).toBe(true);
    expect(recorded).toEqual([["review.verdict", 1, JSON.stringify({ project: "P1", txId: "T1", actor: { kind: "agent", session: "S1" } }), JSON.stringify(v), "agent"]]);
  });

  test("kindVersion: unknown and foreign-source kinds have none; review.verdict is v1 for agent", () => {
    expect(kindVersion("review.verdict", "agent")).toBe(1);
    expect(kindVersion("review.verdict", "ui")).toBeNull();
    expect(kindVersion("no.such", "agent")).toBeNull();
    expect(kindVersion("agent.session", "agent")).toBe(1);
    expect(kindVersion("agent.session", "fin")).toBeNull();
    expect(kindVersion("agent.session", "nonsense")).toBeNull();
  });

  test("a sink that throws is reported, never thrown", async () => {
    await server.start();
    const warned: string[] = [];
    const tracker = new OutcomeTracker({
      store: server.core.store,
      lineage: server.core.lineage,
      verdicts: { version: () => 1, emit: () => { throw new Error("refused"); } },
      clock: server.time.now,
      warn: (m) => void warned.push(m),
    });
    server.core.store.recordTx({ txId: "T-orphan", sessionId: "S-gone", callId: null, attribution: "none", at: 0 });
    tracker.observeSessionDeleted({ id: "S-gone", projectId: "P" } as Session);
    expect(server.core.store.outcomes.of("T-orphan")).toMatchObject([{ verdict: "abandoned", emitted: false }]);
    expect(warned.join("\n")).toContain("refused");
  });
});
