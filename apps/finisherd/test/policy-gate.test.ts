// The per-project policy and its gate (5E plan Task 6, Decisions 18–19): `review.policy` is
// human-only and confirmed, resolves protected references to ids, is audited with ids and
// booleans only, refuses auto merge while it is unavailable, and is serialised with the
// project's in-flight writes (every write holds the gate shared from the policy stage to its
// answer; a policy write holds it exclusive and waits at most 30 s: `POLICY_BUSY`).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { auditDetailsValid, DEFAULT_BRANCH_POLICY, type FinError, HUMAN_ONLY, type Principal, SCOPES, type Scope } from "@finisher/contracts";
import { Effect, Exit, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { PolicyGate, POLICY_WAIT_MS } from "../src/reviews/gate.ts";
import { AUTO_MERGE_AVAILABLE, policyHandler } from "../src/reviews/policy.ts";
import { makeReviews, type Reviews } from "../src/reviews/service.ts";
import { chainHarness, codeOf, errorOf } from "./support/chain-harness.ts";
import { addAsset, commit, confirmed, connectProved, ok, type NdjsonClient, type RunningServer, startServer, tx } from "./support/ndjson-client.ts";

const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const B = "01bbbbbbbbbbbbbbbbbbbbbbbb";
const WIDE: Scope[] = SCOPES.filter((s) => !HUMAN_ONLY.has(s) && s !== "branch.protected" && s !== "review.decide");
const AGENT: Principal = { kind: "agent", id: "01agent000000000000000000a", scopes: WIDE, projects: [A, B], branches: "*", human: false };

const BRANCHES: Record<string, string> = { main: "main", feature: "br3", br3: "br3", release: "br4", br4: "br4" };
const projects: ChainProjects = {
  isOpen: () => true,
  dirOf: () => "/p",
  resolveBranch: (_p, ref) => {
    const id = BRANCHES[ref];
    return id === undefined ? Effect.fail({ code: "BRANCH_NOT_FOUND", message: ref, details: {} } as unknown as FinError) : Effect.succeed(id);
  },
  branches: () => Effect.succeed([{ id: "main", createdBy: "user" }]),
};

let dir: string;
let control: ControlStore;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "policy-gate-"));
  control = openControlStore(dir, { serverId: "sid-1", checkpoint: false });
});
afterEach(async () => {
  await control.close();
  rmSync(dir, { recursive: true, force: true });
});

/** A latch: `wait` resolves once `open` was called. */
function latch() {
  let open!: () => void;
  const p = new Promise<void>((r) => (open = r));
  return { open, wait: Effect.promise(() => p) };
}
const tick = () => new Promise((r) => setTimeout(r, 20));

describe("the gate", () => {
  test("policy_waits_for_in_flight_write", async () => {
    const gate = new PolicyGate();
    const order: string[] = [];
    const held = latch();
    const write = Effect.runPromise(gate.shared(A, Effect.map(held.wait, () => void order.push("write"))));
    await tick();
    const policy = Effect.runPromise(gate.exclusive(A, Effect.sync(() => void order.push("policy"))));
    // Another project is not held up.
    await Effect.runPromise(gate.exclusive(B, Effect.sync(() => void order.push("other"))));
    await tick();
    expect(order).toEqual(["other"]);
    held.open();
    await Promise.all([write, policy]);
    expect(order).toEqual(["other", "write", "policy"]);
  });

  test("a write that starts while a policy write waits goes after it", async () => {
    const gate = new PolicyGate();
    const order: string[] = [];
    const held = latch();
    const first = Effect.runPromise(gate.shared(A, Effect.map(held.wait, () => void order.push("first"))));
    await tick();
    const policy = Effect.runPromise(gate.exclusive(A, Effect.sync(() => void order.push("policy"))));
    await tick();
    const second = Effect.runPromise(gate.shared(A, Effect.sync(() => void order.push("second"))));
    await tick();
    expect(order).toEqual([]);
    held.open();
    await Promise.all([first, policy, second]);
    expect(order).toEqual(["first", "policy", "second"]);
  });

  test("policy_busy_after_30s", async () => {
    expect(POLICY_WAIT_MS).toBe(30_000);
    const gate = new PolicyGate({ waitMs: 50 });
    const held = latch();
    const write = Effect.runPromise(gate.shared(A, held.wait));
    await tick();
    const ran: string[] = [];
    const busy = Effect.runPromiseExit(gate.exclusive(A, Effect.sync(() => void ran.push("policy"))));
    await tick();
    // A write queued behind the policy write runs once that gives up.
    const queued = Effect.runPromise(gate.shared(A, Effect.sync(() => void ran.push("queued"))));
    const exit = await busy;
    expect(Exit.isFailure(exit) && (Exit.findErrorOption(exit) as { value?: FinError }).value).toMatchObject({ code: "POLICY_BUSY", details: { projectId: A, waitedMs: 50 } });
    await queued;
    expect(ran).toEqual(["queued"]);
    held.open();
    await write;
    // Free again: an exclusive hold is taken at once.
    await Effect.runPromise(gate.exclusive(A, Effect.sync(() => void ran.push("policy"))));
    expect(ran).toEqual(["queued", "policy"]);
  });

  /** Whether `p` settles within `ms`. */
  const within = (p: Promise<unknown>, ms = 500) => Promise.race([p.then(() => true), new Promise((r) => setTimeout(() => r(false), ms))]);

  test("a queued shared waiter behind an exclusive one is interrupted at once (Task 6 review, S2)", async () => {
    const gate = new PolicyGate();
    const held = latch();
    const first = Effect.runPromise(gate.shared(A, held.wait));
    await tick();
    const ran: string[] = [];
    const policy = Effect.runPromise(gate.exclusive(A, Effect.sync(() => void ran.push("policy"))));
    await tick();
    const queued = Effect.runFork(gate.shared(A, Effect.sync(() => void ran.push("queued"))));
    await tick();
    // The hold it waits behind is still taken: the interruption does not wait for it.
    expect(await within(Effect.runPromise(Fiber.interrupt(queued)))).toBe(true);
    held.open();
    await Promise.all([first, policy]);
    expect(ran).toEqual(["policy"]);
    await Effect.runPromise(gate.exclusive(A, Effect.void));
  });

  test("a queued exclusive waiter is interrupted at once and lets the writes behind it go", async () => {
    const gate = new PolicyGate();
    const held = latch();
    const first = Effect.runPromise(gate.shared(A, held.wait));
    await tick();
    const ran: string[] = [];
    const policy = Effect.runFork(gate.exclusive(A, Effect.sync(() => void ran.push("policy"))));
    await tick();
    const behind = Effect.runPromise(gate.shared(A, Effect.sync(() => void ran.push("behind"))));
    await tick();
    expect(ran).toEqual([]);
    expect(await within(Effect.runPromise(Fiber.interrupt(policy)))).toBe(true);
    await behind;
    expect(ran).toEqual(["behind"]);
    held.open();
    await first;
    await Effect.runPromise(gate.exclusive(A, Effect.void));
  });

  test("a write queued in the chain behind a policy write is interrupted at once", async () => {
    const gate = new PolicyGate();
    const held = latch();
    const reached: string[] = [];
    const h = chainHarness({ 3: AGENT }, { projects, reviews: { item: () => undefined, protectedBranches: () => [] }, gate }, () => ({
      "tx.commit": (p: { tx: { txId: string } }) => Effect.map(p.tx.txId === "first" ? held.wait : Effect.void, () => (reached.push(p.tx.txId), { ok: true })),
    }));
    const write = (txId: string) => ({ projectId: A, tx: { txId, branch: "feature", author: "a", ops: [] } });
    const first = h.call(3, "tx.commit", write("first"));
    await tick();
    const policy = Effect.runPromise(gate.exclusive(A, Effect.void));
    await tick();
    const queued = h.fork(3, "tx.commit", write("queued"));
    await tick();
    expect(await within(Effect.runPromise(Fiber.interrupt(queued)))).toBe(true);
    held.open();
    expect(codeOf(await first)).toBeUndefined();
    await policy;
    expect(reached).toEqual(["first"]);
    // Nothing is left held: a policy write gets the gate at once.
    await Effect.runPromise(gate.exclusive(A, Effect.void, 50));
  });

  test("a failed or interrupted hold is given back", async () => {
    const gate = new PolicyGate({ waitMs: 50 });
    expect(Exit.isFailure(await Effect.runPromiseExit(gate.shared(A, Effect.fail("boom"))))).toBe(true);
    expect(Exit.isFailure(await Effect.runPromiseExit(gate.exclusive(A, Effect.die("boom"))))).toBe(true);
    const held = latch();
    const fiber = Effect.runFork(gate.shared(A, held.wait));
    await tick();
    await Effect.runPromise(Fiber.interrupt(fiber));
    await Effect.runPromise(gate.exclusive(A, Effect.void));
  });
});

describe("review.policy", () => {
  /** A chain with the real `review.policy` handler; client 1 is `principal`, 2 the console, 3 an agent. */
  function setup(principal: Principal = CONSOLE, gate?: PolicyGate) {
    const reviews: Reviews = gate === undefined ? makeReviews(control) : { ...makeReviews(control), gate };
    const h = chainHarness({ 1: principal, 2: CONSOLE, 3: AGENT }, { projects, reviews: reviews.chain, gate: reviews.gate }, ({ audit }) => ({
      "review.policy": policyHandler({ reviews, projects, audit, transport: "socket" }),
    }));
    const set = (client: number, body: Record<string, unknown>) => h.call(client, "review.policy", { projectId: A, ...body, challenge: h.challenge(client) });
    return { h, reviews, set };
  }
  const commitTo = (branch: string) => ({ projectId: A, tx: { txId: "t", branch, author: "a", ops: [] } });

  test("policy_write_is_human_only_and_confirmed", async () => {
    for (const p of [AGENT, LOCAL, { ...AGENT, scopes: SCOPES }]) {
      const { h } = setup(p);
      expect(codeOf(await h.call(1, "review.policy", { projectId: A, requireReview: true })), p.kind).toBe("HUMAN_ONLY");
    }
    const { h, reviews, set } = setup();
    expect(codeOf(await h.call(1, "review.policy", { projectId: A, requireReview: true }))).toBe("CONFIRMATION_REQUIRED");
    expect(reviews.store.policy(A)).toEqual(DEFAULT_BRANCH_POLICY);
    const exit = await set(1, { requireReview: true });
    expect(Exit.isSuccess(exit) && exit.value).toEqual({ policy: { ...DEFAULT_BRANCH_POLICY, requireReview: true }, old: DEFAULT_BRANCH_POLICY });
    expect(reviews.store.policy(A).requireReview).toBe(true);
  });

  test("policy_audited_old_and_new", async () => {
    const { h, reviews, set } = setup();
    expect(Exit.isSuccess(await set(1, { protected: ["main", "release", "br4"], requireReview: true }))).toBe(true);
    // References are stored as ids, once each; what is not sent is kept.
    expect(reviews.store.policy(A)).toEqual({ protected: ["main", "br4"], requireReview: true, autoMerge: { enabled: false } });
    expect(Exit.isSuccess(await set(1, { protected: ["feature"] }))).toBe(true);
    const rows = h.audited.filter((e) => e.kind === "review.policy");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({
      principal: { kind: "console", id: "console" },
      outcome: "ok",
      details: {
        projectId: A,
        transport: "socket",
        old: { protected: ["main"], requireReview: false, autoMerge: false },
        new: { protected: ["main", "br4"], requireReview: true, autoMerge: false },
      },
    });
    expect(rows[1]!.details).toMatchObject({ old: { protected: ["main", "br4"] }, new: { protected: ["br3"], requireReview: true } });
    for (const r of rows) expect(auditDetailsValid("review.policy", r.details ?? {})).toBe(true);
    // Branch names are never in it.
    expect(JSON.stringify(rows)).not.toMatch(/release|feature/);
    // An unknown branch changes nothing and is not audited.
    expect(codeOf(await set(1, { protected: ["nope"] }))).toBe("BRANCH_NOT_FOUND");
    expect(reviews.store.policy(A).protected).toEqual(["br3"]);
    expect(h.audited.filter((e) => e.kind === "review.policy")).toHaveLength(2);
  });

  test("protect_and_unprotect_apply_to_the_current_list", async () => {
    const { reviews, set } = setup();
    // Resolved to ids and added to the list as it is: another write's branch is not lost.
    expect(Exit.isSuccess(await set(1, { protect: ["release"] }))).toBe(true);
    expect(reviews.store.policy(A).protected).toEqual(["main", "br4"]);
    expect(Exit.isSuccess(await set(1, { protect: ["feature"] }))).toBe(true);
    expect(reviews.store.policy(A).protected).toEqual(["main", "br4", "br3"]);
    // Unprotect by name takes the stored id off; a branch not on the list is a no-op.
    expect(Exit.isSuccess(await set(1, { unprotect: ["release", "feature"] }))).toBe(true);
    expect(reviews.store.policy(A).protected).toEqual(["main"]);
    // Exclusive with the whole list, and with itself; unknown changes nothing.
    const full = await set(1, { protected: ["main"], protect: ["release"] });
    expect(codeOf(full)).toBe("INVALID_REQUEST");
    expect(codeOf(await set(1, { protect: ["release"], unprotect: ["br4"] }))).toBe("INVALID_REQUEST");
    expect(codeOf(await set(1, { unprotect: ["nope"] }))).toBe("BRANCH_NOT_FOUND");
    expect(reviews.store.policy(A).protected).toEqual(["main"]);
  });

  test("auto_merge_on_is_unavailable", async () => {
    expect(AUTO_MERGE_AVAILABLE).toBe(false);
    const { h, reviews, set } = setup();
    expect(errorOf(await set(1, { autoMerge: { enabled: true }, requireReview: true }))).toMatchObject({ code: "AUTO_MERGE_UNAVAILABLE" });
    expect(reviews.store.policy(A)).toEqual(DEFAULT_BRANCH_POLICY);
    expect(h.audited.filter((e) => e.kind === "review.policy")).toEqual([]);
    // Off stays allowed.
    expect(Exit.isSuccess(await set(1, { autoMerge: { enabled: false } }))).toBe(true);
  });

  test("write_after_policy_change_sees_new_policy", async () => {
    const reviews = makeReviews(control);
    const held = latch();
    const order: string[] = [];
    const h = chainHarness({ 2: CONSOLE, 3: AGENT }, { projects, reviews: reviews.chain, gate: reviews.gate }, ({ audit }) => ({
      "review.policy": policyHandler({ reviews, projects, audit, transport: "socket" }),
      "tx.commit": (p: { tx: { txId: string } }) => Effect.map(p.tx.txId === "first" ? held.wait : Effect.void, () => (order.push(p.tx.txId), { ok: true })),
    }));
    // A write into `feature` is in flight under the old policy...
    const first = h.call(3, "tx.commit", { ...commitTo("feature"), tx: { txId: "first", branch: "feature", author: "a", ops: [] } });
    await tick();
    // ...the policy write that protects `feature` waits for it...
    const policy = h.call(2, "review.policy", { projectId: A, protected: ["main", "feature"], challenge: h.challenge(2) }).then((x) => (order.push("policy"), x));
    await tick();
    // ...and a write that starts meanwhile is checked under the new policy.
    const second = h.call(3, "tx.commit", { ...commitTo("feature"), tx: { txId: "second", branch: "feature", author: "a", ops: [] } });
    await tick();
    expect(order).toEqual([]);
    held.open();
    expect(codeOf(await first)).toBeUndefined();
    expect(codeOf(await policy)).toBeUndefined();
    expect(codeOf(await second)).toBe("BRANCH_PROTECTED");
    expect(order).toEqual(["first", "policy"]);
    // Another project's writes never waited.
    expect(codeOf(await h.call(3, "tx.commit", { ...commitTo("feature"), projectId: B }))).toBeUndefined();
  });

  test("a policy write that cannot get the gate is POLICY_BUSY and changes nothing", async () => {
    const gate = new PolicyGate({ waitMs: 50 });
    const held = latch();
    const reviews: Reviews = { ...makeReviews(control), gate };
    const h = chainHarness({ 2: CONSOLE, 3: AGENT }, { projects, reviews: reviews.chain, gate }, ({ audit }) => ({
      "review.policy": policyHandler({ reviews, projects, audit, transport: "socket" }),
      "tx.commit": () => Effect.as(held.wait, { ok: true }),
    }));
    const write = h.call(3, "tx.commit", commitTo("feature"));
    await tick();
    expect(errorOf(await h.call(2, "review.policy", { projectId: A, requireReview: true, challenge: h.challenge(2) }))).toMatchObject({ code: "POLICY_BUSY" });
    expect(reviews.store.policy(A)).toEqual(DEFAULT_BRANCH_POLICY);
    held.open();
    expect(codeOf(await write)).toBeUndefined();
  });
});

describe("through the server", () => {
  let tmp: string;
  let server: RunningServer | undefined;
  const clients: NdjsonClient[] = [];
  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-policy-"));
  });
  afterEach(async () => {
    await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
    await server?.stop();
    server = undefined;
    rmSync(tmp, { recursive: true, force: true });
  });

  test("the console sets the policy, local reads it and still writes the protected branch", async () => {
    server = await startServer(join(tmp, "sd"));
    const local = await connectProved(server, "local");
    const console = await connectProved(server, "console");
    clients.push(local, console);
    const { projectId } = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
    ok(await local.call("branch.create", { projectId, txId: "b1", author: "ann", name: "release", from: "main" }));
    expect(ok<{ policy: unknown }>(await local.call("review.policy.get", { projectId })).policy).toEqual(DEFAULT_BRANCH_POLICY);
    expect((await local.call("review.policy", { projectId, protected: ["release"] })).ok).toBe(false);
    const set = ok<{ policy: { protected: string[] } }>(await confirmed(console, "review.policy", { projectId, protected: ["main", "release"] }));
    expect(set.policy.protected).toEqual(["main", "br1"]);
    expect(ok<{ policy: unknown }>(await local.call("review.policy.get", { projectId })).policy).toEqual(set.policy);
    // `local` keeps `branch.protected` (owner decision 5).
    ok(await commit(local, projectId, { ...tx("t0", 0, [addAsset()]), branch: "release" }));
    const rows = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await console.call("auth.audit.list", { kind: "review.policy" }));
    expect(rows.entries).toHaveLength(1);
    expect(rows.entries[0]!.details).toMatchObject({ old: { protected: ["main"] }, new: { protected: ["main", "br1"], requireReview: false, autoMerge: false } });
  });
});
