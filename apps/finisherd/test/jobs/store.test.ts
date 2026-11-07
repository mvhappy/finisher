// The job store in the `jobs` chain (5C-1 plan Task 7, Decisions 25–27, 30).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type ControlStore, openControlStore } from "../../src/control/db.ts";
import { CONTROL_CHAINS, ChainRegistry } from "../../src/control/migrations.ts";
import { JobStore, type NewJobRow } from "../../src/jobs/store.ts";

let dir: string;
const open: ControlStore[] = [];
let now = 1_000_000;
const clock = (): number => now;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "jobs-store-"));
  now = 1_000_000;
});

afterEach(async () => {
  await Promise.all(open.splice(0).map((s) => s.close()));
  rmSync(dir, { recursive: true, force: true });
});

function control(chains?: ChainRegistry): ControlStore {
  const s = openControlStore(dir, { serverId: "sid-1", checkpoint: false, clock, ...(chains ? { chains } : {}) });
  open.push(s);
  return s;
}

function jobs(): JobStore {
  return new JobStore(control(), clock);
}

const owner = { kind: "local", id: "local" } as const;

function row(over: Partial<NewJobRow> = {}): NewJobRow {
  return { kind: "test.echo", kindVersion: 1, family: "test", params: { value: 1 }, owner, priority: "normal", maxAttempts: 3, ...over };
}

const meta = (s: ControlStore, key: string): string | undefined =>
  (s.db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined)?.value;

describe("job store", () => {
  test("jobs_chain_registers_and_migrates", async () => {
    expect(CONTROL_CHAINS.get("jobs")?.formatVersion).toBe(1);
    // An existing database with `core` only gains the `jobs` chain at the next open.
    const coreOnly = new ChainRegistry();
    const core = CONTROL_CHAINS.get("core")!;
    coreOnly.register("core", core.migrations, core);
    const old = control(coreOnly);
    expect(meta(old, "formatVersion.jobs")).toBeUndefined();
    await old.close();
    open.splice(0);
    const s = control();
    expect(meta(s, "formatVersion.jobs")).toBe("1");
    expect(meta(s, "minReaderVersion.jobs")).toBe("1");
    const tables = (s.db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all() as Array<{ name: string }>).map((t) => t.name);
    expect(tables).toEqual(expect.arrayContaining(["jobs", "job_items"]));
    // `phase` defaults to `run`.
    s.db
      .prepare(
        "INSERT INTO jobs (id, kind, kindVersion, family, params, ownerKind, ownerId, priority, state, maxAttempts, createdAt) " +
          "VALUES ('j', 'test.echo', 1, 'test', '{}', 'system', 'x', 'normal', 'queued', 3, 1)",
      )
      .run();
    expect((s.db.prepare("SELECT phase FROM jobs WHERE id = 'j'").get() as { phase: string }).phase).toBe("run");
    await s.close();
    open.splice(0);
    // A fresh database gets both chains at once.
    rmSync(dir, { recursive: true, force: true });
    dir = mkdtempSync(join(tmpdir(), "jobs-store-"));
    expect(meta(control(), "formatVersion.jobs")).toBe("1");
  });

  test("insert_and_get_keep_every_field", () => {
    const s = jobs();
    const v = s.insert(
      row({
        via: { plugin: "p1" },
        rights: { scopes: ["a"] },
        project: "pr",
        library: "lib",
        parent: "par",
        idemKey: "k",
        dedupeKey: "d",
        warm: true,
      }),
    );
    expect(v).toMatchObject({ kind: "test.echo", state: "queued", attempts: 0, maxAttempts: 3, warm: true, createdAt: now });
    expect(v).toMatchObject({ via: { plugin: "p1" }, rights: { scopes: ["a"] }, project: "pr", library: "lib", parent: "par" });
    expect(v.id).toMatch(/^[0-9a-z]{26}$/);
    expect(s.get(v.id)).toEqual(v);
    expect("startedAt" in v || "finishedAt" in v || "error" in v).toBe(false);
    expect(s.row(v.id)?.phase).toBe("run");
    expect(s.get("nope")).toBeUndefined();
  });

  test("transitions_are_legal_only", () => {
    const s = jobs();
    const id = s.insert(row()).id;
    expect(() => s.transition(id, "succeeded")).toThrow(/illegal job transition queued → succeeded/);
    now += 10;
    const running = s.transition(id, "running", { attempts: 1 });
    expect(running).toMatchObject({ state: "running", attempts: 1, startedAt: now });
    now += 10;
    const back = s.transition(id, "queued", { nextRunAt: now + 5000, error: { code: "WORKER_CRASHED", message: "x", details: {} } });
    expect(back).toMatchObject({ state: "queued", nextRunAt: now + 5000, startedAt: now - 10 });
    expect(back.finishedAt).toBeUndefined();
    s.transition(id, "running", { nextRunAt: null, attempts: 2 });
    const failed = s.transition(id, "failed", { error: { code: "WORKER_CRASHED", message: "again", details: { n: 2 } } });
    expect(failed).toMatchObject({ state: "failed", finishedAt: now, error: { code: "WORKER_CRASHED", details: { n: 2 } } });
    expect(failed.nextRunAt).toBeUndefined();
    for (const to of ["queued", "running", "paused", "cancelled"] as const) expect(() => s.transition(id, to)).toThrow(/illegal/);
    // Paused keeps its place and resumes as queued; interrupted may be re-queued.
    const p = s.insert(row()).id;
    s.transition(p, "paused", { pausedFrom: "queued" });
    expect(s.row(p)?.pausedFrom).toBe("queued");
    expect(() => s.transition(p, "running")).toThrow(/illegal/);
    expect(s.transition(p, "queued", { pausedFrom: null }).state).toBe("queued");
    s.transition(p, "running");
    expect(s.transition(p, "interrupted").finishedAt).toBe(now);
    expect(s.transition(p, "queued").finishedAt).toBeUndefined();
    expect(() => s.transition("nope", "running")).toThrow(/unknown job/);
  });

  test("enter_hooks_keeps_running_and_persists_the_result", () => {
    const s = jobs();
    const id = s.insert(row()).id;
    expect(() => s.enterHooks(id, { a: 1 })).toThrow(/not running/);
    s.transition(id, "running");
    const v = s.enterHooks(id, { a: 1 });
    expect(v).toMatchObject({ state: "running", result: { a: 1 } });
    expect(s.row(id)?.phase).toBe("hooks");
    expect(s.inHooks().map((j) => j.id)).toEqual([id]);
    // A second store over the same database (a restart) sees the same.
    const again = new JobStore(open[0]!, clock);
    expect(again.row(id)).toMatchObject({ state: "running", phase: "hooks", result: { a: 1 } });
    expect(s.transition(id, "succeeded", { result: { a: 2 } })).toMatchObject({ state: "succeeded", result: { a: 2 } });
    expect(s.inHooks()).toEqual([]);
  });

  test("checkpoint_extra_and_isolation_round_trip", () => {
    const s = jobs();
    const id = s.insert(row()).id;
    s.checkpoint(id, { step: 3 });
    expect(s.row(id)?.checkpoint).toEqual({ step: 3 });
    s.transition(id, "running", { extra: { e: 1 }, isolation: { queue: [["a"]] } });
    expect(s.row(id)).toMatchObject({ checkpoint: { step: 3 }, extra: { e: 1 }, isolation: { queue: [["a"]] } });
    s.transition(id, "queued", { checkpoint: null, isolation: null });
    const r = s.row(id)!;
    expect("checkpoint" in r || "isolation" in r).toBe(false);
    expect(r.extra).toEqual({ e: 1 });
  });

  test("idem_per_owner", () => {
    const s = jobs();
    const a = s.insert(row({ idemKey: "k1" }));
    expect(s.byIdem(owner, "k1")?.id).toBe(a.id);
    expect(s.byIdem({ kind: "credential", id: "c1" }, "k1")).toBeUndefined();
    const b = s.insert(row({ idemKey: "k1", owner: { kind: "credential", id: "c1" } }));
    expect(s.byIdem({ kind: "credential", id: "c1" }, "k1")?.id).toBe(b.id);
    expect(() => s.insert(row({ idemKey: "k1" }))).toThrow();
  });

  test("dedupe_active_only", () => {
    const s = jobs();
    const a = s.insert(row({ dedupeKey: "d" }));
    expect(s.activeByDedupe("test.echo", "d")?.id).toBe(a.id);
    expect(s.activeByDedupe("test.sleep", "d")).toBeUndefined();
    s.transition(a.id, "paused");
    expect(s.activeByDedupe("test.echo", "d")?.id).toBe(a.id);
    s.transition(a.id, "cancelled");
    expect(s.activeByDedupe("test.echo", "d")).toBeUndefined();
    now += 1;
    const b = s.insert(row({ dedupeKey: "d" }));
    s.transition(b.id, "running");
    expect(s.activeByDedupe("test.echo", "d")?.id).toBe(b.id);
  });

  test("poison_memory", () => {
    const s = jobs();
    const a = s.insert(row({ kind: "test.crash", dedupeKey: "file1" }));
    s.transition(a.id, "running");
    s.transition(a.id, "failed", { error: { code: "WORKER_CRASHED", message: "x", details: {} } });
    expect(s.poisonedByDedupe("test.crash", "file1")).toBeUndefined();
    const b = s.insert(row({ kind: "test.crash", dedupeKey: "file1" }));
    s.transition(b.id, "running");
    s.transition(b.id, "failed", { error: { code: "POISON_ITEM", message: "poison", details: { item: "file1" } } });
    expect(s.poisonedByDedupe("test.crash", "file1")?.id).toBe(b.id);
    expect(s.poisonedByDedupe("test.crash", "file2")).toBeUndefined();
  });

  test("items_batch_write_and_read", () => {
    const s = jobs();
    const id = s.insert(row({ kind: "test.crash" })).id;
    const err = new FinError({ code: "POISON_ITEM", message: "crashed alone", details: { item: "b" } });
    s.writeItems(id, [
      ["a", { value: { n: 1 } }],
      ["b", { error: err }],
      ["c", {}],
    ]);
    s.writeItems(id, [["c", { value: 3 }]]);
    const items = s.items(id);
    expect([...items.keys()]).toEqual(["a", "b", "c"]);
    expect(items.get("a")).toEqual({ value: { n: 1 } });
    expect(items.get("c")).toEqual({ value: 3 });
    const b = items.get("b") as { error: FinError };
    expect(b.error).toBeInstanceOf(FinError);
    expect(b.error.code).toBe("POISON_ITEM");
    expect(s.get(id)?.itemErrors).toEqual([{ item: "b", error: { code: "POISON_ITEM", message: "crashed alone", details: { item: "b" } } }]);
    expect(s.list().jobs[0]?.itemErrors?.length).toBe(1);
  });

  test("list_cursor_and_filters", () => {
    const s = jobs();
    const ids: string[] = [];
    for (let i = 0; i < 7; i++) {
      now += 1;
      ids.push(s.insert(row({ project: i % 2 === 0 ? "p0" : "p1", kind: i < 5 ? "test.echo" : "test.sleep" })).id);
    }
    s.transition(ids[0]!, "running");
    const page1 = s.list({ limit: 3 });
    expect(page1.jobs.map((j) => j.id)).toEqual([ids[6], ids[5], ids[4]]);
    const page2 = s.list({ limit: 3, cursor: page1.next! });
    expect(page2.jobs.map((j) => j.id)).toEqual([ids[3], ids[2], ids[1]]);
    const page3 = s.list({ limit: 3, cursor: page2.next! });
    expect(page3).toEqual({ jobs: [s.get(ids[0]!)] });
    expect(s.list({ project: "p1" }).jobs.map((j) => j.id)).toEqual([ids[5], ids[3], ids[1]]);
    expect(s.list({ kind: "test.sleep" }).jobs.length).toBe(2);
    expect(s.list({ state: ["running"] }).jobs.map((j) => j.id)).toEqual([ids[0]]);
    expect(s.list({ state: ["running", "queued"], project: "p0", kind: "test.echo" }).jobs.length).toBe(3);
    expect(s.list({ owner: { kind: "device", id: "d" } }).jobs).toEqual([]);
    for (const cursor of ["garbage", "12.", ".abc", "1.a-b"]) {
      expect(() => s.list({ cursor }), cursor).toThrow(expect.objectContaining({ code: "INVALID_REQUEST", details: { field: "cursor" } }));
    }
    expect(s.list({ cursor: `0.${ids[0]}` })).toEqual({ jobs: [] });
    expect(s.list({ limit: 10_000 }).jobs.length).toBe(7);
    expect(s.unfinishedCount()).toBe(7);
    expect(s.running().map((j) => j.id)).toEqual([ids[0]]);
    expect(s.queued().length).toBe(6);
    expect(s.paused()).toEqual([]);
  });

  test("retention_deletes_items_first_in_batches", async () => {
    const c = control();
    const s = new JobStore(c, clock);
    const day = 86_400_000;
    c.tx(() => {
      for (let i = 0; i < 10_000; i++) {
        const id = s.insert(row({ id: `old${String(i).padStart(6, "0")}`, createdAt: 1 })).id;
        s.transition(id, "running");
        s.writeItems(id, ["a", "b", "c", "d"].map((x) => [x, { value: i }] as const));
        s.transition(id, "succeeded");
      }
    });
    now += 8 * day;
    const kept = s.insert(row({ id: "recent" }));
    s.transition(kept.id, "running");
    s.transition(kept.id, "succeeded");
    const active = s.insert(row({ id: "active" }));
    // Count the event-loop turns while it runs: it yields once per statement.
    let turns = 0;
    let running = true;
    const spin = (): void => {
      turns++;
      if (running) setImmediate(spin);
    };
    setImmediate(spin);
    const deleted = await Effect.runPromise(s.retention(now));
    running = false;
    expect(deleted).toBe(10_000);
    expect(turns).toBeGreaterThanOrEqual(80);
    expect((c.db.prepare("SELECT COUNT(*) AS n FROM job_items").get() as { n: number }).n).toBe(0);
    expect(s.list().jobs.map((j) => j.id).sort()).toEqual([active.id, kept.id].sort());
    // Beyond the newest N finished, even when young.
    for (let i = 0; i < 5; i++) {
      now += 1;
      const id = s.insert(row()).id;
      s.transition(id, "running");
      s.transition(id, "failed", { error: { code: "X", message: "x", details: {} } });
    }
    expect(await Effect.runPromise(s.retention(now, { maxRows: 3 }))).toBe(3);
    expect(s.list({ state: ["succeeded", "failed"] }).jobs.length).toBe(3);
  });
});
