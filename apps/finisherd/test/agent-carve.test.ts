// Budget carve-outs for children (sub-project 7B Task 7, Decision 22; spec §13.1; Review Focus 4):
// a child's limits are reserved from its parent's remaining budget in the transaction that
// records the child, return what was not spent at the child's end (any end), a person's raise of
// a child's limit reserves as much on the parent or fails on the parent, and recovery settles
// every carve whose child is gone. The ledger against a real `control.db`; the spawn paths
// against a real server, sessions on the scripted `fake` harness (`FIN_TEST_HOOKS=1`).
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Approval, Session } from "@finisher/contracts";
import fc from "fast-check";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { DEFAULT_BUDGET } from "../src/agent/budget.ts";
import { type Carve, childCarve } from "../src/agent/carve.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import { type LedgerStore, MemoryLedger, utcDay } from "../src/agent/ledger.ts";
import { AgentStore, type NewSession } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));
const fails = (f: () => unknown) => {
  try {
    f();
  } catch (e) {
    return e as { code?: string; details?: Record<string, unknown> };
  }
  throw new Error("expected a failure");
};

// The ledger and the store -------------------------------------------------------------------

describe("the carve in the ledger", () => {
  let dir: string;
  let control: ControlStore;
  let store: AgentStore;
  const clock = manualClock(Date.UTC(2026, 10, 9, 12));
  let n = 0;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "agent-carve-"));
    control = openControlStore(dir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, dir, clock, systemIds(clock));
  });
  afterEach(async () => {
    await control.close();
    rmSync(dir, { recursive: true, force: true });
  });

  const session = (over: Partial<NewSession> = {}): NewSession => {
    n++;
    return {
      id: systemIds(clock).ulid(),
      projectId: "P1",
      harness: "builtin",
      provider: "openrouter",
      model: "anthropic/claude-sonnet-5.5",
      weights: "closed",
      role: "editor",
      mode: "draft",
      rootBranch: `br${n}`,
      base: { branch: "main", seq: 3 },
      budget: {},
      createdBy: { principal: { kind: "local", id: "local" } },
      ...over,
    };
  };

  /** A root with `tokens` and `money` limits, idle. */
  async function rootOf(tokens: number, money: number): Promise<string> {
    const s = await store.createSession(session());
    store.ledger().setLimit(s.id, "tokens", tokens);
    store.ledger().setLimit(s.id, "money", money);
    store.apply(s.id, { kind: "ready" });
    return s.id;
  }

  async function childOf(parent: string, carve: Carve): Promise<string> {
    const s = await store.createSession(session({ parent, rootSession: parent, depth: 1, carve }));
    store.apply(s.id, { kind: "ready" });
    return s.id;
  }

  const row = (id: string, kind: "tokens" | "money") => store.ledger().read(id, kind);
  const carves = () => control.db.prepare("SELECT sessionId, parentId, kind, amount, settled FROM budget_carves ORDER BY sessionId, kind").all();

  test("carve_reserves_on_parent: the parent's reserved and the child's limits, in the transaction that records the child", async () => {
    const p = await rootOf(10_000, 4_000_000);
    const c = await childOf(p, { tokens: 3000, money: 1_000_000 });
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 0, reserved: 3000 });
    expect(row(p, "money")).toEqual({ limit: 4_000_000, used: 0, reserved: 1_000_000 });
    expect(row(c, "tokens")).toEqual({ limit: 3000, used: 0, reserved: 0 });
    expect(row(c, "money")).toEqual({ limit: 1_000_000, used: 0, reserved: 0 });
    // A carve larger than the parent's remaining is BUDGET_EXCEEDED on the parent: no child, nothing held.
    const before = store.list().sessions.length;
    let e: { code?: string; details?: Record<string, unknown> } | undefined;
    await store.createSession(session({ parent: p, rootSession: p, depth: 1, carve: { tokens: 7001 } })).catch((x: unknown) => (e = x as typeof e));
    expect(e?.code).toBe("BUDGET_EXCEEDED");
    expect(e?.details).toMatchObject({ session: p, kind: "tokens", asked: 7001, remaining: 7000 });
    expect(store.list().sessions.length).toBe(before);
    expect(row(p, "tokens").reserved).toBe(3000);
    // A failure later in the same transaction takes the carve back with the child's record.
    control.db.exec("CREATE TEMP TRIGGER no_state BEFORE INSERT ON session_state BEGIN SELECT RAISE(ABORT, 'refused'); END");
    await expect(store.createSession(session({ parent: p, rootSession: p, depth: 1, carve: { tokens: 1000, money: 5 } }))).rejects.toThrow();
    control.db.exec("DROP TRIGGER no_state");
    expect(store.list().sessions.length).toBe(before);
    expect(row(p, "tokens").reserved).toBe(3000);
    expect(row(p, "money").reserved).toBe(1_000_000);
    expect(carves()).toHaveLength(2);
  });

  test("carve_returns_unused: at the child's end the parent's remaining is what it had minus what the child spent, once", async () => {
    const p = await rootOf(10_000, 4_000_000);
    const c = await childOf(p, { tokens: 3000, money: 1_000_000 });
    store.ledger().add(c, "tokens", { used: 700, reserved: 100 });
    store.ledger().add(c, "money", { used: 250_000 });
    store.apply(c, { kind: "ended", reason: "briefDone" });
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 700, reserved: 0 });
    expect(row(p, "money")).toEqual({ limit: 4_000_000, used: 250_000, reserved: 0 });
    // Settled once: another settle and a recovery charge nothing more.
    store.ledger().settleCarve(c);
    store.recoverAfterCrash();
    store.recoverAfterCrash();
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 700, reserved: 0 });
    expect(carves()).toEqual([expect.objectContaining({ sessionId: c, settled: 1 }), expect.objectContaining({ sessionId: c, settled: 1 })]);
  });

  test("a deleted child settles before its rows go; a nested child's held carve counts as its parent's spend", async () => {
    const p = await rootOf(10_000, 0);
    const c = await childOf(p, { tokens: 4000 });
    const g = await store.createSession(session({ parent: c, rootSession: p, depth: 2, carve: { tokens: 1000 } }));
    expect(row(c, "tokens")).toEqual({ limit: 4000, used: 0, reserved: 1000 });
    store.ledger().add(c, "tokens", { used: 500 });
    // The child ends with its own child still live: its parent is charged what it spent and what it still holds.
    await store.delete(c);
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 1500, reserved: 0 });
    // The grandchild's end settles into a parent that is gone: nothing is written for it.
    store.ledger().add(g.id, "tokens", { used: 10 });
    store.apply(g.id, { kind: "ended", reason: "briefDone" });
    expect(row(c, "tokens")).toEqual({ limit: null, used: 0, reserved: 0 });
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 1500, reserved: 0 });
  });

  test("recovery keeps live children's carves held, returns per-call reservations, settles carves whose child is gone", async () => {
    const p = await rootOf(10_000, 4_000_000);
    const live = await childOf(p, { tokens: 1000 });
    const gone = await childOf(p, { tokens: 2000, money: 100 });
    store.ledger().add(gone, "tokens", { used: 300 });
    // A per-call reservation of the parent's, then a crash; and a child whose rows went without its end (no settle).
    store.ledger().add(p, "tokens", { reserved: 50 });
    store.ledger().addDaily(utcDay(clock.now()), { reserved: 50 });
    control.db.exec("PRAGMA foreign_keys = OFF");
    control.db.prepare("DELETE FROM sessions WHERE id = ?").run(gone);
    control.db.exec("PRAGMA foreign_keys = ON");
    control.db.prepare("DELETE FROM budgets WHERE sessionId = ?").run(gone);
    store.recoverAfterCrash();
    // The gone child's spend is unknown: it is charged its whole carve (fail closed).
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 2000, reserved: 1000 });
    expect(row(p, "money")).toEqual({ limit: 4_000_000, used: 100, reserved: 0 });
    expect(store.ledger().daily(utcDay(clock.now())).reserved).toBe(0);
    store.recoverAfterCrash();
    expect(row(p, "tokens")).toEqual({ limit: 10_000, used: 2000, reserved: 1000 });
    expect(store.get(live)!.status).toBe("idle");
    expect(store.ledger().read(live, "tokens").limit).toBe(1000);
  });

  test("raise_child_raises_parent_reserved_or_fails (ledger): both together, or BUDGET_EXCEEDED on the parent and nothing raised", async () => {
    const p = await rootOf(5000, 0);
    const c = await childOf(p, { tokens: 1000 });
    const e = fails(() => store.ledger().raise(c, { tokens: 4001 }));
    expect(e.code).toBe("BUDGET_EXCEEDED");
    expect(e.details).toMatchObject({ session: p, kind: "tokens", asked: 4001, remaining: 4000 });
    expect(store.ledger().canRaise(c, { tokens: 4001 })?.code).toBe("BUDGET_EXCEEDED");
    expect(row(c, "tokens").limit).toBe(1000);
    expect(row(p, "tokens").reserved).toBe(1000);
    expect(store.ledger().canRaise(c, { tokens: 4000, wallMs: 10 })).toBeUndefined();
    store.ledger().raise(c, { tokens: 4000, wallMs: 10 });
    expect(row(c, "tokens").limit).toBe(5000);
    expect(row(p, "tokens")).toEqual({ limit: 5000, used: 0, reserved: 5000 });
    // A root's raise touches nothing else.
    store.ledger().raise(p, { tokens: 1 });
    expect(row(p, "tokens")).toEqual({ limit: 5001, used: 0, reserved: 5000 });
    // The raised carve returns in full.
    store.apply(c, { kind: "ended", reason: "userEnded" });
    expect(row(p, "tokens")).toEqual({ limit: 5001, used: 0, reserved: 0 });
  });

  test("default_carve_is_half_remaining_capped_by_role (ledger): asked kinds as asked; the rest the role's, at most half what is left", async () => {
    const p = await rootOf(10_000, 3_000_001);
    store.ledger().add(p, "tokens", { used: 1000, reserved: 1000 });
    const half = childCarve(store.ledger(), p, {}, undefined, DEFAULT_BUDGET);
    expect(half.carve).toEqual({ tokens: 4000, money: 1_500_000 });
    expect(half.budget).toMatchObject({ tokens: 4000, moneyMicros: 1_500_000 });
    const capped = childCarve(store.ledger(), p, { tokens: 100, moneyMicros: 20 }, undefined, DEFAULT_BUDGET);
    expect(capped.carve).toEqual({ tokens: 100, money: 20 });
    const asked = childCarve(store.ledger(), p, { tokens: 8000, wallMs: 5 }, { tokens: 8000 }, DEFAULT_BUDGET);
    expect(asked.carve).toEqual({ tokens: 8000, money: 1_500_000 });
    // The wall clock is the child's own (not carved).
    expect(asked.budget.wallMs).toBe(5);
    const over = fails(() => childCarve(store.ledger(), p, { tokens: 8001 }, { tokens: 8001 }, DEFAULT_BUDGET));
    expect(over).toMatchObject({ code: "BUDGET_EXCEEDED", details: { session: p, kind: "tokens", asked: 8001, remaining: 8000 } });
    // Nothing left to halve is BUDGET_EXCEEDED on the parent too.
    store.ledger().add(p, "tokens", { used: 7999 });
    expect(fails(() => childCarve(store.ledger(), p, {}, undefined, DEFAULT_BUDGET))).toMatchObject({ code: "BUDGET_EXCEEDED", details: { session: p, kind: "tokens", remaining: 1 } });
  });

  test("the in-memory ledger carves, raises and settles as control.db does", () => {
    const run = (l: LedgerStore) => {
      l.setLimit("p", "tokens", 1000);
      l.setLimit("p", "money", 0);
      l.carve("p", "c", { tokens: 400, money: 0 });
      const before = l.read("p", "tokens");
      const over = l.canRaise("c", { tokens: 601 })?.code;
      l.raise("c", { tokens: 100 });
      l.add("c", "tokens", { used: 250 });
      l.settleCarve("c");
      l.settleCarve("c");
      return { before, over, after: l.read("p", "tokens"), child: l.read("c", "tokens") };
    };
    const db = run(store.ledger());
    expect(run(new MemoryLedger())).toEqual(db);
    expect(db).toEqual({ before: { limit: 1000, used: 0, reserved: 400 }, over: "BUDGET_EXCEEDED", after: { limit: 1000, used: 250, reserved: 0 }, child: { limit: 500, used: 250, reserved: 0 } });
  });

  test("carve_property: over spawn, spend, raise, end, delete and crash, used + reserved ≤ limit for every session, no carve outlives its child after recovery, the day's money is real spend once", async () => {
    type Op =
      | { op: "spawn"; at: number; ask: number | null }
      | { op: "spend" | "reserve" | "raise"; at: number; frac: number }
      | { op: "end" | "delete"; at: number }
      | { op: "crash" };
    const at = fc.nat(7);
    const frac = fc.double({ min: 0, max: 1, noNaN: true });
    const op: fc.Arbitrary<Op> = fc.oneof(
      fc.record({ op: fc.constant("spawn" as const), at, ask: fc.option(frac, { nil: null }) }),
      fc.record({ op: fc.constantFrom("spend" as const, "reserve" as const, "raise" as const), at, frac }),
      fc.record({ op: fc.constantFrom("end" as const, "delete" as const), at }),
      fc.constant({ op: "crash" as const }),
    );
    const day = utcDay(clock.now());
    await fc.assert(
      fc.asyncProperty(fc.integer({ min: 0, max: 50_000 }), fc.integer({ min: 0, max: 5_000_000 }), fc.array(op, { maxLength: 30 }), async (tokens, money, ops) => {
        control.db.exec("DELETE FROM budget_days");
        const l = store.ledger();
        const ids = [await rootOf(tokens, money)];
        const parentOf = new Map<string, string>();
        let spent = 0;
        const alive = (i: number) => {
          const id = ids[i % ids.length]!;
          const s = store.get(id);
          return s !== undefined && s.status !== "ended" ? id : undefined;
        };
        const left = (id: string, kind: "tokens" | "money") => {
          const r = l.read(id, kind);
          return Math.max(0, (r.limit ?? 0) - r.used - r.reserved);
        };
        const check = () => {
          for (const id of ids) {
            for (const kind of ["tokens", "money"] as const) {
              const r = l.read(id, kind);
              if (r.limit !== null) expect(r.used + r.reserved, `${kind} of ${id}`).toBeLessThanOrEqual(r.limit);
            }
          }
          expect(l.daily(day).used).toBe(spent);
        };
        for (const o of ops) {
          if (o.op === "crash") {
            store.recoverAfterCrash();
            const open = control.db.prepare("SELECT sessionId, parentId, kind, amount FROM budget_carves WHERE settled = 0").all() as Array<{ sessionId: string; parentId: string; kind: string; amount: number }>;
            for (const c of open) expect(store.get(c.sessionId)?.status, "an open carve's child is live").not.toBe("ended");
            for (const id of ids) {
              if (store.get(id) === undefined) continue;
              for (const kind of ["tokens", "money"] as const) {
                const held = open.filter((c) => c.parentId === id && c.kind === kind).reduce((a, c) => a + c.amount, 0);
                expect(l.read(id, kind).reserved).toBe(held);
              }
            }
            check();
            continue;
          }
          const id = alive(o.at);
          if (id === undefined) continue;
          if (o.op === "spawn") {
            const asked = o.ask === null ? undefined : { tokens: Math.floor(o.ask * (left(id, "tokens") + 2)) };
            try {
              const { carve } = childCarve(l, id, asked ?? {}, asked, DEFAULT_BUDGET);
              const c = await store.createSession(session({ parent: id, rootSession: ids[0]!, depth: 1, carve }));
              store.apply(c.id, { kind: "ready" });
              ids.push(c.id);
              parentOf.set(c.id, id);
            } catch (e) {
              expect((e as { code?: string }).code).toBe("BUDGET_EXCEEDED");
            }
          } else if (o.op === "spend" || o.op === "reserve") {
            const t = Math.floor(o.frac * left(id, "tokens"));
            const m = Math.floor(o.frac * left(id, "money"));
            if (o.op === "reserve") {
              l.add(id, "tokens", { reserved: t });
            } else {
              l.add(id, "tokens", { used: t });
              l.add(id, "money", { used: m });
              l.addDaily(day, { used: m });
              spent += m;
            }
          } else if (o.op === "raise") {
            const by = Math.floor(o.frac * 2 * (left(parentOf.get(id) ?? id, "tokens") + 1));
            const before = l.read(id, "tokens").limit;
            try {
              l.raise(id, { tokens: by });
            } catch (e) {
              expect((e as { code?: string }).code).toBe("BUDGET_EXCEEDED");
              expect(l.read(id, "tokens").limit).toBe(before);
            }
          } else if (o.op === "end") {
            store.apply(id, { kind: "ended", reason: "userEnded" });
          } else {
            await store.delete(id);
          }
          check();
        }
      }),
      { numRuns: 60 },
    );
  });
});

// Through the server --------------------------------------------------------------------------

describe("carves through spawn", () => {
  let tmp: string;
  let serverData: string;
  let server: AgentServer;
  const hooks = process.env.FIN_TEST_HOOKS;
  let n = 0;
  const id = () => `01C${String(++n).padStart(23, "0")}`;

  const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
  const usage = (input: number, output: number) => ({ type: "usage", input, output, cacheRead: 0, cacheWrite: 0, reasoning: 0 });
  const PARENT: FakeScript = { name: "parent", turns: [[{ emit: [msg("on it")] }, { awaitAnswer: "never" }]] };
  const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };
  /** A brief that spends `input + output` tokens and runs to its end (the child ends itself). */
  const spends = (input: number, output: number): FakeScript => ({ name: "spends", turns: [[{ emit: [usage(input, output), msg("done"), { type: "turnCompleted", stopReason: "endTurn" }] }]] });
  /** A brief that spends past any small carve and goes on. */
  const OVER: FakeScript = { name: "over", turns: [[{ emit: [usage(800, 400), msg("spent")] }, { awaitAnswer: "never" }], [{ emit: [msg("again")] }, { awaitAnswer: "never" }]] };

  beforeAll(() => {
    process.env.FIN_TEST_HOOKS = "1";
  });
  afterAll(() => {
    if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
    else process.env.FIN_TEST_HOOKS = hooks;
  });
  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-carve-"));
    serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "settings.json"), "{}");
  });
  afterEach(async () => {
    await server?.stop();
    rmSync(tmp, { recursive: true, force: true });
  });

  async function boot(scripts: readonly FakeScript[]) {
    server = new AgentServer(serverData, { timers: false, fakeScripts: scripts });
    await server.start();
    const local = await server.local();
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
    return { local, projectId };
  }

  const get = (sessionId: string) => server.core.store.get(sessionId)!;
  const ledger = (sessionId: string, kind: "tokens" | "money" = "tokens") => server.core.store.ledger().read(sessionId, kind);

  async function until<A>(f: () => A | undefined | false, what: string, ms = 8000): Promise<A> {
    const t0 = Date.now();
    for (;;) {
      const v = f();
      if (v !== undefined && v !== false) return v;
      if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
      await new Promise((r) => setTimeout(r, 10));
    }
  }

  async function root(local: NdjsonClient, projectId: string, budget: Record<string, number>, role = "editor") {
    const { session } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go", role, budget }));
    await until(() => get(session.id).status === "running", `${session.id} running`);
    return { session, client: await server.agent(credentialPath(server.core.store.root, session.id)) };
  }

  const spawnRaw = (parent: NdjsonClient, projectId: string, role: string, more: Record<string, unknown> = {}) =>
    parent.call("agent.session.start", { projectId, role, prompt: `brief for a ${role}`, ...more });

  async function spawn(parent: NdjsonClient, projectId: string, role = "fixer", more: Record<string, unknown> = {}) {
    const { session } = ok<{ session: Session }>(await spawnRaw(parent, projectId, role, more));
    const client = await server.agent(credentialPath(server.core.store.root, session.id)).catch(() => undefined);
    return { session, client };
  }

  test("carve_reserves_on_parent (spawn): `--budget` is reserved on the parent; the child's limits and record are the carve", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId, { tokens: 10_000, moneyMicros: 4_000_000 });
    const c = await spawn(p.client, projectId, "fixer", { budget: { tokens: 3000 } });
    expect(c.session.budget).toMatchObject({ tokens: 3000, moneyMicros: 2_000_000 });
    expect(ledger(c.session.id)).toEqual({ limit: 3000, used: 0, reserved: 0 });
    expect(ledger(c.session.id, "money")).toEqual({ limit: 2_000_000, used: 0, reserved: 0 });
    expect(ledger(p.session.id)).toMatchObject({ limit: 10_000, reserved: 3000 });
    expect(ledger(p.session.id, "money")).toMatchObject({ limit: 4_000_000, reserved: 2_000_000 });
    // More than the parent has left is BUDGET_EXCEEDED on the parent, before any branch or record.
    const branchesBefore = ok<{ branches: unknown[] }>(await local.call("branch.list", { projectId, all: true })).branches.length;
    const over = await spawnRaw(p.client, projectId, "fixer", { budget: { tokens: 7001 } });
    expect(code(over)).toBe("BUDGET_EXCEEDED");
    expect(details(over)).toMatchObject({ session: p.session.id, kind: "tokens", asked: 7001, remaining: 7000 });
    expect(server.core.store.tree.children(p.session.id)).toEqual([c.session.id]);
    expect(ok<{ branches: unknown[] }>(await local.call("branch.list", { projectId, all: true })).branches.length).toBe(branchesBefore);
  });

  test("carve_returns_unused (spawn): a brief that ran to its end leaves the parent what it had minus what the child spent", async () => {
    const { local, projectId } = await boot([PARENT, spends(500, 200)]);
    const p = await root(local, projectId, { tokens: 10_000 });
    const c = await spawn(p.client, projectId, "fixer", { budget: { tokens: 3000 } });
    await until(() => get(c.session.id).status === "ended", "the brief's end");
    expect(ledger(c.session.id)).toMatchObject({ limit: 3000, used: 700 });
    expect(ledger(p.session.id)).toEqual({ limit: 10_000, used: 700, reserved: 0 });
  });

  test("default_carve_is_half_remaining_capped_by_role (spawn), nested: a grandchild carves from its parent, which carved from the root", async () => {
    const { local, projectId } = await boot([PARENT, WORKING]);
    const p = await root(local, projectId, { tokens: 10_000 });
    const a = await spawn(p.client, projectId, "clip-finder");
    expect(ledger(a.session.id).limit).toBe(5000);
    const b = await spawn(p.client, projectId, "fixer");
    expect(ledger(b.session.id).limit).toBe(2500);
    expect(ledger(p.session.id)).toMatchObject({ used: 0, reserved: 7500 });
    const g = await spawn(a.client!, projectId, "clip-finder");
    expect(ledger(g.session.id).limit).toBe(2500);
    expect(ledger(a.session.id)).toMatchObject({ limit: 5000, reserved: 2500 });
    expect(ledger(p.session.id)).toMatchObject({ reserved: 7500 });
    // The role's budget caps the default: a root with more than twice it gives a child the role's.
    const big = await root(local, projectId, { tokens: 10 * DEFAULT_BUDGET.tokens });
    expect(ledger((await spawn(big.client, projectId, "fixer")).session.id).limit).toBe(DEFAULT_BUDGET.tokens);
    // A root's end ends every descendant: each carve settles on its parent.
    ok(await local.call("agent.session.end", { sessionId: p.session.id }));
    await until(() => [a, b, g].every((x) => get(x.session.id).status === "ended"), "the descendants' ends");
    expect(ledger(a.session.id)).toMatchObject({ used: 0, reserved: 0 });
    expect(ledger(p.session.id)).toMatchObject({ used: 0, reserved: 0 });
  });

  test("child_cannot_exceed_carve: BUDGET_EXCEEDED stops the child, not the parent; the child cannot answer its own budget approval", async () => {
    const { local, projectId } = await boot([PARENT, OVER]);
    const p = await root(local, projectId, { tokens: 5000 });
    const c = await spawn(p.client, projectId, "fixer", { budget: { tokens: 1000 } });
    await until(() => get(c.session.id).status === "interrupted", "the child's budget stop");
    expect(get(c.session.id).statusReason).toBe("BUDGET_EXCEEDED");
    expect(server.core.store.approvals(c.session.id)).toEqual([expect.objectContaining({ kind: "budget", status: "open" })]);
    expect(get(p.session.id).status).toBe("running");
    expect(ledger(p.session.id)).toEqual({ limit: 5000, used: 0, reserved: 1000 });
    expect(server.core.store.approvals(p.session.id)).toEqual([]);
    const approval = server.core.store.approvals(c.session.id)[0]!;
    expect(code(await c.client!.call("agent.approval.answer", { id: approval.id, decision: "allow", add: { tokens: 1000 }, challenge: "x" }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    expect(code(await p.client.call("agent.approval.answer", { id: approval.id, decision: "allow", add: { tokens: 1000 }, challenge: "x" }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    expect(ledger(c.session.id).limit).toBe(1000);
  });

  test("raise_child_raises_parent_reserved_or_fails: a person's raise past the parent's remaining is BUDGET_EXCEEDED on the parent; within it both rise", async () => {
    const { local, projectId } = await boot([PARENT, OVER]);
    const p = await root(local, projectId, { tokens: 5000 });
    const c = await spawn(p.client, projectId, "fixer", { budget: { tokens: 1000 } });
    await until(() => get(c.session.id).status === "interrupted", "the child's budget stop");
    const approval = server.core.store.approvals(c.session.id)[0]!;
    const owner = await server.console();
    const refused = await confirmed(owner, "agent.approval.answer", { id: approval.id, decision: "allow", add: { tokens: 4001 } });
    expect(code(refused)).toBe("BUDGET_EXCEEDED");
    expect(details(refused)).toMatchObject({ session: p.session.id, kind: "tokens", asked: 4001, remaining: 4000 });
    expect(server.core.store.approval(approval.id)!.status).toBe("open");
    expect(ledger(c.session.id).limit).toBe(1000);
    expect(ledger(p.session.id).reserved).toBe(1000);
    const answered = ok<{ approval: Approval }>(await confirmed(owner, "agent.approval.answer", { id: approval.id, decision: "allow", add: { tokens: 2000 } }));
    expect(answered.approval).toMatchObject({ status: "answered", decision: "allow", consumedAt: expect.any(Number) });
    expect(get(c.session.id).status).not.toBe("interrupted");
    expect(ledger(c.session.id).limit).toBe(3000);
    expect(ledger(p.session.id)).toEqual({ limit: 5000, used: 0, reserved: 3000 });
    // A person deletes the child: what it spent is charged to the parent, the rest returns.
    ok(await confirmed(owner, "agent.session.delete", { sessionId: c.session.id }));
    expect(ledger(p.session.id)).toEqual({ limit: 5000, used: 1200, reserved: 0 });
  });
});
