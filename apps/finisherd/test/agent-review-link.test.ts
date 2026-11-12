// Review requests from sessions and critics on child branches (sub-project 7B Task 11, plan
// Decisions 25 and 26; spec §5.4, §6.2, §9.1, §12.5). Against a real server and `control.db`,
// sessions on the scripted `fake` harness (a test-hooks build run with `FIN_TEST_HOOKS=1`): a
// `draft` session's `review.request` opens a 5E item for its branch, is recorded on the session
// and emits `branchReady`; a session ending with unmerged commits and no review emits it once;
// a critic on a reviewed branch works on a child branch of it; the outcome tracker emits 5E's
// `review.verdict` v1 and counts a review's merge once; the runtime's archive of a branch tells
// 5E's conflict items.
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { FinRpcs, type FinRpcTag, type Principal, type ReviewItem, type ReviewView, type Session } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { kindVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { credentialPath, MODE_SCOPES, NEVER_SESSION_SCOPES, registryBranchOps } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import { type BranchReady, withReviewLink } from "../src/agent/review-link.ts";
import { settingsPath } from "../src/config.ts";
import { controlDbPath } from "../src/control/db.ts";
import { toFinError } from "../src/errors.ts";
import { entries } from "./support/agent-journal.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let server: AgentServer;
const hooks = process.env.FIN_TEST_HOOKS;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
/** A run that never ends by itself (the session stays `running`). */
const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-review-link-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

/** A server (diagnostics on: the journal records `review.verdict`) and a project with an asset on `main`. */
async function boot() {
  writeFileSync(settingsPath(serverData), JSON.stringify({ privacy: { diagnostics: "on" } }));
  server = new AgentServer(serverData, { timers: false, fakeScripts: [WORKING] });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const ready: BranchReady[] = [];
  server.core.reviewLink.onBranchReady((ev) => void ready.push(ev));
  return { local, projectId, ready };
}

const get = (sessionId: string) => server.core.store.get(sessionId)!;

async function until<A>(f: () => A | undefined | false, what: string, ms = 8000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** A session a person started in its first run; its credential's client. */
async function start(local: NdjsonClient, projectId: string, more: Record<string, unknown> = {}) {
  const { session } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go", ...more }));
  await until(() => get(session.id).status === "running", `${session.id} running`);
  return { session, client: await server.agent(credentialPath(server.core.store.root, session.id)) };
}

const branches = async (c: NdjsonClient, projectId: string) => ok<{ branches: BranchView[] }>(await c.call("branch.list", { projectId, all: true })).branches;
const branch = async (c: NdjsonClient, projectId: string, idOf: string) => (await branches(c, projectId)).find((b) => b.id === idOf)!;

/** A commit on `on` as `c`; its txId. */
async function edit(c: NdjsonClient, projectId: string, on: string, at = 0): Promise<string> {
  const txId = id();
  ok(await commit(c, projectId, { ...tx(txId, 1, [insert(at)]), branch: on }));
  return txId;
}

const request = (c: NdjsonClient, projectId: string, source: string) => c.call("review.request", { projectId, source, title: "Ready" });
const outcomes = (txId: string) => server.core.store.outcomes.of(txId).map((o) => ({ verdict: o.verdict, via: o.via, by: o.by, emitted: o.emitted }));

/** The `review.verdict` entries journalled so far (`diagnostics.status` flushes the journal). */
async function verdicts(local: NdjsonClient): Promise<Array<Record<string, any>>> { // eslint-disable-line @typescript-eslint/no-explicit-any
  ok(await local.call("diagnostics.status", {}));
  return entries(serverData, "events").filter((e) => e.kind === "review.verdict").map((e) => e.payload);
}

describe("review.write in the session modes", () => {
  test("draft and auto hold review.write; no mode holds review.decide or branch.protected", () => {
    expect(MODE_SCOPES.draft).toContain("review.write");
    expect(MODE_SCOPES.auto).toContain("review.write");
    expect(MODE_SCOPES.plan).not.toContain("review.write");
    for (const [mode, scopes] of Object.entries(MODE_SCOPES)) {
      expect(scopes, mode).not.toContain("review.decide");
      expect(scopes, mode).not.toContain("branch.protected");
    }
    expect(NEVER_SESSION_SCOPES.has("review.decide")).toBe(true);
    expect(NEVER_SESSION_SCOPES.has("branch.protected")).toBe(true);
  });
});

describe("a session's review request", () => {
  test("from a draft session: a 5E item for its branch, recorded on the session, branchReady with the head seq, a notice in its run", async () => {
    const { local, projectId, ready } = await boot();
    const { session: s, client: a } = await start(local, projectId, { mode: "draft" });
    const root = s.rootBranch!;
    await edit(a, projectId, root);
    const { item, existing } = ok<{ item: ReviewItem; existing: boolean }>(await request(a, projectId, root));
    expect(existing).toBe(false);
    expect(item).toMatchObject({ kind: "branchReview", state: "open", source: root, target: "main", requestedBy: { principal: `agent:${s.id}` } });
    await server.core.reviewLink.settled();
    const head = (await branch(local, projectId, root)).head;
    expect(ready).toEqual([{ sessionId: s.id, projectId, branch: root, headSeq: head, reviewId: item.id }]);
    expect(server.core.store.reviews.of(s.id)).toEqual([item.id]);
    // The run goes on (Decision 25 as built): a `notice` names the item.
    const notice = await until(() => server.core.store.nodes(s.id).find((x) => x.kind === "notice" && (x.payload as { review?: unknown }).review !== undefined), "the review notice");
    expect(notice.payload).toEqual({ review: { itemId: item.id, branch: root, headSeq: head } });
    expect(get(s.id).status).toBe("running");
    // Its own item: comment, refresh and read; deciding stays a person's.
    ok(await a.call("review.comment", { itemId: item.id, body: "look at the cut" }));
    ok(await a.call("review.refresh", { itemId: item.id }));
    expect(ok<ReviewView>(await a.call("review.get", { itemId: item.id })).item.id).toBe(item.id);
    expect(code(await a.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [] }))).toBe("SCOPE_DENIED");
    // Asking again answers the open item, is recorded once, and is ready again.
    await edit(a, projectId, root, 1);
    expect(ok<{ item: ReviewItem; existing: boolean }>(await request(a, projectId, root))).toMatchObject({ existing: true, item: { id: item.id } });
    await server.core.reviewLink.settled();
    expect(server.core.store.reviews.of(s.id)).toEqual([item.id]);
    expect(ready.map((r) => r.headSeq)).toEqual([head, (await branch(local, projectId, root)).head]);
    // Its end emits no second branchReady: it has a review.
    ok(await local.call("agent.session.end", { sessionId: s.id }));
    await server.core.reviewLink.settled();
    expect(ready).toHaveLength(2);
  });

  test("a plan session holds no review.write; a request for a branch outside the session's subtree links nothing", async () => {
    const { local, projectId, ready } = await boot();
    const planned = await start(local, projectId, { mode: "plan" });
    expect(code(await request(planned.client, projectId, "main"))).toBe("SCOPE_DENIED");
    const other = ok<{ branch: BranchView }>(await local.call("branch.create", { projectId, txId: id(), author: "ann", name: "feature", from: "main" })).branch.id;
    await edit(local, projectId, other);
    const { client: a, session: s } = await start(local, projectId, { mode: "draft" });
    ok(await request(a, projectId, other));
    await server.core.reviewLink.settled();
    expect(ready).toEqual([]);
    expect(server.core.store.reviews.of(s.id)).toEqual([]);
  });
});

describe("a session's end", () => {
  test("with unmerged commits and no review: branchReady once, without a review id", async () => {
    const { local, projectId, ready } = await boot();
    const { session: s, client: a } = await start(local, projectId);
    const root = s.rootBranch!;
    await edit(a, projectId, root);
    await edit(a, projectId, root, 1);
    const head = (await branch(local, projectId, root)).head;
    ok(await local.call("agent.session.end", { sessionId: s.id }));
    await server.core.reviewLink.settled();
    expect(ready).toEqual([{ sessionId: s.id, projectId, branch: root, headSeq: head }]);
    // Ended is final: a second end is refused and emits nothing.
    expect(code(await local.call("agent.session.end", { sessionId: s.id }))).toBe("SESSION_ENDED");
    await server.core.reviewLink.settled();
    expect(ready).toHaveLength(1);
  });

  test("without commits, with its commits merged already, or deleted: none", async () => {
    const { local, projectId, ready } = await boot();
    const idle = await start(local, projectId);
    ok(await local.call("agent.session.end", { sessionId: idle.session.id }));
    const merged = await start(local, projectId);
    await edit(merged.client, projectId, merged.session.rootBranch!);
    ok(await local.call("history.merge", { projectId, txId: id(), author: "ann", from: merged.session.rootBranch!, into: "main" }));
    ok(await local.call("agent.session.end", { sessionId: merged.session.id }));
    const deleted = await start(local, projectId);
    await edit(deleted.client, projectId, deleted.session.rootBranch!);
    ok(await confirmed(await server.console(), "agent.session.delete", { sessionId: deleted.session.id }));
    await server.core.reviewLink.settled();
    expect(ready).toEqual([]);
  });
});

describe("critics on child branches", () => {
  test("a critic on a reviewed branch works on <reviewed>.critic-<n>: it cannot write the reviewed branch, and its base seq is the head it started from", async () => {
    const { local, projectId } = await boot();
    const { session: s, client: a } = await start(local, projectId);
    const root = s.rootBranch!;
    await edit(a, projectId, root);
    ok(await request(a, projectId, root));
    const reviewed = await branch(local, projectId, root);
    const critic = await start(local, projectId, { role: "critic", mode: "draft", base: root });
    const made = await branch(local, projectId, critic.session.rootBranch!);
    expect(made).toMatchObject({ name: `${reviewed.name}.critic-1`, parent: root });
    expect(critic.session.base).toEqual({ branch: root, seq: reviewed.head });
    expect(get(critic.session.id).base.seq).toBe(reviewed.head);
    const refused = await commit(critic.client, projectId, { ...tx(id(), 1, [insert(3)]), branch: root });
    expect(code(refused)).toBe("BRANCH_NOT_ALLOWED");
    await edit(critic.client, projectId, made.id, 3);
    // While the critic lives, its branch is out of the reviewed session's reach.
    expect(code(await commit(a, projectId, { ...tx(id(), 1, [insert(4)]), branch: made.id }))).toBe("BRANCH_NOT_ALLOWED");
  });

  test("a critic whose base is under no review gets an ordinary root", async () => {
    const { local, projectId } = await boot();
    const critic = await start(local, projectId, { role: "critic", mode: "draft", base: "main" });
    const made = await branch(local, projectId, critic.session.rootBranch!);
    expect(made.name).toBe("agent.critic-1");
  });

  test("a critic's revert of an agent transaction on its child branch is by: critic, via: critic, emitted as review.verdict v1", async () => {
    const { local, projectId } = await boot();
    expect(kindVersion("review.verdict", "agent")).toBe(1);
    const { session: s, client: a } = await start(local, projectId);
    const root = s.rootBranch!;
    const tryBranch = ok<{ branch: { id: string } }>(await a.call("branch.create", { projectId, txId: id(), author: "x", from: root, name: "try-1" })).branch.id;
    const edited = await edit(a, projectId, tryBranch);
    const g = ok<{ mergeId: string }>(await a.call("history.merge", { projectId, txId: id(), author: "x", from: tryBranch, into: root }));
    ok(await request(a, projectId, root));
    const critic = await start(local, projectId, { role: "critic", mode: "draft", base: root });
    ok(await critic.client.call("history.revert", { projectId, txId: id(), author: "x", mergeId: g.mergeId, branch: critic.session.rootBranch! }));
    expect(outcomes(edited)).toEqual([{ verdict: "reverted", via: "critic", by: "critic", emitted: true }]);
    const seen = (await verdicts(local)).filter((p) => p.itemKind === "none" && p.txIds?.includes(edited));
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ verdict: "reverted", by: "critic", via: "critic", sessionId: s.id });
  });
});

describe("a review's merge in the outcome tracker", () => {
  test("is counted once: 5E's verdict, the ledger's merged (not emitted), and no abandoned at the delete", async () => {
    const { local, projectId } = await boot();
    const { session: s, client: a } = await start(local, projectId);
    const root = s.rootBranch!;
    const t1 = await edit(a, projectId, root);
    const { item } = ok<{ item: ReviewItem }>(await request(a, projectId, root));
    const decided = ok<ReviewView>(await local.call("review.decide", { itemId: item.id, expect: item.snapshot, decisions: [{ txId: t1, decision: "accept" }] }));
    ok(await local.call("review.merge", { itemId: item.id, expect: decided.item.snapshot, idemKey: "m1" }));
    expect(outcomes(t1)).toEqual([{ verdict: "merged", via: "merge", by: "user", emitted: false }]);
    ok(await confirmed(await server.console(), "agent.session.delete", { sessionId: s.id }));
    expect(outcomes(t1)).toEqual([{ verdict: "merged", via: "merge", by: "user", emitted: false }]);
    const merged = (await verdicts(local)).filter((p) => p.verdict === "merged" && p.txIds?.includes(t1));
    expect(merged).toHaveLength(1);
    expect(merged[0]).toMatchObject({ itemKind: "branchReview", by: "user" });
    expect((await verdicts(local)).filter((p) => p.txIds?.includes(t1) && p.verdict === "abandoned")).toEqual([]);
  });
});

describe("branchReady is reachable only from review.request and a session's end", () => {
  test("the chain's after stage links review.request and no other RPC", async () => {
    const { local, projectId, ready } = await boot();
    const { session: s } = await start(local, projectId);
    const projects = {
      branches: (p: string) => Effect.promise(async () => await branches(local, p)),
    } as unknown as ChainProjects;
    const after = withReviewLink({}, server.core.reviewLink, projects).after!;
    const principal: Principal = { kind: "agent", id: s.id, scopes: [...MODE_SCOPES.draft], projects: [projectId], branches: [s.rootBranch!], human: false };
    const result = { item: { id: "item-1", kind: "branchReview", projectId, source: s.rootBranch! }, existing: false };
    const tags = [...FinRpcs.requests.keys()] as FinRpcTag[];
    expect(tags.length).toBeGreaterThan(50);
    for (const tag of tags) {
      if (tag === "review.request") continue;
      await Effect.runPromise(after(tag, principal, { projectId }, result, undefined));
    }
    await server.core.reviewLink.settled();
    expect(ready).toEqual([]);
    expect(server.core.store.reviews.of(s.id)).toEqual([]);
    await Effect.runPromise(after("review.request", principal, { projectId }, result, undefined));
    await server.core.reviewLink.settled();
    expect(ready.map((r) => r.reviewId)).toEqual(["item-1"]);
  });

  test("only the chain's after stage and the runtime's end reach the link's emitters", () => {
    const src = join(import.meta.dirname, "..", "src");
    const files: string[] = [];
    const walk = (d: string) => {
      for (const f of readdirSync(d)) {
        const p = join(d, f);
        if (statSync(p).isDirectory()) walk(p);
        else if (p.endsWith(".ts")) files.push(p);
      }
    };
    walk(src);
    const sites = (re: RegExp) => files.filter((f) => re.test(readFileSync(f, "utf8"))).map((f) => relative(src, f).replaceAll("\\", "/")).sort();
    expect(sites(/\.requested\(/)).toEqual(["agent/review-link.ts"]);
    expect(sites(/\.sessionEnded\(/)).toEqual(["agent/runtime.ts"]);
    // The runtime's notice and the `auto` merge trigger (7B Task 12, wired in `main.ts`).
    expect(sites(/\.onBranchReady\(/)).toEqual(["agent/runtime.ts", "main.ts"]);
    expect(sites(/withReviewLink\(/)).toEqual(["agent/review-link.ts", "handlers.ts"]);
  });
});

describe("the runtime's archive of a branch (5E residual)", () => {
  test("supersedes the branch's mergeConflict items, as branch.set does", async () => {
    const { local, projectId } = await boot();
    ok(await commit(local, projectId, tx(id(), 1, [insert(0, 5)])));
    const feature = ok<{ branch: BranchView }>(await local.call("branch.create", { projectId, txId: id(), author: "ann", name: "feature", from: "main" })).branch.id;
    const del = { op: "clip.delete", clip: "c1" };
    ok(await commit(local, projectId, { ...tx(id(), (await branch(local, projectId, feature)).head, [del]), branch: feature }));
    ok(await commit(local, projectId, tx(id(), (await branch(local, projectId, "main")).head, [del])));
    expect(code(await local.call("history.merge", { projectId, txId: id(), author: "ann", from: feature, into: "main" }))).toBe("CONFLICT");
    const items = async () => ok<{ items: ReviewItem[] }>(await local.call("review.list", { projectId, kind: "mergeConflict" })).items;
    expect((await items()).map((i) => i.state)).toEqual(["open"]);
    // The runtime's branch ops (a branch made for a session that failed to record), over RPC.
    const call = <A>(tag: string, payload: unknown) =>
      Effect.flatMap(Effect.promise(() => local.call(tag, payload)), (e: Exit) => (e.ok ? Effect.succeed(e.value as A) : Effect.fail(toFinError(Object.assign(new Error(e.error.message), { ...e.error, finNode: true })))));
    const registry = {
      branches: (p: string) => call<{ branches: BranchView[] }>("branch.list", { projectId: p, all: true }),
      resolveBranch: (_p: string, ref: string) => Effect.succeed(ref),
      history: (_p: string, op: "branchCreate" | "branchSet", payload: unknown) => call(op === "branchSet" ? "branch.set" : "branch.create", payload),
    };
    const ops = registryBranchOps(registry, { actor: { kind: "local" } } as never, (p, b) => server.core.reviewLink.archived(p, b, "local:local"));
    await Effect.runPromise(ops.archiveBranch!(projectId, { txId: id(), author: "ann", branch: feature }));
    expect((await items()).map((i) => i.state)).toEqual(["superseded"]);
  });

  test("a start whose record fails archives the branch made for it through the review link", async () => {
    const { local, projectId } = await boot();
    const seen: string[] = [];
    const link = server.core.reviewLink;
    const archived = link.archived.bind(link);
    link.archived = (p, b, by) => {
      seen.push(b);
      return archived(p, b, by);
    };
    const db = new DatabaseSync(controlDbPath(serverData));
    db.exec("CREATE TRIGGER no_state BEFORE INSERT ON session_state BEGIN SELECT RAISE(ABORT, 'refused'); END");
    try {
      expect((await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go" })).ok).toBe(false);
    } finally {
      db.exec("DROP TRIGGER no_state");
      db.close();
    }
    const made = (await branches(local, projectId)).filter((b) => b.name.startsWith("agent."));
    expect(made.map((b) => b.state)).toEqual(["archived"]);
    expect(seen).toEqual([made[0]!.id]);
  });
});
