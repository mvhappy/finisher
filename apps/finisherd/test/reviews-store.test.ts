// The reviews store, hub and resolver (5E plan Task 5, Decisions 8–10, 17, 23, 26).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  DEFAULT_BRANCH_POLICY,
  type FinError,
  type Principal,
  REVIEW_COMMENT_AT_MAX,
  REVIEW_COMMENTS_MAX,
  ReviewCommentPayload,
  ReviewRequestPayload,
  REVIEW_LIMIT,
  type ReviewItem,
  requiredScopes,
  ruleOf,
  type Snapshot,
} from "@finisher/contracts";
import { Effect, Exit, Fiber, Schema, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type ChainProjects, type ChainReviews, resolve } from "../src/chain/resolve.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { ReviewStore } from "../src/reviews/store.ts";
import { ReviewHub } from "../src/reviews/hub.ts";
import { makeReviews } from "../src/reviews/service.ts";
import { LOCAL } from "../src/principal.ts";

let dir: string;
const stores: ControlStore[] = [];
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "reviews-"));
});
afterEach(async () => {
  await Promise.all(stores.splice(0).map((s) => s.close()));
  rmSync(dir, { recursive: true, force: true });
});
const control = (): ControlStore => {
  const s = openControlStore(dir, { serverId: "sid-1", checkpoint: false });
  stores.push(s);
  return s;
};

const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const snap = (n: number): Snapshot => ({ sourceSeq: n, sourceHash: `h${n}`, targetSeq: 2, targetHash: "t2", plannedAt: 1000 });
const by = { principal: "agent:a1", actor: { kind: "agent" } };
const branchReview = (extra: object = {}) => ({ projectId: P, kind: "branchReview" as const, source: "br1", target: "main", snapshot: snap(5), title: "Try 1", requestedBy: by, ...extra });
const code = (f: () => unknown): FinError => {
  try {
    f();
  } catch (e) {
    return e as FinError;
  }
  throw new Error("expected a failure");
};

describe("the reviews chain", () => {
  test("reviews_chain_migrates_once", async () => {
    const s = control();
    const n = () => s.db.prepare("SELECT COUNT(*) AS n FROM migrations WHERE chain = 'reviews'").get() as { n: number };
    expect(n().n).toBe(2);
    const meta = (k: string) => (s.db.prepare("SELECT value FROM meta WHERE key = ?").get(k) as { value: string }).value;
    // 0002 (7B Task 12) adds `review_verdicts.session`; an older reader ignores it.
    expect([meta("formatVersion.reviews"), meta("minReaderVersion.reviews")]).toEqual(["2", "1"]);
    const columns = (s.db.prepare("SELECT name FROM pragma_table_info('review_verdicts')").all() as Array<{ name: string }>).map((c) => c.name);
    expect(columns).toContain("session");
    const tables = (s.db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all() as Array<{ name: string }>).map((t) => t.name);
    for (const t of ["reviews", "review_decisions", "review_events", "review_comments", "branch_policy", "review_verdicts"]) expect(tables).toContain(t);
    new ReviewStore(s).open(branchReview());
    await s.close();
    stores.pop();
    const again = control();
    expect((again.db.prepare("SELECT COUNT(*) AS n FROM migrations WHERE chain = 'reviews'").get() as { n: number }).n).toBe(2);
    expect(new ReviewStore(again).list(P)).toHaveLength(1);
  });

  test("one_open_review_per_pair", () => {
    const store = new ReviewStore(control());
    const a = store.open(branchReview());
    expect(store.findOpen(P, "branchReview", "br1", "main")?.id).toBe(a.id);
    // Asked again: the open item, title and summary unchanged.
    const again = store.open(branchReview({ title: "Other title", summary: "other" }));
    expect(again.id).toBe(a.id);
    expect(again.title).toBe("Try 1");
    expect(store.list(P)).toHaveLength(1);
    // Another pair, another kind, another project each get their own.
    expect(store.open(branchReview({ source: "br2" })).id).not.toBe(a.id);
    expect(store.open(branchReview({ kind: "mergeConflict", title: "Conflict" })).id).not.toBe(a.id);
    expect(store.open(branchReview({ projectId: "01bbbbbbbbbbbbbbbbbbbbbbbb" })).id).not.toBe(a.id);
    // Once closed, a new request opens a new item.
    store.setState(a.id, "dismissed", "console");
    expect(store.findOpen(P, "branchReview", "br1", "main")).toBeUndefined();
    const next = store.open(branchReview());
    expect(next.id).not.toBe(a.id);
    expect(next.state).toBe("open");
  });

  test("the RPC schemas and the store apply one text rule (Task 5 review, N1, N2)", () => {
    const store = new ReviewStore(control());
    const decodes = (schema: Schema.Codec<unknown, unknown>, v: unknown) => Exit.isSuccess(Schema.decodeUnknownExit(schema)(v));
    const request = (title: string, summary?: string) => ({ projectId: P, source: "try", title, ...(summary === undefined ? {} : { summary }) });
    // A title of 200 astral code points (400 UTF-16 units) is accepted by both; 201 by neither.
    const astral = "\u{1F600}".repeat(200);
    expect(decodes(ReviewRequestPayload, request(astral))).toBe(true);
    expect(store.open(branchReview({ title: astral })).title).toBe(astral);
    expect(decodes(ReviewRequestPayload, request(`${astral}x`))).toBe(false);
    expect(code(() => store.open(branchReview({ source: "b2", title: `${astral}x` }))).details).toMatchObject({ reason: "tooLong" });
    // A summary is counted in UTF-8 bytes by both.
    const wide = "é".repeat(4 * 1024 + 1);
    expect(decodes(ReviewRequestPayload, request("t", wide))).toBe(false);
    expect(code(() => store.open(branchReview({ source: "b3", summary: wide }))).details).toMatchObject({ field: "summary", reason: "tooLong" });
    // C1 controls are refused by both.
    expect(decodes(ReviewRequestPayload, request("a\u0085b"))).toBe(false);
    expect(code(() => store.open(branchReview({ source: "b4", title: "a\u0085b" }))).details).toMatchObject({ reason: "control" });
    // A comment's `at` is bounded.
    const at = "1".repeat(REVIEW_COMMENT_AT_MAX + 1);
    expect(decodes(ReviewCommentPayload, { itemId: "i", body: "b", at })).toBe(false);
    // The same rule as the store's: code points, no control characters (Task 6 review, N1).
    expect(decodes(ReviewCommentPayload, { itemId: "i", body: "b", at: "\u{1F600}".repeat(REVIEW_COMMENT_AT_MAX) })).toBe(true);
    expect(decodes(ReviewCommentPayload, { itemId: "i", body: "b", at: "1\u0085" })).toBe(false);
    const item = store.list(P)[0]!;
    expect(code(() => store.comment({ itemId: item.id, principal: "local:local", body: "b", at })).details).toMatchObject({ field: "at", reason: "tooLong" });
  });

  test("limits_and_text_rules", () => {
    const store = new ReviewStore(control());
    const bad = (f: () => unknown) => code(f);
    expect(bad(() => store.open(branchReview({ title: "" }))).code).toBe("INVALID_REQUEST");
    expect(bad(() => store.open(branchReview({ title: "x".repeat(201) }))).details).toMatchObject({ field: "title", reason: "tooLong" });
    expect(bad(() => store.open(branchReview({ title: "tab\there" }))).details).toMatchObject({ field: "title", reason: "control" });
    expect(bad(() => store.open(branchReview({ summary: "x".repeat(8 * 1024 + 1) }))).details).toMatchObject({ field: "summary" });
    // Newlines are allowed; 200 characters are too.
    const item = store.open(branchReview({ title: "é".repeat(200), summary: "line one\nline two" }));
    expect(item.summary).toBe("line one\nline two");
    // An error never carries the text it refused.
    const e = bad(() => store.open(branchReview({ title: "secret\u0007title" })));
    expect(JSON.stringify(e)).not.toContain("secret");
    // Comments: 16 KiB, no control characters, 1,000 an item.
    const c = (body: string) => store.comment({ itemId: item.id, principal: "local:local", body });
    expect(code(() => c("")).details).toMatchObject({ field: "body" });
    expect(code(() => c("y".repeat(16 * 1024 + 1))).details).toMatchObject({ field: "body", reason: "tooLong" });
    expect(code(() => c("a\u0000b")).details).toMatchObject({ reason: "control" });
    for (let i = 0; i < REVIEW_COMMENTS_MAX; i++) c(`comment ${i}`);
    expect(code(() => c("one more")).code).toBe("REVIEW_LIMIT");
    expect(store.comments(item.id)).toHaveLength(REVIEW_COMMENTS_MAX);
    // Reasons: 2 KiB.
    expect(code(() => store.setDecisions(item.id, [{ txId: "t1", decision: "reject", reason: "z".repeat(2049) }], "local:local")).details).toMatchObject({ field: "reason" });
    // 256 open items a project.
    const other = new ReviewStore(control());
    for (let i = 0; i < REVIEW_LIMIT; i++) other.open(branchReview({ source: `b${i}`, projectId: "01cccccccccccccccccccccccc" }));
    const full = code(() => other.open(branchReview({ source: "last", projectId: "01cccccccccccccccccccccccc" })));
    expect(full).toMatchObject({ code: "REVIEW_LIMIT", details: { max: REVIEW_LIMIT } });
    // Closing one makes room.
    other.setState(other.list("01cccccccccccccccccccccccc")[0]!.id, "dismissed", "local:local");
    expect(other.open(branchReview({ source: "last", projectId: "01cccccccccccccccccccccccc" })).state).toBe("open");
  });

  test("suggestions_do_not_count_toward_the_limit", () => {
    const store = new ReviewStore(control());
    const R = "01dddddddddddddddddddddddd";
    for (let i = 0; i < REVIEW_LIMIT; i++) {
      store.open({ projectId: R, kind: "pluginSuggestion", title: `@acme/p${i}`, requestedBy: { principal: `agent:a${i}` }, subject: { pluginId: `@acme/p${i}` } });
    }
    // 256 open suggestions leave room for the 256 reviews, and the next suggestion is still taken.
    for (let i = 0; i < REVIEW_LIMIT; i++) store.open(branchReview({ source: `b${i}`, projectId: R }));
    expect(code(() => store.open(branchReview({ source: "last", projectId: R }))).code).toBe("REVIEW_LIMIT");
    expect(store.open({ projectId: R, kind: "pluginSuggestion", title: "@acme/more", requestedBy: { principal: "agent:z" }, subject: { pluginId: "@acme/more" } }).state).toBe("open");
  });

  test("decisions_keyed_by_origin_survive_snapshot_change", () => {
    const store = new ReviewStore(control());
    const item = store.open(branchReview());
    store.setDecisions(item.id, [
      { txId: "t1", decision: "accept" },
      { txId: "t2", decision: "reject", reason: "no" },
      { txId: "t3", decision: "accept" },
    ], "local:local");
    store.setDecisions(item.id, [{ txId: "t3", decision: "pending" }], "local:local");
    // A refresh moves the snapshot; the marks stay with their origins.
    const refreshed = store.setSnapshot(item.id, snap(9), "agent:a1");
    expect(refreshed.snapshot).toEqual(snap(9));
    const d = store.decisions(item.id);
    expect([...d.keys()]).toEqual(["t1", "t2"]);
    expect(d.get("t2")).toMatchObject({ decision: "reject", reason: "no", decidedBy: "local:local" });
    // Another item has its own marks.
    const other = store.open(branchReview({ source: "br2" }));
    expect(store.decisions(other.id).size).toBe(0);
    // Changing a mark replaces it.
    store.setDecisions(item.id, [{ txId: "t2", decision: "accept" }], "console:console");
    expect(store.decisions(item.id).get("t2")).toMatchObject({ decision: "accept", decidedBy: "console:console" });
    expect(store.events(item.id).map((e) => e.kind)).toEqual(["opened", "decided", "decided", "refreshed", "decided"]);
  });

  test("states_follow_the_workflow_and_a_closed_item_refuses_writes", () => {
    const store = new ReviewStore(control());
    const item = store.open(branchReview());
    expect(store.setState(item.id, "changesRequested", "local:local").state).toBe("changesRequested");
    expect(store.findOpen(P, "branchReview", "br1", "main")?.id).toBe(item.id);
    expect(store.setState(item.id, "open", "agent:a1").state).toBe("open");
    const merged = store.setState(item.id, "merged", "local:local", { mergeId: "m1" });
    expect(merged).toMatchObject({ state: "merged", mergeId: "m1", closedBy: "local:local" });
    expect(merged.closedAt).toBeGreaterThan(0);
    for (const f of [
      () => store.setState(item.id, "open", "x"),
      () => store.setDecisions(item.id, [{ txId: "t1", decision: "accept" }], "x"),
      () => store.comment({ itemId: item.id, principal: "x", body: "late" }),
      () => store.setSnapshot(item.id, snap(1), "x"),
    ]) {
      expect(code(f)).toMatchObject({ code: "REVIEW_CLOSED", details: { state: "merged" } });
    }
    expect(code(() => store.setState("nope", "open", "x")).code).toBe("REVIEW_NOT_FOUND");
    expect(store.list(P, { state: "merged" }).map((i) => i.id)).toEqual([item.id]);
    expect(store.list(P, { state: "open" })).toHaveLength(0);
  });

  test("policy_default_protects_main", () => {
    const store = new ReviewStore(control());
    expect(store.policy(P)).toEqual({ protected: ["main"], requireReview: false, autoMerge: { enabled: false } });
    expect(DEFAULT_BRANCH_POLICY.protected).toEqual(["main"]);
    const next = { protected: ["main", "br9"], requireReview: true, autoMerge: { enabled: false } };
    expect(store.setPolicy(P, next).old).toEqual(DEFAULT_BRANCH_POLICY);
    expect(store.policy(P)).toEqual(next);
    expect(store.setPolicy(P, DEFAULT_BRANCH_POLICY).old).toEqual(next);
    // Another project keeps the default.
    expect(store.policy("01bbbbbbbbbbbbbbbbbbbbbbbb")).toEqual(DEFAULT_BRANCH_POLICY);
  });
  test("auto_merge_roles_round_trip: stored deduplicated, absent means every role, enabledBy kept", () => {
    const s = control();
    const store = new ReviewStore(s);
    store.setPolicy(P, { protected: ["main"], requireReview: false, autoMerge: { enabled: true, roles: ["editor", "editor", "colourist"] } }, "console:console");
    expect(store.policy(P).autoMerge).toEqual({ enabled: true, roles: ["editor", "colourist"] });
    expect(store.autoMergeEnabledBy(P)).toBe("console:console");
    store.setPolicy(P, { protected: ["main"], requireReview: false, autoMerge: { enabled: true } });
    expect(store.policy(P).autoMerge).toEqual({ enabled: true });
    expect(store.autoMergeEnabledBy(P)).toBe("console:console");
  });

  test("verdicts_name_their_session (reviews 0002)", () => {
    const s = control();
    const store = new ReviewStore(s);
    const snapshot = { sourceSeq: 2, targetSeq: 1, plannedAt: 0 };
    const insert = s.db.prepare("INSERT INTO review_verdicts (verdictId, itemId, kind, snapshot, pass, at, session) VALUES (?, ?, ?, ?, ?, ?, ?)");
    insert.run("qa1", "it1", "qa", JSON.stringify(snapshot), 1, 1, null);
    insert.run("cr1", "it1", "critic", JSON.stringify(snapshot), 1, 2, "01critic00000000000000000a");
    expect(store.verdicts("it1")).toEqual([
      { verdictId: "qa1", kind: "qa", snapshot, pass: true, at: 1 },
      { verdictId: "cr1", kind: "critic", snapshot, pass: true, at: 2, session: "01critic00000000000000000a" },
    ]);
  });
});

describe("the hub", () => {
  test("watch_sends_the_items_first_then_each_change_and_lagging_ends_it", async () => {
    const s = control();
    const reviews = makeReviews(s);
    const first = reviews.store.open(branchReview());
    const seen: unknown[] = [];
    const fiber = Effect.runFork(Stream.runForEach(reviews.hub.watch(P, () => reviews.store.list(P)), (e) => Effect.sync(() => void seen.push(e))));
    await vi_until(() => seen.length === 1);
    expect(seen[0]).toMatchObject({ type: "snapshot", items: [{ id: first.id }] });
    reviews.store.comment({ itemId: first.id, principal: "local:local", body: "hello" });
    reviews.store.open(branchReview({ projectId: "01bbbbbbbbbbbbbbbbbbbbbbbb" }));
    await vi_until(() => seen.length === 2);
    expect(seen[1]).toMatchObject({ type: "item", item: { id: first.id } });
    await new Promise((r) => setTimeout(r, 50));
    expect(seen).toHaveLength(2);
    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(reviews.hub.watchers(P)).toBe(0);
    // A watcher that does not read ends with WATCH_LAGGING.
    const hub = new ReviewHub();
    const item = (n: number): ReviewItem => ({ ...first, id: `i${n}` });
    const lag = Effect.runPromise(Effect.flip(Stream.runDrain(Stream.tap(hub.watch(P, () => [], 2), () => Effect.sleep(200)))));
    await new Promise((r) => setTimeout(r, 20));
    for (let n = 0; n < 8; n++) hub.publish(item(n));
    expect(await lag).toMatchObject({ code: "WATCH_LAGGING" });
    expect(hub.watchers(P)).toBe(0);
  });
});

const vi_until = async (f: () => boolean) => {
  const end = Date.now() + 5000;
  while (!f()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 10));
  }
};

describe("the review resolver", () => {
  const agent = (projects: Principal["projects"]): Principal => ({ kind: "agent", id: "a1", scopes: [], projects, branches: "*", human: false });
  const deps = (open: boolean, protectedIds: string[] = ["main"]): { projects: ChainProjects; reviews: (r: ReviewStore) => ChainReviews } => ({
    projects: {
      isOpen: () => open,
      dirOf: () => undefined,
      resolveBranch: (_p, ref) => Effect.succeed(ref === "protected-name" ? "main" : ref),
      branches: () => Effect.succeed([]),
    },
    reviews: (r) => ({ item: (id) => r.get(id), protectedBranches: () => protectedIds }),
  });
  const run = (rule: string, payload: unknown, principal: Principal, d: ReturnType<typeof deps>, store: ReviewStore) =>
    Effect.runPromiseExit(resolve(ruleOf(rule as never), payload, d.projects, { principal, reviews: d.reviews(store) }));

  test("review_resolver_other_kinds_of_another_project_are_not_found", async () => {
    const store = new ReviewStore(control());
    const conflict = store.open(branchReview({ kind: "mergeConflict", title: "Conflict", snapshot: undefined }));
    const suggestion = store.open({ projectId: P, kind: "pluginSuggestion", title: "@acme/pack", requestedBy: by, subject: { pluginId: "@acme/pack" } });
    for (const item of [conflict, suggestion]) {
      for (const rule of ["review.get", "review.decide", "review.merge", "review.close"]) {
        const outside = await run(rule, { itemId: item.id }, agent(["01bbbbbbbbbbbbbbbbbbbbbbbb"]), deps(true), store);
        expect(outside._tag, `${item.kind} ${rule}`).toBe("Failure");
        expect(outside._tag === "Failure" ? JSON.stringify(outside.cause) : "").toContain("REVIEW_NOT_FOUND");
      }
    }
  });

  test("review_resolver_reach", async () => {
    const store = new ReviewStore(control());
    const item = store.open(branchReview());
    const d = deps(true);
    // In reach: the project joins the reach check and the branches join the payload.
    const ok = await run("review.merge", { itemId: item.id }, agent([P]), d, store);
    expect(ok._tag).toBe("Success");
    if (ok._tag === "Success") {
      expect(ok.value.projects).toEqual([P]);
      expect(ok.value.payload).toMatchObject({ itemId: item.id, projectId: P, source: "br1", target: "main" });
      expect(ok.value.facts.reopens).toBeUndefined();
      expect(ok.value.facts.protectedTarget).toBe(true);
    }
    // An item of another project and an unknown id answer alike.
    const outside = await run("review.merge", { itemId: item.id }, agent(["01bbbbbbbbbbbbbbbbbbbbbbbb"]), d, store);
    const unknown = await run("review.merge", { itemId: "nope" }, agent([P]), d, store);
    for (const exit of [outside, unknown]) {
      expect(exit._tag).toBe("Failure");
      const err = exit._tag === "Failure" ? JSON.stringify(exit.cause) : "";
      expect(err).toContain("REVIEW_NOT_FOUND");
    }
    // The two failures are the same but for the id the caller sent.
    const shape = (e: typeof outside) => (e._tag === "Failure" ? JSON.stringify(e.cause).replaceAll(/"itemId":"[^"]*"/g, '"itemId":"_"') : "");
    expect(shape(outside)).toBe(shape(unknown));
    // A project that is not open is reopened by serving it.
    const closed = await run("review.get", { itemId: item.id }, LOCAL, deps(false), store);
    expect(closed._tag === "Success" && closed.value.facts.reopens).toBe(true);
    // A call that needs no review rule leaves the payload alone.
    const none = await run("review.list", { projectId: P }, agent([P]), d, store);
    expect(none._tag === "Success" && none.value.payload).toEqual({ projectId: P });
  });

  test("protected_target_is_read_from_the_policy_for_history_drop", async () => {
    const store = new ReviewStore(control());
    const payload = { projectId: P, txId: "t", author: "a", from: "try", into: "protected-name", drops: ["x"] };
    const fact = async (principal: Principal, protectedIds: string[]) => {
      const exit = await run("history.drop", payload, principal, deps(true, protectedIds), store);
      return exit._tag === "Success" ? exit.value.facts.protectedTarget : "failed";
    };
    expect(await fact(agent([P]), ["main"])).toBe(true);
    expect(await fact(agent([P]), ["other"])).toBe(false);
    // A project outside the caller's reach is not looked at: nothing is resolved for it.
    expect(await fact(agent(["01bbbbbbbbbbbbbbbbbbbbbbbb"]), ["main"])).toBeUndefined();
    // A rule without `protectedNeeds` sets no fact.
    const merge = await run("history.merge", { projectId: P, from: "try", into: "main" }, agent([P]), deps(true), store);
    expect(merge._tag === "Success" && merge.value.facts.protectedTarget).toBeUndefined();
  });

  test("a closed project is not opened to read the protected target (Task 5 review, S1)", async () => {
    const store = new ReviewStore(control());
    let lookups = 0;
    const closed = deps(false, ["br3"]);
    const counting = { ...closed, projects: { ...closed.projects, resolveBranch: (p: string, ref: string) => (lookups++, closed.projects.resolveBranch(p, ref)) } };
    // `history.drop` into a branch other than `main` while the project is closed: no lookup (it
    // would reopen the project before authorisation), the fact stays unset, so the call asks
    // for `project.write` (which a reopen needs too) and the policy stage checks the target.
    const drop = await run("history.drop", { projectId: P, txId: "t", author: "a", from: "try", into: "br3", drops: ["x"] }, agent([P]), counting, store);
    expect(drop._tag === "Success" && drop.value.facts.protectedTarget).toBeUndefined();
    expect(drop._tag === "Success" && drop.value.facts.reopens).toBe(true);
    expect(lookups).toBe(0);
    expect(requiredScopes("history.drop", { projectId: P }, drop._tag === "Success" ? drop.value.facts : {})).toContain("project.write");
    // A review item's target is an id already: compared with the policy, never looked up.
    const item = store.open(branchReview({ target: "br3" }));
    const merge = await run("review.merge", { itemId: item.id }, agent([P]), counting, store);
    expect(merge._tag === "Success" && merge.value.facts.protectedTarget).toBe(true);
    expect(lookups).toBe(0);
  });
});
