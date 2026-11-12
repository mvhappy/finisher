// The internal auto-merge path (5E plan Task 9, Decision 19; spec §8.5): the policy actor merges
// a branch review only while the path is available, the owner's policy is on, `requireReview`
// is off and the QA gate and the critic both passed the item's exact snapshot; every check
// refuses in order and leaves the item open; the merge is bounded by the judged snapshot,
// audited `auto: true` and journalled `by: policy`, `via: autoMerge`; and no RPC, from any
// principal kind with the most permissive payloads the schemas allow, reaches the path.
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import {
  auditDetailsValid,
  type FinError,
  type Principal,
  type PrincipalKind,
  type ReviewItem,
  SCOPES,
  STREAM_RPCS,
  type Snapshot,
  WRITE_RPCS,
} from "@finisher/contracts";
import type { BranchView, ReviewPlan } from "@finisher/core-types";
import { Engine } from "@finisher/fin-node";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { ChainProjects } from "../src/chain/resolve.ts";
import type { AuditInput } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { toFinError } from "../src/errors.ts";
import { CAPABILITIES } from "../src/handlers.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { type JudgedVerdict, judge, policyGuard, storedVerdicts, tryAutoMerge, type VerdictSource } from "../src/reviews/auto-merge.ts";
import { mergeReview } from "../src/reviews/merge.ts";
import type { ReviewEngine } from "../src/reviews/plan.ts";
import { AUTO_MERGE_AVAILABLE, writePolicy } from "../src/reviews/policy.ts";
import { reviewHandlers } from "../src/reviews/rpc.ts";
import { makeReviews, type Reviews } from "../src/reviews/service.ts";
import type { ReviewVerdictPayload } from "../src/reviews/verdicts.ts";
import { callerOf } from "../src/reviews/workflow.ts";
import { chainHarness } from "./support/chain-harness.ts";
import { addAsset, commit, connectProved, type NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";
import { type FinRpcTag, SAMPLE_PAYLOADS } from "./support/payloads.ts";

// The spy: every import of the module, the server's included, gets the counted `tryAutoMerge`.
vi.mock("../src/reviews/auto-merge.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("../src/reviews/auto-merge.ts")>();
  return { ...real, tryAutoMerge: vi.fn(real.tryAutoMerge) };
});

const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const SESSION = "01sess0000000000000000000a";
const AGENT_AUTHOR = "agent:01agent000000000000000000a";
const ENABLED_BY = "console:console";

let tmp: string;
const engines: Engine[] = [];
const controls: ControlStore[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-auto-merge-"));
  vi.mocked(tryAutoMerge).mockClear();
});

afterEach(async () => {
  await Promise.all(engines.splice(0).map((e) => e.close().catch(() => undefined)));
  await Promise.all(controls.splice(0).map((c) => c.close().catch(() => undefined)));
  rmSync(tmp, { recursive: true, force: true });
});

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const json = <A>(f: () => Promise<string>): Effect.Effect<A, FinError> => Effect.tryPromise({ try: async () => JSON.parse(await f()) as A, catch: toFinError });

/** The workflow's engine port over a fin-node `Engine` (one project). */
function engineOf(e: Engine): ReviewEngine {
  return {
    records: (_p, branch) => json(() => e.opsSince(branch, 0)),
    resolveBranch: (_p, ref) => json(() => e.resolveBranch(ref)),
    branches: () => json<BranchView[]>(() => e.branches(true)),
    plan: (_p, from, into) => json(() => e.reviewPlan(JSON.stringify({ from, into }))),
    write: (_p, op, payload, meta) => {
      const { projectId: _projectId, ...req } = payload as Record<string, unknown>;
      return json(() => e[op](JSON.stringify(req), JSON.stringify(meta)));
    },
  };
}

const asset = (txId: string) => ({ ...addAsset(`$${txId}`), path: `/media/${txId}.mov`, fingerprint: txId });

interface Fixture {
  readonly e: Engine;
  readonly reviews: Reviews;
  readonly control: ControlStore;
  readonly audited: AuditInput[];
  readonly entries: ReviewVerdictPayload[];
  readonly feature: string;
  /** The engine port the reviews service uses (a fresh service over it is a restart). */
  readonly engine: ReviewEngine;
  readonly item: ReviewItem;
  commitOn(branch: string, txId: string): Promise<void>;
}

/**
 * A project whose `main` holds an asset and whose `feature` branch (made by an agent) holds
 * `txIds`; a review of `feature` into `main` requested by `local`.
 */
async function fixture(txIds: ReadonlyArray<string> = ["f1", "f2"], wrap: (e: ReviewEngine) => ReviewEngine = (x) => x): Promise<Fixture> {
  const root = mkdtempSync(join(tmp, "fx-"));
  mkdirSync(join(root, "sd"));
  mkdirSync(join(root, "control"));
  const e = await Engine.create(join(root, "sd"), join(root, "p"), "Demo", "30");
  engines.push(e);
  await e.commit(JSON.stringify({ txId: "m0", author: "u", branch: "main", baseRev: 0, schemaVersion: 2, ops: [asset("m0")] }));
  const created = JSON.parse(await e.branchCreate(JSON.stringify({ txId: "b1", author: AGENT_AUTHOR, name: "feature", from: "main" }))) as { branch: BranchView };
  const feature = created.branch.id;
  const commitOn = async (branch: string, txId: string) => {
    await e.commit(JSON.stringify({ txId, author: AGENT_AUTHOR, branch, baseRev: await e.head(branch), schemaVersion: 2, ops: [asset(txId)] }));
  };
  for (const t of txIds) await commitOn(feature, t);
  const control = openControlStore(join(root, "control"), { serverId: "sid-1", checkpoint: false });
  controls.push(control);
  const audited: AuditInput[] = [];
  const entries: ReviewVerdictPayload[] = [];
  const engine = wrap(engineOf(e));
  const reviews = makeReviews(control, {
    engine,
    audit: { append: (a) => void audited.push(a) },
    verdicts: { emit: (_p, _a, es) => void entries.push(...es) },
  });
  const local = callerOf(LOCAL, { actor: { kind: "user" } }, "socket");
  const { item } = await run(reviews.workflow.request(local, { projectId: A, source: "feature", title: "Please look" }));
  return { e, reviews, control, audited, entries, feature, engine, item, commitOn };
}

/** The owner turned auto merge on (as `review.policy` will once it is available). */
function enable(f: Fixture, extra: { requireReview?: boolean; enabledBy?: string | undefined } = {}): void {
  f.reviews.store.setPolicy(A, { protected: ["main"], requireReview: extra.requireReview ?? false, autoMerge: { enabled: true } }, "enabledBy" in extra ? extra.enabledBy : ENABLED_BY);
}

/** A passing QA verdict and a passing critic verdict for `snapshot`. */
const passing = (snapshot: Snapshot): JudgedVerdict[] => [
  { kind: "qa", pass: true, verdictId: "qa1", snapshot, at: 1 },
  { kind: "critic", pass: true, verdictId: "cr1", snapshot, at: 2 },
];

function source(verdicts: ReadonlyArray<JudgedVerdict>): VerdictSource & { calls: number } {
  const s = {
    calls: 0,
    verdicts: () => {
      s.calls++;
      return verdicts;
    },
  };
  return s;
}

const deps = (f: Fixture, verdicts: VerdictSource, available: boolean | undefined) => ({ reviews: f.reviews, verdicts, available, audit: { append: (a: AuditInput) => void f.audited.push(a) } });
/** `available`: `true` unless given; `"default"` leaves it unset (`AUTO_MERGE_AVAILABLE`). */
const attempt = (f: Fixture, verdicts: VerdictSource, available: boolean | "default" = true) => run(tryAutoMerge(f.item.id, SESSION, deps(f, verdicts, available === "default" ? undefined : available)));

/** What a merge of `feature` into `main` would do now, by origin. */
async function statuses(f: Fixture): Promise<Record<string, string>> {
  const plan = JSON.parse(await f.e.reviewPlan(JSON.stringify({ from: f.feature, into: "main" }))) as ReviewPlan;
  return Object.fromEntries(plan.entries.filter((e) => e.branch === f.feature).map((e) => [e.origin, e.status]));
}

/** Nothing happened: the item is open with its events as they were, `main` did not move, nothing was audited or journalled. */
async function nothingHappened(f: Fixture, mainHead: number, events: number): Promise<void> {
  expect(f.reviews.store.get(f.item.id)?.state).toBe("open");
  expect(f.reviews.store.events(f.item.id)).toHaveLength(events);
  expect(await f.e.head("main")).toBe(mainHead);
  expect(f.audited).toEqual([]);
  expect(f.entries).toEqual([]);
}

describe("the checks, in order", () => {
  test("nothing_merges_when_unavailable", async () => {
    expect(AUTO_MERGE_AVAILABLE).toBe(false);
    const f = await fixture();
    enable(f);
    const s = source(passing(f.item.snapshot!));
    const head = await f.e.head("main");
    // The default (`AUTO_MERGE_AVAILABLE`), and `false` given.
    expect(await attempt(f, s, "default")).toEqual({ merged: false, reason: "unavailable" });
    expect(await attempt(f, s, false)).toEqual({ merged: false, reason: "unavailable" });
    expect(s.calls).toBe(0);
    await nothingHappened(f, head, 1);
  });

  test("policy_off_refuses", async () => {
    const f = await fixture();
    const s = source(passing(f.item.snapshot!));
    const head = await f.e.head("main");
    // The default policy (off).
    expect(await attempt(f, s)).toEqual({ merged: false, reason: "policyOff" });
    // On, but nobody is recorded as having turned it on: refused too.
    enable(f, { enabledBy: undefined });
    expect(f.reviews.store.autoMergeEnabledBy(A)).toBeUndefined();
    expect(await attempt(f, s)).toEqual({ merged: false, reason: "policyOff" });
    // Turned off again.
    enable(f);
    f.reviews.store.setPolicy(A, { protected: ["main"], requireReview: false, autoMerge: { enabled: false } });
    expect(await attempt(f, s)).toEqual({ merged: false, reason: "policyOff" });
    expect(s.calls).toBe(0);
    await nothingHappened(f, head, 1);
  });

  test("require_review_refuses", async () => {
    const f = await fixture();
    enable(f, { requireReview: true });
    const s = source(passing(f.item.snapshot!));
    const head = await f.e.head("main");
    expect(await attempt(f, s)).toEqual({ merged: false, reason: "requireReview" });
    expect(s.calls).toBe(0);
    await nothingHappened(f, head, 1);
  });

  test("missing_verdict_refuses", async () => {
    const f = await fixture();
    enable(f);
    const [qa, critic] = passing(f.item.snapshot!);
    const head = await f.e.head("main");
    for (const verdicts of [[], [qa!], [critic!]]) expect(await attempt(f, source(verdicts))).toEqual({ merged: false, reason: "verdictMissing" });
    await nothingHappened(f, head, 1);
  });

  test("failed_verdict_refuses", async () => {
    const f = await fixture();
    enable(f);
    const snapshot = f.item.snapshot!;
    const [qa, critic] = passing(snapshot);
    const head = await f.e.head("main");
    // The critic failed; the QA gate failed; a newer failing QA verdict outweighs an older pass.
    for (const verdicts of [
      [qa!, { ...critic!, pass: false }],
      [{ ...qa!, pass: false }, critic!],
      [qa!, critic!, { ...qa!, verdictId: "qa2", pass: false, at: 3 }],
    ]) {
      expect(await attempt(f, source(verdicts))).toEqual({ merged: false, reason: "verdictFailed" });
    }
    // Missing comes before failed.
    expect(await attempt(f, source([{ ...qa!, pass: false }]))).toEqual({ merged: false, reason: "verdictMissing" });
    await nothingHappened(f, head, 1);
  });

  test("older_snapshot_verdict_refuses", async () => {
    const f = await fixture();
    enable(f);
    const judged = f.item.snapshot!;
    // The branch moved and the review was refreshed: the verdicts judged the older snapshot.
    await f.commitOn(f.feature, "f3");
    const local = callerOf(LOCAL, { actor: { kind: "user" } }, "socket");
    const refreshed = (await run(f.reviews.workflow.refresh(local, f.item.id))).item;
    expect(refreshed.snapshot!.sourceSeq).toBe(judged.sourceSeq + 1);
    const head = await f.e.head("main");
    expect(await attempt(f, source(passing(judged)))).toEqual({ merged: false, reason: "snapshotMismatch" });
    // One verdict for the new snapshot is not enough; the same seqs with another hash are not either.
    const [qa] = passing(refreshed.snapshot!);
    expect(await attempt(f, source([qa!, { ...passing(judged)[1]!, at: 5 }]))).toEqual({ merged: false, reason: "snapshotMismatch" });
    const otherHash = { ...refreshed.snapshot!, sourceHash: "blake3:" + "0".repeat(64) };
    expect(await attempt(f, source(passing(otherHash)))).toEqual({ merged: false, reason: "snapshotMismatch" });
    // Failed comes before a mismatch.
    expect(await attempt(f, source([passing(judged)[0]!, { ...passing(judged)[1]!, pass: false }]))).toEqual({ merged: false, reason: "verdictFailed" });
    await nothingHappened(f, head, 2);
  });

  test("item_stays_open_on_refusal", async () => {
    const f = await fixture();
    const snapshot = f.item.snapshot!;
    const head = await f.e.head("main");
    const cases: Array<[string, () => void, ReadonlyArray<JudgedVerdict>, boolean | "default"]> = [
      ["unavailable", () => enable(f), passing(snapshot), "default"],
      ["policyOff", () => f.reviews.store.setPolicy(A, { protected: ["main"], requireReview: false, autoMerge: { enabled: false } }), passing(snapshot), true],
      ["requireReview", () => enable(f, { requireReview: true }), passing(snapshot), true],
      ["verdictMissing", () => enable(f), passing(snapshot).slice(1), true],
      ["verdictFailed", () => enable(f), passing(snapshot).map((v) => ({ ...v, pass: false })), true],
      ["snapshotMismatch", () => enable(f), passing({ ...snapshot, targetSeq: snapshot.targetSeq - 1 }), true],
    ];
    for (const [reason, setup, verdicts, available] of cases) {
      setup();
      expect(await attempt(f, source(verdicts), available), reason).toEqual({ merged: false, reason });
      await nothingHappened(f, head, 1);
    }
    // Nothing was decided either: a person can still take it from here.
    expect(f.reviews.store.decisions(f.item.id).size).toBe(0);
    expect(await statuses(f)).toEqual({ f1: "willMerge", f2: "willMerge" });
    // Not an open branch review: the human path's errors.
    expect(await Effect.runPromise(Effect.flip(tryAutoMerge("01zzzzzzzzzzzzzzzzzzzzzzzz", SESSION, deps(f, source(passing(snapshot)), true))))).toMatchObject({ code: "REVIEW_NOT_FOUND" });
  });
});

describe("the merge", () => {
  test("merges_with_both_verdicts_passing", async () => {
    const f = await fixture();
    enable(f);
    const judged = f.item.snapshot!;
    // A commit after the judged snapshot: never merged by it.
    await f.commitOn(f.feature, "f3");
    const result = await attempt(f, source(passing(judged)));
    expect(result).toMatchObject({ merged: true });
    const item = f.reviews.store.get(f.item.id)!;
    expect(item).toMatchObject({ state: "merged", closedBy: "policy:autoMerge" });
    expect(result.merged && item.mergeId === result.mergeId).toBe(true);
    // Bounded by the judged snapshot's `sourceUpTo`: f1 and f2 (pending, accepted by the policy), not f3.
    expect(await statuses(f)).toEqual({ f1: "alreadyPresent", f2: "alreadyPresent", f3: "willMerge" });
    // f3 is left, so the agent's branch stays.
    expect((JSON.parse(await f.e.branches(true)) as BranchView[]).find((b) => b.id === f.feature)?.state).not.toBe("archived");
    // Audited once, `auto: true`, with the snapshot, the session, the verdicts and who enabled it; no `auto: false` row.
    expect(f.audited).toHaveLength(1);
    const row = f.audited[0]!;
    expect(row).toMatchObject({ kind: "review.merge", principal: { kind: "policy", id: "autoMerge" }, outcome: "ok" });
    expect(row.details).toEqual({
      projectId: A,
      item: f.item.id,
      protected: true,
      auto: true,
      snapshot: { sourceSeq: judged.sourceSeq, sourceHash: judged.sourceHash, targetSeq: judged.targetSeq, targetHash: judged.targetHash },
      session: SESSION,
      verdicts: [
        { kind: "qa", verdictId: "qa1" },
        { kind: "critic", verdictId: "cr1" },
      ],
      enabledBy: ENABLED_BY,
    });
    expect(auditDetailsValid("review.merge", row.details)).toBe(true);
    // The verdict says it was the policy, never a person's acceptance.
    expect(f.entries).toEqual([expect.objectContaining({ itemKind: "branchReview", verdict: "merged", by: "policy", via: "autoMerge", txIds: ["f1", "f2"], accepted: 2, sessionId: SESSION })]);
    // A second attempt finds the item merged by this path: the same answer, not audited again.
    expect(await run(attempt2(f))).toEqual(result);
    expect(f.audited).toHaveLength(1);
    // A person's merge of another snapshot is not this path's: closed.
    const local = callerOf(LOCAL, { actor: { kind: "user" } }, "socket");
    const other = await fixture(["g1"]);
    enable(other);
    await run(other.reviews.workflow.decide(local, { itemId: other.item.id, expect: other.item.snapshot!, decisions: [{ txId: "g1", decision: "accept" }] }));
    const decided = other.reviews.store.get(other.item.id)!;
    await run(mergeReview(other.reviews.workflow, local, { itemId: decided.id, expect: decided.snapshot!, idemKey: "by-hand" }));
    expect(await Effect.runPromise(Effect.flip(tryAutoMerge(other.item.id, SESSION, deps(other, source(passing(decided.snapshot!)), true))))).toMatchObject({ code: "REVIEW_CLOSED" });
  });

  test("the merge finishes the agent's branch and carries the policy actor's guard", async () => {
    let seen: { op: string; meta: unknown; author: unknown }[] = [];
    const f = await fixture(["f1"], (en) => ({
      ...en,
      write: (p, op, payload, meta) => {
        seen.push({ op, meta, author: (payload as { author?: unknown }).author });
        return en.write(p, op, payload, meta);
      },
    }));
    f.reviews.store.setPolicy(A, { protected: ["main", "keep"], requireReview: false, autoMerge: { enabled: true } }, ENABLED_BY);
    seen = [];
    expect(await attempt(f, source(passing(f.item.snapshot!)))).toMatchObject({ merged: true });
    const guard = { allowBranches: ["main", f.feature], denyBranches: ["keep"] };
    expect(policyGuard(f.item, ["main", "keep"])).toEqual(guard);
    expect(seen).toEqual([
      { op: "merge", meta: { actor: { kind: "system", session: SESSION }, guard }, author: "policy:autoMerge" },
      { op: "branchSet", meta: { actor: { kind: "system", session: SESSION }, guard }, author: "policy:autoMerge" },
    ]);
    expect((JSON.parse(await f.e.branches(true)) as BranchView[]).find((b) => b.id === f.feature)?.state).toBe("archived");
  });

  test("stored verdicts are read from review_verdicts", async () => {
    const f = await fixture();
    enable(f);
    const insert = f.control.db.prepare("INSERT INTO review_verdicts (verdictId, itemId, kind, snapshot, pass, at) VALUES (?, ?, ?, ?, ?, ?)");
    insert.run("qa1", f.item.id, "qa", JSON.stringify(f.item.snapshot), 1, 1);
    const stored = storedVerdicts(f.reviews.store);
    expect(await attempt(f, stored)).toEqual({ merged: false, reason: "verdictMissing" });
    insert.run("cr1", f.item.id, "critic", JSON.stringify(f.item.snapshot), 1, 2);
    expect(stored.verdicts(f.item.id).map((v) => [v.kind, v.pass])).toEqual([
      ["qa", true],
      ["critic", true],
    ]);
    expect(await attempt(f, stored)).toMatchObject({ merged: true });
  });

  test("policy_turned_off_in_flight_refuses_or_completes_first", async () => {
    // Refused: a policy write that began first (behind a write in flight) turns it off before the auto merge is checked.
    {
      const f = await fixture();
      enable(f);
      const head = await f.e.head("main");
      let open!: () => void;
      const held = new Promise<void>((r) => (open = r));
      const write = Effect.runPromise(f.reviews.gate.shared(A, Effect.promise(() => held)));
      await tick();
      const off = Effect.runPromise(writePolicy(policyDeps(f), CONSOLE, { projectId: A, autoMerge: { enabled: false } }));
      await tick();
      const auto = attempt(f, source(passing(f.item.snapshot!)));
      await tick();
      open();
      await write;
      await off;
      expect(await auto).toEqual({ merged: false, reason: "policyOff" });
      expect(await f.e.head("main")).toBe(head);
      expect(f.reviews.store.get(f.item.id)?.state).toBe("open");
    }
    // Completes first: the policy write waits for the auto merge in flight.
    {
      let entered!: () => void;
      const merging = new Promise<void>((r) => (entered = r));
      let release!: () => void;
      const gate = new Promise<void>((r) => (release = r));
      const f = await fixture(["f1", "f2"], (en) => ({
        ...en,
        write: (p, op, payload, meta) =>
          op === "merge"
            ? Effect.flatMap(
                Effect.promise(() => {
                  entered();
                  return gate;
                }),
                () => en.write(p, op, payload, meta),
              )
            : en.write(p, op, payload, meta),
      }));
      enable(f);
      const auto = attempt(f, source(passing(f.item.snapshot!)));
      await merging;
      const order: string[] = [];
      const off = Effect.runPromise(writePolicy(policyDeps(f), CONSOLE, { projectId: A, autoMerge: { enabled: false } })).then(() => void order.push("policy"));
      await tick();
      expect(f.reviews.store.policy(A).autoMerge.enabled).toBe(true);
      expect(order).toEqual([]);
      release();
      const result = await auto;
      order.unshift("merge");
      await off;
      expect(result).toMatchObject({ merged: true });
      expect(order).toEqual(["merge", "policy"]);
      expect(f.audited.map((a) => a.kind)).toEqual(["review.merge", "review.policy"]);
      expect(f.reviews.store.policy(A).autoMerge.enabled).toBe(false);
      expect(f.reviews.store.autoMergeEnabledBy(A)).toBeUndefined();
    }
  });

  test("turning auto merge on records who did, and nothing else reads it", async () => {
    const f = await fixture();
    const d = { ...policyDeps(f), autoMergeAvailable: true };
    const on = await run(writePolicy(d, CONSOLE, { projectId: A, autoMerge: { enabled: true } }));
    expect(on.policy).toEqual({ protected: ["main"], requireReview: false, autoMerge: { enabled: true } });
    expect(f.reviews.store.policy(A)).toEqual(on.policy);
    expect(f.reviews.store.autoMergeEnabledBy(A)).toBe("console:console");
    // A write that does not send `autoMerge` keeps who enabled it; turning it off forgets them.
    await run(writePolicy(d, { kind: "device", id: "01device00000000000000000a" }, { projectId: A, requireReview: false }));
    expect(f.reviews.store.autoMergeEnabledBy(A)).toBe("console:console");
    await run(writePolicy(d, CONSOLE, { projectId: A, autoMerge: { enabled: false } }));
    expect(f.reviews.store.autoMergeEnabledBy(A)).toBeUndefined();
    // The audit row stays ids and booleans.
    for (const row of f.audited) expect(auditDetailsValid(row.kind, row.details)).toBe(true);
  });
});

describe("review follow-ups", () => {
  test("changes_requested_refuses", async () => {
    const f = await fixture();
    enable(f);
    const local = callerOf(LOCAL, { actor: { kind: "user" } }, "socket");
    await run(f.reviews.workflow.decide(local, { itemId: f.item.id, expect: f.item.snapshot!, requestChanges: {} }));
    const events = f.reviews.store.events(f.item.id).length;
    const head = await f.e.head("main");
    expect(await attempt(f, source(passing(f.reviews.store.get(f.item.id)!.snapshot!)))).toEqual({ merged: false, reason: "changesRequested" });
    expect(f.reviews.store.get(f.item.id)?.state).toBe("changesRequested");
    expect(f.reviews.store.events(f.item.id)).toHaveLength(events);
    expect(await f.e.head("main")).toBe(head);
    expect(f.audited).toEqual([]);
  });

  test("an accepted origin dropped since the decision refuses with dropsChanged", async () => {
    const f = await fixture();
    enable(f);
    const local = callerOf(LOCAL, { actor: { kind: "user" } }, "socket");
    const snapshot = f.item.snapshot!;
    await run(f.reviews.workflow.decide(local, { itemId: f.item.id, expect: snapshot, decisions: [{ txId: "f1", decision: "accept" }] }));
    await f.e.drop(JSON.stringify({ txId: "bob-drop", author: "bob", into: "main", from: f.feature, drops: ["f1"] }));
    const head = await f.e.head("main");
    const audited = f.audited.length;
    expect(await attempt(f, source(passing(snapshot)))).toEqual({ merged: false, reason: "dropsChanged" });
    expect(f.reviews.store.get(f.item.id)?.state).toBe("open");
    expect(await f.e.head("main")).toBe(head);
    expect(f.audited).toHaveLength(audited);
  });

  test("a session or verdict id the audit cannot record never empties the row", async () => {
    const f = await fixture();
    enable(f);
    const snapshot = f.item.snapshot!;
    const head = await f.e.head("main");
    // A session that is not an id: refused before anything is read.
    const s = source(passing(snapshot));
    expect(await Effect.runPromise(Effect.flip(tryAutoMerge(f.item.id, "not an id", deps(f, s, true))))).toMatchObject({ code: "INVALID_REQUEST", details: { field: "session" } });
    expect(s.calls).toBe(0);
    // A verdict whose id the audit cannot hold does not count: missing.
    const [qa, critic] = passing(snapshot);
    expect(await attempt(f, source([qa!, { ...critic!, verdictId: "critic verdict #1" }]))).toEqual({ merged: false, reason: "verdictMissing" });
    expect(await attempt(f, source([{ ...qa!, verdictId: "q".repeat(65) }, critic!]))).toEqual({ merged: false, reason: "verdictMissing" });
    await nothingHappened(f, head, 1);
    // A good one beside a bad one is judged on the good one, and the row keeps every field.
    expect(await attempt(f, source([qa!, critic!, { ...critic!, verdictId: "bad id", at: 0 }]))).toMatchObject({ merged: true });
    expect(f.audited).toHaveLength(1);
    expect(f.audited[0]!.details).toMatchObject({ auto: true, enabledBy: ENABLED_BY, session: SESSION, verdicts: [{ verdictId: "qa1" }, { verdictId: "cr1" }] });
    expect(auditDetailsValid("review.merge", f.audited[0]!.details)).toBe(true);
  });

  test("a failing verdict wins a tie", async () => {
    const snapshot: Snapshot = { sourceSeq: 2, targetSeq: 1, plannedAt: 0 };
    const [qa, critic] = passing(snapshot);
    const failing = { ...critic!, verdictId: "cr2", pass: false };
    expect(judge([qa!, critic!, failing], snapshot)).toBe("verdictFailed");
    expect(judge([qa!, failing, critic!], snapshot)).toBe("verdictFailed");
    expect(judge([qa!, { ...critic!, at: 1 }, { ...failing, at: 0 }], snapshot)).toEqual([qa, { ...critic!, at: 1 }]);
  });

  test("crash_between_merge_and_state_is_finished_by_a_replay", async () => {
    const f = await fixture();
    enable(f);
    const judged = f.item.snapshot!;
    // The engine merge lands, then the process dies before the item is marked merged.
    const setState = f.reviews.store.setState.bind(f.reviews.store);
    f.reviews.store.setState = ((...args: Parameters<typeof setState>) => {
      if (args[1] === "merged") throw new Error("crash");
      return setState(...args);
    }) as typeof setState;
    await expect(attempt(f, source(passing(judged)))).rejects.toThrow();
    expect(await statuses(f)).toEqual({ f1: "alreadyPresent", f2: "alreadyPresent" });
    expect(f.reviews.store.get(f.item.id)?.state).toBe("open");
    expect(f.audited).toEqual([]);
    // After a restart (a new service, nothing cached), the replay finishes the item.
    const audited: AuditInput[] = [];
    const writes: string[] = [];
    const engine: ReviewEngine = { ...f.engine, write: (p, op, payload, meta) => Effect.tap(f.engine.write(p, op, payload, meta), () => Effect.sync(() => void writes.push(op))) };
    const restarted = makeReviews(f.control, { engine, audit: { append: (a) => void audited.push(a) } });
    const head = await f.e.head("main");
    const replay = await run(tryAutoMerge(f.item.id, SESSION, { reviews: restarted, verdicts: source(passing(judged)), available: true, audit: { append: (a) => void audited.push(a) } }));
    expect(replay).toEqual({ merged: true, mergeId: `auto-${f.item.id}-${judged.sourceSeq}` });
    expect(restarted.store.get(f.item.id)).toMatchObject({ state: "merged", mergeId: replay.merged ? replay.mergeId : "" });
    // Nothing was copied twice: fin-store answered the stored merge, and the only new record is
    // the archive of the finished agent branch the crash had cut off (a `branchSet` on main).
    expect(writes).toEqual(["merge", "branchSet"]);
    expect(await f.e.head("main")).toBe(head + 1);
    expect(await statuses(f)).toEqual({ f1: "alreadyPresent", f2: "alreadyPresent" });
    expect((JSON.parse(await f.e.branches(true)) as BranchView[]).find((b) => b.id === f.feature)?.state).toBe("archived");
    expect(audited.filter((a) => a.kind === "review.merge")).toHaveLength(1);
  });

  test("after_a_cache_eviction_it_replans_or_refuses", async () => {
    // The source has not moved: the judged snapshot's list is planned again, and it merges.
    {
      const f = await fixture();
      enable(f);
      const restarted = makeReviews(f.control, { engine: f.engine });
      const r = await run(tryAutoMerge(f.item.id, SESSION, { reviews: restarted, verdicts: source(passing(f.item.snapshot!)), available: true, audit: { append: (a) => void f.audited.push(a) } }));
      expect(r).toMatchObject({ merged: true });
      expect(await statuses(f)).toEqual({ f1: "alreadyPresent", f2: "alreadyPresent" });
    }
    // The source moved since: the list cannot be planned exactly again, so a person refreshes.
    {
      const f = await fixture();
      enable(f);
      await f.commitOn(f.feature, "f3");
      const head = await f.e.head("main");
      const restarted = makeReviews(f.control, { engine: f.engine });
      const r = await run(tryAutoMerge(f.item.id, SESSION, { reviews: restarted, verdicts: source(passing(f.item.snapshot!)), available: true, audit: { append: (a) => void f.audited.push(a) } }));
      expect(r).toEqual({ merged: false, reason: "replanNeeded" });
      expect(await f.e.head("main")).toBe(head);
      expect(restarted.store.get(f.item.id)?.state).toBe("open");
      expect(f.audited).toEqual([]);
    }
  });
});

const tick = () => new Promise((r) => setTimeout(r, 20));
const attempt2 = (f: Fixture) => tryAutoMerge(f.item.id, SESSION, deps(f, source(passing(f.item.snapshot!)), true));
const policyDeps = (f: Fixture) => ({
  reviews: f.reviews,
  projects: { resolveBranch: (_p: string, ref: string) => Effect.succeed(ref) },
  audit: { append: (a: AuditInput) => void f.audited.push(a) } as never,
  transport: "socket",
});

// ---------------------------------------------------------------------------------------------

/** Every string `from` in `v` replaced by `to`, deeply. */
function substitute(v: unknown, map: Readonly<Record<string, string>>): unknown {
  if (typeof v === "string") return map[v] ?? v;
  if (Array.isArray(v)) return v.map((x) => substitute(x, map));
  if (v !== null && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, substitute(x, map)]));
  return v;
}

/**
 * The most permissive payload of `tag` the schemas allow for this item: the sample with the real
 * project, item and branch, and every optional switch of the review RPCs on.
 */
function permissive(tag: string, ids: { projectId: string; itemId: string; branch: string; snapshot: Snapshot; origins: ReadonlyArray<string> }): Record<string, unknown> {
  const base = substitute(SAMPLE_PAYLOADS[tag as FinRpcTag] ?? {}, { "01aaaaaaaaaaaaaaaaaaaaaaaa": ids.projectId, "01eeeeeeeeeeeeeeeeeeeeeeee": ids.itemId, try: ids.branch }) as Record<string, unknown>;
  switch (tag) {
    case "review.decide":
      return { ...base, expect: ids.snapshot, decisions: ids.origins.map((txId) => ({ txId, decision: "accept" })) };
    case "review.merge":
      return { ...base, expect: ids.snapshot, acceptPending: true, idemKey: `k-${Math.random()}` };
    case "review.policy":
      return { ...base, protected: [], requireReview: false, autoMerge: { enabled: true } };
    case "review.request":
      return { ...base, into: "main", summary: "s" };
    default:
      return base;
  }
}

const KINDS: ReadonlyArray<PrincipalKind> = ["anonymous", "local", "console", "device", "agent", "mcp", "plugin"];

/** Every `.ts` file under `src/` (path relative to `src/`, text). */
function sources(): ReadonlyArray<[string, string]> {
  const src = join(dirname(fileURLToPath(import.meta.url)), "..", "src");
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

const AUTO_MERGE = "reviews/auto-merge.ts";
/** A static or literal dynamic import of the module, `.ts`, `.js` or bare. */
const IMPORTS_AUTO_MERGE = /(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)["'`][^"'`\n]*auto-merge(?:\.[cm]?[jt]s)?["'`]/;
/** A dynamic `import()` whose specifier is not a literal: it could name any module. */
const COMPUTED_IMPORT = /\bimport\(\s*(?!["'`])/;
/** Computed imports that exist on purpose: catalog extension modules by their URL (5D). */
const COMPUTED_IMPORT_SITES = ["library/catalog/extensions.ts"];
/** What builds a policy `Caller`: the `internal` actor in an object, `policyCaller`, the `policy` principal. */
const POLICY_CALLER = [/[{,]\s*internal\s*:\s*[A-Za-z_{]/, /\bpolicyCaller\b/, /\bkind\s*:\s*"policy"/];
/** Where the policy actor and principal are declared as types (`readonly kind: "policy";`), nothing more. */
const POLICY_TYPE_DECLARATIONS = ["diagnostics.ts", "reviews/workflow.ts"];

describe("no_rpc_reaches_auto_merge", () => {
  test("no source module but the path itself imports it", () => {
    const files = sources();
    expect(files.filter(([p, text]) => p !== AUTO_MERGE && IMPORTS_AUTO_MERGE.test(text)).map(([p]) => p)).toEqual([]);
    // No computed import could reach it either.
    expect(files.filter(([, text]) => text.split("\n").some((l) => COMPUTED_IMPORT.test(l) && !l.trimStart().startsWith("//"))).map(([p]) => p)).toEqual(COMPUTED_IMPORT_SITES);
    // The checks catch what they claim to.
    for (const spec of ['"./auto-merge.ts"', '"./auto-merge.js"', "'../reviews/auto-merge'"]) {
      expect(IMPORTS_AUTO_MERGE.test(`import { x } from ${spec};`)).toBe(true);
      expect(IMPORTS_AUTO_MERGE.test(`const m = await import(${spec});`)).toBe(true);
      expect(IMPORTS_AUTO_MERGE.test(`import ${spec};`)).toBe(true);
    }
    expect(IMPORTS_AUTO_MERGE.test("// the auto-merge path (`auto-merge.ts`) merges")).toBe(false);
    expect(COMPUTED_IMPORT.test("await import(name)")).toBe(true);
    expect(COMPUTED_IMPORT.test('await import("node:fs")')).toBe(false);
  });

  test("only the path builds a policy caller", () => {
    const offenders = sources().flatMap(([p, text]) => {
      if (p === AUTO_MERGE) return [];
      const lines = text.split("\n").filter((l) => !(POLICY_TYPE_DECLARATIONS.includes(p) && /^\s*readonly kind: "policy";\s*$/.test(l)));
      return lines.filter((l) => POLICY_CALLER.some((re) => re.test(l))).map((l) => `${p}: ${l.trim()}`);
    });
    expect(offenders).toEqual([]);
    // The patterns match the forms that would build one.
    expect(POLICY_CALLER.some((re) => re.test("const c = { principal: p, internal: actor };"))).toBe(true);
    expect(POLICY_CALLER.some((re) => re.test('const p = { kind: "policy", id: "autoMerge" };'))).toBe(true);
    expect(POLICY_CALLER.some((re) => re.test("const c = policyCaller(a, g, s);"))).toBe(true);
  });

  test("every review and write RPC, through the chain, for every principal kind with every scope", async () => {
    const f = await fixture(["f1", "f2", "f3"]);
    enable(f);
    const chainProjects: ChainProjects = {
      isOpen: () => true,
      dirOf: () => join(tmp, "p"),
      resolveBranch: (_p, ref) => json(() => f.e.resolveBranch(ref)),
      branches: () => json<BranchView[]>(() => f.e.branches(true)),
    };
    const principals = Object.fromEntries(
      KINDS.map((kind, i) => [i + 1, { kind, id: kind === "local" || kind === "console" ? kind : `01${kind.padEnd(24, "0")}`.slice(0, 26), scopes: [...SCOPES], projects: "*", branches: "*", human: true } satisfies Principal]),
    );
    const h = chainHarness(principals, { projects: chainProjects, reviews: f.reviews.chain, gate: f.reviews.gate }, ({ audit }) =>
      reviewHandlers({ guard: (_t, self) => self, reviews: f.reviews, projects: chainProjects, audit, transport: "socket" }),
    );
    const tags = [...new Set([...WRITE_RPCS, ...CAPABILITIES.filter((t) => t.startsWith("review."))])].filter((t) => !STREAM_RPCS.includes(t));
    let reached = 0;
    for (const client of Object.keys(principals).map(Number)) {
      for (const tag of tags) {
        const item = f.reviews.store.get(f.item.id)!;
        const payload = permissive(tag, { projectId: A, itemId: item.id, branch: "feature", snapshot: item.snapshot!, origins: ["f1", "f2", "f3"] });
        if (tag === "review.policy") payload.challenge = h.challenge(client);
        const exit = await h.call(client, tag, payload);
        if (exit._tag === "Success" && tag.startsWith("review.")) reached++;
      }
    }
    // The review handlers ran (a person merged the item by hand), yet nothing reached the path.
    expect(reached).toBeGreaterThan(0);
    expect(f.reviews.store.get(f.item.id)?.state).toBe("merged");
    expect(f.audited.filter((a) => a.kind === "review.merge").every((a) => a.details?.auto === false)).toBe(true);
    expect(f.entries.every((e) => e.by !== "policy" && e.via === undefined)).toBe(true);
    expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
  });

  test("every RPC tag on a real server, as local (the widest reach)", { timeout: 180_000 }, async () => {
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    let ready!: (i: ServerInfo) => void;
    const started = new Promise<ServerInfo>((r) => (ready = r));
    const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
    const info = await started;
    const clients: NdjsonClient[] = [];
    try {
      const local = await connectProved({ info, serverData }, "local");
      clients.push(local);
      // A project, an agent's branch and an open review of it.
      const { projectId } = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
      ok(await commit(local, projectId, tx("m0", 0, [asset("m0")])));
      const made = ok<{ branch: BranchView }>(await local.call("branch.create", { projectId, txId: "b1", author: AGENT_AUTHOR, name: "feature", from: "main" }));
      const headOf = async (id: string) => ok<{ branches: BranchView[] }>(await local.call("branch.list", { projectId, all: true })).branches.find((b) => b.id === id)!.head;
      for (const t of ["f1", "f2"]) ok(await commit(local, projectId, { ...tx(t, await headOf(made.branch.id), [asset(t)]), branch: made.branch.id, author: AGENT_AUTHOR }));
      const { item } = ok<{ item: ReviewItem }>(await local.call("review.request", { projectId, source: "feature", title: "Please look" }));
      const callers: ReadonlyArray<[string, NdjsonClient]> = [["local", local]];
      // Closing the item would leave `review.merge` nothing to merge: it goes after the merge.
      const LAST = new Set(["review.close", "server.stop", "auth.revokeSelf"]);
      const tags = CAPABILITIES.filter((t) => !LAST.has(t));
      const sweep = async (c: NdjsonClient, tag: string) => {
        const now = await Promise.race([local.call("review.get", { itemId: item.id }), new Promise<undefined>((r) => setTimeout(r, 2_000))]);
        const snapshot = now?.ok === true ? (now.value as { item: ReviewItem }).item.snapshot! : item.snapshot!;
        const payload = permissive(tag, { projectId, itemId: item.id, branch: "feature", snapshot, origins: ["f1", "f2"] });
        if (STREAM_RPCS.includes(tag)) {
          const s = c.stream(tag, payload, () => undefined);
          await Promise.race([s.done, tick()]);
          c.interrupt(s.id);
          return;
        }
          await Promise.race([c.call(tag, payload), new Promise((r) => setTimeout(r, 2_000))]);
      };
      for (const [, c] of callers) for (const tag of tags) await sweep(c, tag);
      // The sweep reached the review handlers: `local` merged the item by hand.
      expect(ok<{ item: ReviewItem }>(await local.call("review.get", { itemId: item.id })).item.state).toBe("merged");
      for (const [, c] of callers) for (const tag of LAST) await sweep(c, tag).catch(() => undefined);
      expect(vi.mocked(tryAutoMerge)).not.toHaveBeenCalled();
    } finally {
      // `server.stop` from `local` ended the server: closing what is left must not wait on it.
      const within = (p: Promise<unknown>) => Promise.race([p.catch(() => undefined), new Promise((r) => setTimeout(r, 5_000))]);
      await within(Promise.all(clients.map((c) => c.close())));
      await within(Effect.runPromise(Fiber.interrupt(fiber)));
    }
  });
});
