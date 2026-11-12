// The `auto` merge policy's session-side trigger (sub-project 7B Task 12, plan Decision 27; spec
// §6.5; control-plane spec §8.5). Against a real server and `control.db`, sessions on the
// scripted `fake` harness: on `branchReady` of an `auto` session whose role the project's policy
// admits, with no open approval, within budget, and a critic session outside the author's tree
// that started from the judged head, the trigger hands the item to 5E's `tryAutoMerge`, which
// merges as the `policy` actor; every other case leaves the item open. Availability and the
// verdicts are injected through the server's options, never the environment; by default nothing
// is read and nothing merges. No RPC reaches the trigger but through `branchReady`.
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { type ReviewItem, type Session, type Snapshot, STREAM_RPCS } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";
import { type AutoMergeOutcome, type AutoMergeTriggerOptions } from "../src/agent/auto-merge.ts";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import type { BranchReady } from "../src/agent/review-link.ts";
import { settingsPath } from "../src/config.ts";
import { controlDbPath } from "../src/control/db.ts";
import { CAPABILITIES } from "../src/handlers.ts";
import { type JudgedVerdict, tryAutoMerge, type VerdictSource } from "../src/reviews/auto-merge.ts";
import { entries } from "./support/agent-journal.ts";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";
import { type FinRpcTag, SAMPLE_PAYLOADS } from "./support/payloads.ts";

// The spies: every trigger the server builds counts its calls; `tryAutoMerge` is counted too.
const triggered = vi.hoisted(() => [] as unknown[]);
vi.mock("../src/agent/auto-merge.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("../src/agent/auto-merge.ts")>();
  return {
    ...real,
    autoMergeTrigger: (deps: Parameters<typeof real.autoMergeTrigger>[0]) => {
      const trigger = real.autoMergeTrigger(deps);
      return (ev: BranchReady) => {
        triggered.push(ev);
        return trigger(ev);
      };
    },
  };
});
vi.mock("../src/reviews/auto-merge.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("../src/reviews/auto-merge.ts")>();
  return { ...real, tryAutoMerge: vi.fn(real.tryAutoMerge) };
});

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
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-auto-merge-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  triggered.length = 0;
  vi.mocked(tryAutoMerge).mockClear();
});

afterEach(async () => {
  await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

/** The test's verdict source: what `set` put there, counting every read. */
function verdictSource(): VerdictSource & { list: JudgedVerdict[]; reads: number } {
  const s = {
    list: [] as JudgedVerdict[],
    reads: 0,
    verdicts: (_itemId: string) => {
      s.reads++;
      return s.list;
    },
  };
  return s;
}

interface Booted {
  readonly local: NdjsonClient;
  readonly projectId: string;
  readonly ready: BranchReady[];
  readonly outcomes: AutoMergeOutcome[];
  readonly source: ReturnType<typeof verdictSource>;
}

/**
 * A server (diagnostics on: the journal records `review.verdict`) and a project with an asset
 * on `main`. `available` (default true) and the verdicts reach the trigger through the options.
 */
async function boot(o: { available?: boolean; verdicts?: VerdictSource | "none" } = {}): Promise<Booted> {
  writeFileSync(settingsPath(serverData), JSON.stringify({ privacy: { diagnostics: "on" } }));
  const source = verdictSource();
  const outcomes: AutoMergeOutcome[] = [];
  const autoMerge: AutoMergeTriggerOptions = {
    ...(o.available === undefined ? { available: true } : o.available ? { available: true } : {}),
    ...(o.verdicts === "none" ? {} : { verdicts: o.verdicts ?? source }),
    observe: (_ev, out) => void outcomes.push(out),
  };
  server = new AgentServer(serverData, { timers: false, fakeScripts: [WORKING], autoMerge });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const ready: BranchReady[] = [];
  server.core.reviewLink.onBranchReady((ev) => void ready.push(ev));
  return { local, projectId, ready, outcomes, source };
}

/** The project's policy, written as the owner would once sub-project 10 allows it (`review.policy` refuses it today). */
function policy(projectId: string, p: { enabled?: boolean; roles?: string[]; requireReview?: boolean } = {}): void {
  const db = new DatabaseSync(controlDbPath(serverData));
  try {
    const autoMerge = { enabled: p.enabled ?? true, enabledBy: "console:console", ...(p.roles === undefined ? {} : { roles: p.roles }) };
    db.prepare(
      "INSERT INTO branch_policy (projectId, protected, requireReview, autoMerge) VALUES (?, ?, ?, ?) " +
        "ON CONFLICT (projectId) DO UPDATE SET protected = excluded.protected, requireReview = excluded.requireReview, autoMerge = excluded.autoMerge",
    ).run(projectId, JSON.stringify(["main"]), p.requireReview === true ? 1 : 0, JSON.stringify(autoMerge));
  } finally {
    db.close();
  }
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

/** A session `by` started (a person: `local`; a parent: its own client); its credential's client. */
async function start(by: NdjsonClient, projectId: string, more: Record<string, unknown> = {}) {
  const { session } = ok<{ session: Session }>(await by.call("agent.session.start", { projectId, harness: "fake", prompt: "go", ...more }));
  await until(() => get(session.id).status === "running", `${session.id} running`);
  return { session, client: await server.agent(credentialPath(server.core.store.root, session.id)) };
}

const branches = async (c: NdjsonClient, projectId: string) => ok<{ branches: BranchView[] }>(await c.call("branch.list", { projectId, all: true })).branches;
const head = async (c: NdjsonClient, projectId: string, branch: string) => (await branches(c, projectId)).find((b) => b.id === branch)!.head;

/** A commit on `on` as `c`; its txId. */
async function edit(c: NdjsonClient, projectId: string, on: string, at = 0): Promise<string> {
  const txId = id();
  ok(await commit(c, projectId, { ...tx(txId, 1, [insert(at)]), branch: on }));
  return txId;
}

/** The session's review request, the trigger's outcomes for it once they have run. */
async function request(c: NdjsonClient, projectId: string, source: string): Promise<ReviewItem> {
  const { item } = ok<{ item: ReviewItem }>(await c.call("review.request", { projectId, source, title: "Ready" }));
  await server.core.reviewLink.settled();
  return item;
}

const itemOf = async (local: NdjsonClient, itemId: string) => ok<{ item: ReviewItem }>(await local.call("review.get", { itemId })).item;

/** Passing QA and critic verdicts for `snapshot`, the critic's by `critic`. */
const passing = (snapshot: Snapshot, critic: string | undefined, at = 1): JudgedVerdict[] => [
  { kind: "qa", pass: true, verdictId: `qa${at}`, snapshot, at },
  { kind: "critic", pass: true, verdictId: `cr${at}`, snapshot, at: at + 1, ...(critic === undefined ? {} : { session: critic }) },
];

/** An `auto` session with one commit and its review requested (the first trigger ends at 5E: no verdicts yet). */
async function authored(b: Booted) {
  const author = await start(b.local, b.projectId, { mode: "auto" });
  const root = author.session.rootBranch!;
  const t1 = await edit(author.client, b.projectId, root);
  const item = await request(author.client, b.projectId, root);
  // What that first trigger read and asked of 5E is not the test's subject.
  b.source.reads = 0;
  vi.mocked(tryAutoMerge).mockClear();
  return { author, root, t1, item };
}

/** A critic on the reviewed branch (`<reviewed>.critic-1`, its base the reviewed head). */
const criticOf = (b: Booted, root: string) => start(b.local, b.projectId, { role: "critic", mode: "draft", base: root });

const ledger = (txId: string) => server.core.store.outcomes.of(txId).map((o) => ({ verdict: o.verdict, via: o.via, by: o.by, emitted: o.emitted }));

/** The `review.verdict` entries journalled so far (`diagnostics.status` flushes the journal). */
async function journalled(local: NdjsonClient): Promise<Array<Record<string, any>>> { // eslint-disable-line @typescript-eslint/no-explicit-any
  ok(await local.call("diagnostics.status", {}));
  return entries(serverData, "events").filter((e) => e.kind === "review.verdict").map((e) => e.payload);
}

describe("nothing merges by default", () => {
  test("nothing_merges_by_default: unavailable, the verdicts are never read, the item stays open", async () => {
    const b = await boot({ available: false });
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    b.source.list = passing(item.snapshot!, critic.session.id);
    await request(author.client, b.projectId, root);
    expect(b.outcomes).toEqual([
      { merged: false, reason: "unavailable" },
      { merged: false, reason: "unavailable" },
    ]);
    expect(b.source.reads).toBe(0);
    expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("with no verdict source injected the server reads review_verdicts, and still nothing merges while unavailable", async () => {
    const b = await boot({ available: false, verdicts: "none" });
    policy(b.projectId);
    const { item } = await authored(b);
    expect(b.outcomes).toEqual([{ merged: false, reason: "unavailable" }]);
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });
});

describe("the session-side checks", () => {
  test("draft_session_never_triggers", async () => {
    const b = await boot();
    policy(b.projectId);
    const s = await start(b.local, b.projectId, { mode: "draft" });
    await edit(s.client, b.projectId, s.session.rootBranch!);
    const item = await request(s.client, b.projectId, s.session.rootBranch!);
    expect(b.outcomes).toEqual([{ merged: false, reason: "notAuto" }]);
    expect(b.source.reads).toBe(0);
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("policy off: refused before any session fact is read", async () => {
    const b = await boot();
    policy(b.projectId, { enabled: false });
    await authored(b);
    expect(b.outcomes).toEqual([{ merged: false, reason: "policyOff" }]);
    expect(b.source.reads).toBe(0);
  });

  test("role_not_admitted: the policy's roles leave the editor out; absent roles admit every role", async () => {
    const b = await boot();
    policy(b.projectId, { roles: ["colourist"] });
    const { item, root, author } = await authored(b);
    expect(b.outcomes).toEqual([{ merged: false, reason: "roleNotAdmitted" }]);
    expect((await itemOf(b.local, item.id)).state).toBe("open");
    // The same session, admitted: it goes on to the next check (no critic verdict yet).
    policy(b.projectId, { roles: ["colourist", "editor"] });
    await request(author.client, b.projectId, root);
    expect(b.outcomes[1]).toEqual({ merged: false, reason: "verdictMissing" });
  });

  test("open_approval_blocks", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    b.source.list = passing(item.snapshot!, critic.session.id);
    server.core.store.openApproval({ sessionId: author.session.id, kind: "budget", subject: { kind: "tokens", used: 1, limit: 1 }, argsHash: "budget:tokens", expiresAt: server.time.now() + 60_000 });
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toEqual({ merged: false, reason: "approvalOpen" });
    expect(b.source.reads).toBe(0);
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("over_budget_blocks", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    b.source.list = passing(item.snapshot!, critic.session.id);
    const l = server.core.store.ledger();
    const tokens = l.read(author.session.id, "tokens");
    l.add(author.session.id, "tokens", { used: tokens.limit! - tokens.used });
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toEqual({ merged: false, reason: "overBudget" });
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("critic_must_be_distinct: the author, its child, its grandchild, an unknown session and a verdict naming none", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const child = await start(author.client, b.projectId, { role: "clip-finder" });
    const grandchild = await start(child.client, b.projectId, { role: "clip-finder" });
    expect(get(grandchild.session.id).rootSession).toBe(author.session.id);
    const cases: Array<[string | undefined, string]> = [
      [author.session.id, "criticNotDistinct"],
      [child.session.id, "criticNotDistinct"],
      [grandchild.session.id, "criticNotDistinct"],
      ["01ZZZZZZZZZZZZZZZZZZZZZZZZ", "criticUnknown"],
      [undefined, "criticUnknown"],
    ];
    for (const [critic, reason] of cases) {
      b.source.list = passing(item.snapshot!, critic);
      await request(author.client, b.projectId, root);
      expect(b.outcomes.at(-1), String(critic)).toEqual({ merged: false, reason });
    }
    expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("the newest critic verdict is the one checked: an older distinct critic does not stand for a newer one in the tree", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    b.source.list = [...passing(item.snapshot!, critic.session.id), { ...passing(item.snapshot!, author.session.id, 5)[1]! }];
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toEqual({ merged: false, reason: "criticNotDistinct" });
  });

  test("critic_on_older_head_blocks: a critic that started before the reviewed head moved", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    // The author commits again and the review is refreshed: the snapshot is the new head.
    await edit(author.client, b.projectId, root, 1);
    ok(await author.client.call("review.refresh", { itemId: item.id }));
    const now = await itemOf(b.local, item.id);
    expect(now.snapshot!.sourceSeq).toBe(await head(b.local, b.projectId, root));
    expect(get(critic.session.id).base.seq).toBeLessThan(now.snapshot!.sourceSeq);
    b.source.list = passing(now.snapshot!, critic.session.id);
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toEqual({ merged: false, reason: "criticStale" });
    expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });
});

describe("the merge", () => {
  test("merges_when_all_hold: 5E merges as policy; the ledger records merged by policy via autoMerge, once", async () => {
    const b = await boot();
    policy(b.projectId);
    const { item, root, author, t1 } = await authored(b);
    expect(b.outcomes).toEqual([{ merged: false, reason: "verdictMissing" }]);
    const critic = await criticOf(b, root);
    expect(get(critic.session.id).base).toEqual({ branch: root, seq: item.snapshot!.sourceSeq });
    b.source.list = passing(item.snapshot!, critic.session.id);
    const mainBefore = await head(b.local, b.projectId, "main");
    await request(author.client, b.projectId, root);
    const merged = b.outcomes.at(-1)!;
    expect(merged).toMatchObject({ merged: true });
    expect(vi.mocked(tryAutoMerge)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(tryAutoMerge).mock.calls[0]![1]).toBe(author.session.id);
    const done = await itemOf(b.local, item.id);
    expect(done).toMatchObject({ state: "merged", mergeId: merged.merged ? merged.mergeId : "" });
    expect(await head(b.local, b.projectId, "main")).toBeGreaterThan(mainBefore);
    // The ledger: merged, by policy, via autoMerge, not emitted (5E's verdict is the entry).
    expect(ledger(t1)).toEqual([{ verdict: "merged", via: "autoMerge", by: "policy", emitted: false }]);
    const seen = (await journalled(b.local)).filter((p) => p.txIds?.includes(t1));
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ verdict: "merged", by: "policy", via: "autoMerge", itemKind: "branchReview" });
    // A later delete abandons nothing: the merge was recorded.
    ok(await confirmed(await server.console(), "agent.session.delete", { sessionId: author.session.id }));
    expect(ledger(t1)).toEqual([{ verdict: "merged", via: "autoMerge", by: "policy", emitted: false }]);
    expect((await journalled(b.local)).filter((p) => p.txIds?.includes(t1))).toHaveLength(1);
  });

  test("5E judges the verdicts the trigger checked: they are read once", async () => {
    let reads = 0;
    let checked: JudgedVerdict[] = [];
    const b = await boot({
      verdicts: {
        // The first read is what the trigger checks; any later read would see a newer, failing critic verdict.
        verdicts: () => (reads++ === 0 ? checked : [...checked, { ...checked[1]!, verdictId: "cr9", at: 9, pass: false }]),
      },
    });
    policy(b.projectId);
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    checked = passing(item.snapshot!, critic.session.id);
    reads = 0;
    await request(author.client, b.projectId, root);
    expect(reads).toBe(1);
    expect(b.outcomes.at(-1)).toMatchObject({ merged: true });
  });

  test("a 5E refusal (requireReview) leaves the item open", async () => {
    const b = await boot();
    policy(b.projectId, { requireReview: true });
    const { item, root, author } = await authored(b);
    const critic = await criticOf(b, root);
    b.source.list = passing(item.snapshot!, critic.session.id);
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toEqual({ merged: false, reason: "requireReview" });
    expect(vi.mocked(tryAutoMerge)).toHaveBeenCalled();
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });

  test("stored verdicts: the server's default source reads review_verdicts with the critic's session", async () => {
    const b = await boot({ verdicts: "none" });
    policy(b.projectId);
    const { item, root, author, t1 } = await authored(b);
    const critic = await criticOf(b, root);
    const db = new DatabaseSync(controlDbPath(serverData));
    try {
      const insert = db.prepare("INSERT INTO review_verdicts (verdictId, itemId, kind, snapshot, pass, at, session) VALUES (?, ?, ?, ?, ?, ?, ?)");
      for (const v of passing(item.snapshot!, critic.session.id)) insert.run(v.verdictId, item.id, v.kind, JSON.stringify(v.snapshot), 1, v.at, v.session ?? null);
    } finally {
      db.close();
    }
    await request(author.client, b.projectId, root);
    expect(b.outcomes.at(-1)).toMatchObject({ merged: true });
    expect(ledger(t1)).toEqual([{ verdict: "merged", via: "autoMerge", by: "policy", emitted: false }]);
  });
});

describe("events the trigger only refuses", () => {
  test("a session end without a review: noReview, nothing read", async () => {
    const b = await boot();
    policy(b.projectId);
    const s = await start(b.local, b.projectId, { mode: "auto" });
    await edit(s.client, b.projectId, s.session.rootBranch!);
    ok(await b.local.call("agent.session.end", { sessionId: s.session.id }));
    await server.core.reviewLink.settled();
    expect(b.ready).toHaveLength(1);
    expect(b.ready[0]!.reviewId).toBeUndefined();
    expect(b.outcomes).toEqual([{ merged: false, reason: "noReview" }]);
    expect(b.source.reads).toBe(0);
  });

  test("trigger errors never fail the caller's RPC", async () => {
    const b = await boot({
      verdicts: {
        verdicts: () => {
          throw new Error("the verdict store is gone");
        },
      },
    });
    policy(b.projectId);
    const author = await start(b.local, b.projectId, { mode: "auto" });
    const root = author.session.rootBranch!;
    await edit(author.client, b.projectId, root);
    const exit = await author.client.call("review.request", { projectId: b.projectId, source: root, title: "Ready" });
    expect(exit.ok).toBe(true);
    await server.core.reviewLink.settled();
    expect(triggered).toHaveLength(1);
    expect(b.outcomes).toEqual([]);
    const item = (exit as { value: { item: ReviewItem } }).value.item;
    expect((await itemOf(b.local, item.id)).state).toBe("open");
  });
});

/** Every `.ts` file under `src/` (path relative to `src/`, text). */
function sources(): ReadonlyArray<[string, string]> {
  const src = join(import.meta.dirname, "..", "src");
  const found: Array<[string, string]> = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (name.endsWith(".ts")) found.push([relative(src, path).split("\\").join("/"), readFileSync(path, "utf8")]);
    }
  };
  walk(src);
  return found;
}

const importers = (re: RegExp) => sources().filter(([, text]) => re.test(text)).map(([p]) => p).sort();

describe("no_rpc_reaches_the_trigger", () => {
  test("only the server wiring builds the trigger, and only the trigger imports 5E's path", () => {
    expect(importers(/(?:\bfrom\s*|\bimport\s*\(?\s*)["'`][^"'`\n]*agent\/auto-merge(?:\.[cm]?[jt]s)?["'`]|from\s*["'`]\.\/auto-merge(?:\.ts)?["'`]/)).toEqual(["agent/wiring.ts", "main.ts"]);
    // `agent/wiring.ts` holds the options' type, nothing that runs.
    const wiring = sources().find(([p]) => p === "agent/wiring.ts")![1];
    expect(wiring.split("\n").filter((l) => /auto-merge/.test(l) && /\bimport\b/.test(l)).every((l) => /^import type\b/.test(l.trim()))).toBe(true);
    expect(importers(/\bautoMergeTrigger\(/)).toEqual(["agent/auto-merge.ts", "main.ts"]);
    expect(importers(/(?:\bfrom\s*|\bimport\s*\(?\s*)["'`][^"'`\n]*reviews\/auto-merge(?:\.[cm]?[jt]s)?["'`]/)).toEqual(["agent/auto-merge.ts"]);
  });

  test("every RPC tag as local, console and an auto session reaches the trigger only through branchReady, and nothing merges", { timeout: 240_000 }, async () => {
    const b = await boot({ available: false });
    policy(b.projectId);
    const author = await start(b.local, b.projectId, { mode: "auto" });
    const root = author.session.rootBranch!;
    await edit(author.client, b.projectId, root);
    const consoleClient = await server.console();
    const LAST = new Set(["server.stop", "auth.revokeSelf", "agent.session.delete", "agent.session.end"]);
    const tags = CAPABILITIES.filter((t) => !LAST.has(t));
    const payloadOf = (tag: string) => {
      const sample = JSON.stringify(SAMPLE_PAYLOADS[tag as FinRpcTag] ?? {}).replaceAll("01aaaaaaaaaaaaaaaaaaaaaaaa", b.projectId).replaceAll('"try"', JSON.stringify(root));
      return JSON.parse(sample) as Record<string, unknown>;
    };
    const sweep = async (c: NdjsonClient, tag: string) => {
      const payload = payloadOf(tag);
      if (STREAM_RPCS.includes(tag)) {
        const s = c.stream(tag, payload, () => undefined);
        await Promise.race([s.done, new Promise((r) => setTimeout(r, 50))]);
        c.interrupt(s.id);
        return;
      }
      await Promise.race([c.call(tag, payload).catch(() => undefined), new Promise((r) => setTimeout(r, 2_000))]);
    };
    for (const c of [b.local, consoleClient]) for (const tag of tags) await sweep(c, tag);
    await server.core.reviewLink.settled();
    // A person's calls never make a branch ready.
    expect(triggered).toEqual([]);
    for (const tag of tags) await sweep(author.client, tag);
    await server.core.reviewLink.settled();
    // The session's own calls reach it only as branchReady events (its review requests).
    expect(triggered).toEqual(b.ready);
    expect(b.outcomes.every((o) => !o.merged)).toBe(true);
    expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
  });
});
