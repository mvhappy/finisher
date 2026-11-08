// 7A Task 7: branch reach and path reach of an agent session (Decisions 14d, 16, 17; spec §5.1,
// §5.2, §5.6): its writes stay in its root branch's subtree, in the chain and in the store's
// guard; it reaches its project folder and the libraries the project references; every write
// carries the server's author; every commit, undo and merge is recorded under the session.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Principal, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BranchLineage, PathReach, sessionAuthor } from "../src/agent/reach.ts";
import { authorize } from "../src/chain/authorize.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { committedOf } from "../src/history.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-reach-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

const SESSION = { id: "01S0000000000000000000000S", projectId: "01P", role: "editor", rootBranch: "br1" } as unknown as Session;
const AGENT: Principal = { kind: "agent", id: SESSION.id, scopes: ["project.read", "project.write"], projects: ["01P"], branches: ["br1"], human: false };

function fakeProjects(branches: Array<{ id: string; parent: string | null }>, dirs: Record<string, string> = {}) {
  let reads = 0;
  const projects: ChainProjects = {
    isOpen: () => true,
    dirOf: (id) => dirs[id],
    resolveBranch: (_, ref) => Effect.succeed(ref),
    branches: () =>
      Effect.sync(() => {
        reads++;
        return branches.map((b) => ({ ...b, createdBy: "x" }));
      }),
  };
  return { projects, reads: () => reads, branches };
}

describe("units", () => {
  test("BranchLineage: the root's subtree; a branch it does not know reads the index again, once", async () => {
    const f = fakeProjects([
      { id: "main", parent: null },
      { id: "br1", parent: "main" },
      { id: "br2", parent: "br1" },
      { id: "br3", parent: "main" },
    ]);
    const lineage = new BranchLineage();
    const run = <A>(e: Effect.Effect<A, unknown>) => Effect.runPromise(e as Effect.Effect<A>);
    expect([...(await run(lineage.subtree(f.projects, "p", "br1")))].sort()).toEqual(["br1", "br2"]);
    expect(f.reads()).toBe(1);
    expect((await run(lineage.subtree(f.projects, "p", "br1", ["br2"]))).has("br2")).toBe(true);
    expect(f.reads()).toBe(1);
    // A branch made since: read again, then found.
    f.branches.push({ id: "br4", parent: "br2" });
    expect((await run(lineage.subtree(f.projects, "p", "br1", ["br4"]))).has("br4")).toBe(true);
    expect(f.reads()).toBe(2);
    // One outside stays outside (one more read, no loop).
    expect((await run(lineage.subtree(f.projects, "p", "br1", ["br3"]))).has("br3")).toBe(false);
    expect(f.reads()).toBe(3);
    // An unknown root reaches nothing.
    expect((await run(lineage.subtree(f.projects, "p", "br9"))).size).toBe(0);
  });

  test("path_reach (units): the project's libraries are roots, read-only reach; another library is not", async () => {
    const projectDir = join(tmp, "P");
    const lib = join(tmp, "lib-a");
    const other = join(tmp, "lib-b");
    for (const d of [projectDir, lib, other]) mkdirSync(d, { recursive: true });
    const f = fakeProjects([], { "01P": projectDir });
    const sessions = { get: (id: string) => (id === SESSION.id ? SESSION : undefined), recordTx: () => undefined };
    const paths = new PathReach(sessions, f.projects, (id) => Effect.succeed(id === "01P" ? [lib] : []));
    expect(await paths.roots(AGENT, "01P")).toEqual([projectDir, join(projectDir, "workspace", SESSION.id), lib]);
    expect(await paths.roots({ ...AGENT, id: "01OTHER0000000000000000000" }, "01P")).toEqual([]);
    const probe = (path: string) =>
      Effect.runPromise(
        Effect.flip(
          Effect.as(
            authorize("asset.probe", AGENT, { projectId: "01P", path }, { payload: {}, projects: ["01P"], paths: [path], facts: {}, pathsIn: "01P" }, f.projects, {
              extraRoots: (p, only) => paths.extra(p, only),
            }),
            "ok",
          ),
        ).pipe(Effect.map((e) => (e as { code: string }).code), Effect.orElseSucceed(() => "ok")),
      );
    expect(await probe(join(lib, "a.mov"))).toBe("ok");
    expect(await probe(join(projectDir, "workspace", SESSION.id, "a.mov"))).toBe("ok");
    expect(await probe(join(other, "a.mov"))).toBe("PATH_NOT_ALLOWED");
    expect(await probe(join(tmp, "elsewhere.mov"))).toBe("PATH_NOT_ALLOWED");
  });

  test("sessionAuthor", () => {
    expect(sessionAuthor(SESSION)).toBe(`agent:editor:${SESSION.id}`);
  });
});

// --- On a server --------------------------------------------------------------------------

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

async function setUp() {
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const one = await open(server.core, local, projectId);
  const two = await open(server.core, local, projectId);
  const a = await server.agent(one.credentialPath);
  return { local, projectId, s1: one.session, s2: two.session, a };
}

const write = (c: NdjsonClient, tag: string, payload: Record<string, unknown>, headers: ReadonlyArray<[string, string]> = []) =>
  c.call(tag, { txId: id(), author: "someone-else", ...payload }, headers);

describe("reach on a server", () => {
  test("reach_matrix: its root and what it made beneath, yes; main and a sibling session's root, no; reads, yes", async () => {
    const { local, projectId, s1, s2, a } = await setUp();
    const root = s1.rootBranch!;
    const sibling = s2.rootBranch!;
    // A branch beneath its root, made by the session itself.
    const child = ok<{ branch: { id: string; createdBy: string } }>(await write(a, "branch.create", { projectId, from: root, name: "try-1" })).branch;
    expect(child.createdBy).toBe(sessionAuthor(s1));
    expect(code(await write(a, "branch.create", { projectId, from: "main", name: "escape" }))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await write(a, "branch.create", { projectId, from: sibling, name: "escape" }))).toBe("BRANCH_NOT_ALLOWED");
    const at = async (branch: string) => commit(a, projectId, { ...tx(id(), 1, [insert(0)]), branch });
    expect(code(await at(root))).toBeUndefined();
    expect(code(await at(child.id))).toBeUndefined();
    expect(code(await at("main"))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await at(sibling))).toBe("BRANCH_NOT_ALLOWED");
    // History calls, by their target.
    for (const [tag, payload, want] of [
      ["history.checkpoint", { branch: root, name: "c1" }, undefined],
      ["history.checkpoint", { branch: "main", name: "c1" }, "BRANCH_NOT_ALLOWED"],
      ["history.undo", { branch: child.id }, undefined],
      ["history.undo", { branch: "main" }, "BRANCH_NOT_ALLOWED"],
      ["history.redo", { branch: sibling }, "BRANCH_NOT_ALLOWED"],
      ["history.merge", { from: "main", into: root }, undefined],
      ["history.merge", { from: root, into: "main" }, "BRANCH_NOT_ALLOWED"],
      ["history.merge", { from: root, into: sibling }, "BRANCH_NOT_ALLOWED"],
      ["branch.set", { branch: child.id, name: "try-renamed" }, undefined],
      ["branch.set", { branch: sibling, name: "taken" }, "BRANCH_NOT_ALLOWED"],
      ["branch.set", { branch: "main", archived: true }, "BRANCH_NOT_ALLOWED"],
    ] as const) {
      expect(code(await write(a, tag, { projectId, ...payload })), `${tag} ${JSON.stringify(payload)}`).toBe(want);
    }
    // Reads of any branch.
    for (const branch of ["main", sibling, root]) expect(code(await a.call("snapshot.get", { projectId, branch })), branch).toBeUndefined();
    // A person's writes are not limited by any of this.
    expect(code(await commit(local, projectId, { ...tx(id(), 1, [insert(0)]), branch: sibling }))).toBeUndefined();
  });

  test("path_reach: another project's folder is PROJECT_NOT_FOUND, any other path PATH_NOT_ALLOWED", async () => {
    const { local, projectId, s1, a } = await setUp();
    const q = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "Q"), name: "Q", rate: "30" })).projectId;
    expect(code(await a.call("asset.probe", { projectId: q, path: join(tmp, "Q", "a.mov") }))).toBe("PROJECT_NOT_FOUND");
    expect(code(await a.call("project.open", { dir: join(tmp, "Q") }))).toBe("PROJECT_NOT_FOUND");
    expect(code(await a.call("asset.probe", { projectId, path: join(tmp, "Q", "a.mov") }))).toBe("PATH_NOT_ALLOWED");
    expect(code(await a.call("asset.probe", { projectId, path: join(tmp, "outside.mov") }))).toBe("PATH_NOT_ALLOWED");
    // Inside its own workspace the probe passes reach (and fails on the missing file).
    const inWorkspace = code(await a.call("asset.probe", { projectId, path: join(tmp, "P", "workspace", s1.id, "a.mov") }));
    expect(["PATH_NOT_ALLOWED", "PROJECT_NOT_FOUND", "SCOPE_DENIED"]).not.toContain(inWorkspace);
  });

  test("author_is_set_by_server: what it sends, and fin-author, are ignored", async () => {
    const { projectId, s1, a } = await setUp();
    const txId = id();
    ok(await commit(a, projectId, { ...tx(txId, 1, [insert(0)]), branch: s1.rootBranch!, author: "someone-else" }));
    const got = ok<{ tx: { author: string } }>(await a.call("tx.get", { projectId, txId }, [["fin-author", "someone-else"]]));
    expect(got.tx.author).toBe(`agent:editor:${s1.id}`);
  });

  test("session_txs_written_after_every_agent_write: with and without a call, undo, merge into its root; never another's", async () => {
    const { local, projectId, s1, s2, a } = await setUp();
    const root = s1.rootBranch!;
    const call = "01CALL00000000000000000000";
    const c1 = id();
    ok(await a.call("tx.commit", { projectId, tx: { ...tx(c1, 1, [insert(0)]), branch: root }, generation: await generation(a, projectId, root) }, [["fin-actor-call", call]]));
    const c2 = id();
    ok(await commit(a, projectId, { ...tx(c2, 2, [insert(0)]), branch: root }));
    // A dry run lands nothing and records nothing.
    ok(await write(a, "history.undo", { projectId, branch: root, dryRun: true }));
    const undone = committedOf(ok(await write(a, "history.undo", { projectId, branch: root }))).map((c) => c.tx.txId);
    // Something on main to merge into its root.
    ok(await commit(local, projectId, tx(id(), 1, [insert(5)])));
    const merged = committedOf(ok(await write(a, "history.merge", { projectId, from: "main", into: root }))).map((c) => c.tx.txId);
    expect(undone.length).toBeGreaterThan(0);
    expect(merged.length).toBeGreaterThan(0);
    const rows = server.core.store.txsOfSession(s1.id);
    expect(rows.map((r) => r.txId).sort()).toEqual([c1, c2, ...undone, ...merged].sort());
    expect(rows.find((r) => r.txId === c1)).toMatchObject({ callId: call, attribution: "exact" });
    expect(rows.find((r) => r.txId === c2)).toMatchObject({ callId: null, attribution: "none" });
    expect(server.core.store.txsOfSession(s2.id)).toEqual([]);
    // A person's commits are not a session's.
    expect([...server.core.store.sessionOfTx([c1, c2]).keys()].sort()).toEqual([c1, c2].sort());
  });

  test("agent.self and agent.self.try: the session's own default and tries, inside its subtree only", async () => {
    const { local, projectId, s1, s2, a } = await setUp();
    const root = s1.rootBranch!;
    const child = ok<{ branch: { id: string } }>(await write(a, "branch.create", { projectId, from: root, name: "try-1" })).branch.id;
    type Self = { sessionId: string; projectId: string; rootBranch?: string; defaultBranch: string; tries: Array<{ branch: string; previousDefault: string }> };
    expect(ok<Self>(await a.call("agent.self", {}))).toEqual({ sessionId: s1.id, projectId, rootBranch: root, defaultBranch: root, tries: [] });
    expect(ok<Self>(await a.call("agent.self.try", { action: "record", branch: "try-1" })).tries).toEqual([{ branch: child, previousDefault: root }]);
    expect(ok<Self>(await a.call("agent.self.try", { action: "setDefault", branch: child })).defaultBranch).toBe(child);
    expect(ok<{ defaultBranch?: string }>(await a.call("auth.whoami", {})).defaultBranch).toBe(child);
    for (const outside of ["main", s2.rootBranch!]) {
      expect(code(await a.call("agent.self.try", { action: "record", branch: outside })), outside).toBe("BRANCH_NOT_ALLOWED");
    }
    expect(ok<Self>(await a.call("agent.self.try", { action: "clear", branch: child }))).toMatchObject({ defaultBranch: root, tries: [] });
    // Another session's state is its own; a person has none.
    expect(server.core.store.state(s2.id).defaultBranch).toBe(s2.rootBranch);
    expect(code(await local.call("agent.self", {}))).toBe("SCOPE_DENIED");
  });
});

async function generation(c: NdjsonClient, projectId: string, branch: string) {
  return ok<{ generation: unknown }>(await c.call("snapshot.get", { projectId, branch })).generation;
}
