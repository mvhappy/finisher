// The control store (plan 5A Task 1, Decisions 1–3).
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { measureControlBudget } from "../scripts/control-budget.ts";
import { type ControlStore, controlDbPath, openControlStore } from "../src/control/db.ts";
import { ChainRegistry, CONTROL_CHAINS, type Migration } from "../src/control/migrations.ts";
import { isUlid } from "../src/control/ulid.ts";

const WIN = process.platform === "win32";
let dir: string;
const open: ControlStore[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "control-"));
});

afterEach(async () => {
  await Promise.all(open.splice(0).map((s) => s.close()));
  rmSync(dir, { recursive: true, force: true });
});

function store(opts: Partial<Parameters<typeof openControlStore>[1]> = {}): ControlStore {
  const s = openControlStore(dir, { serverId: "sid-1", checkpoint: false, ...opts });
  open.push(s);
  return s;
}

async function closeAll(): Promise<void> {
  await Promise.all(open.splice(0).map((s) => s.close()));
}

function meta(s: ControlStore, key: string): string | undefined {
  return (s.db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined)?.value;
}

/** A registry with `core` plus the given chains. */
function registry(...chains: Array<[string, Migration[], { minReaderVersion?: number }?]>): ChainRegistry {
  const r = new ChainRegistry();
  const core = CONTROL_CHAINS.get("core")!;
  r.register("core", core.migrations, core);
  for (const [name, steps, v] of chains) r.register(name, steps, v);
  return r;
}

const table = (name: string, n: number): Migration => ({ n, up: (db) => db.exec(`CREATE TABLE ${name} (x INTEGER) STRICT`) });

describe("control store", () => {
  test("opens_wal_0600_with_meta_and_env_id", async () => {
    const s = store();
    expect(isUlid(s.envId)).toBe(true);
    expect((s.db.prepare("PRAGMA journal_mode").get() as { journal_mode: string }).journal_mode).toBe("wal");
    expect((s.db.prepare("PRAGMA foreign_keys").get() as { foreign_keys: number }).foreign_keys).toBe(1);
    expect((s.db.prepare("PRAGMA synchronous").get() as { synchronous: number }).synchronous).toBe(1);
    expect(meta(s, "formatVersion.core")).toBe("1");
    expect(meta(s, "minReaderVersion.core")).toBe("1");
    expect(Number(meta(s, "createdAt"))).toBeGreaterThan(0);
    if (!WIN) expect(statSync(controlDbPath(dir)).mode & 0o777).toBe(0o600);
    const envId = s.envId;
    await closeAll();
    expect(store().envId).toBe(envId);
  });

  // 10k commits: well under the default 5 s test timeout alone, not at load average 20.
  test("main_connection_never_autocheckpoints", { timeout: 60_000 }, async () => {
    // No checkpoint worker: whatever shrinks the WAL here could only be the main connection. (With one
    // running, a scheduling stall of its idle time in the middle of the writes lets it truncate, which
    // is its job and not this test's question.)
    const s = store({ checkpoint: false });
    expect((s.db.prepare("PRAGMA wal_autocheckpoint").get() as { wal_autocheckpoint: number }).wal_autocheckpoint).toBe(0);
    s.db.exec("CREATE TABLE t (x TEXT)");
    const ins = s.db.prepare("INSERT INTO t (x) VALUES (?)");
    // 10k separate commits: with autocheckpoint at 1000 pages the WAL would be reset over and over.
    for (let i = 0; i < 10_000; i++) ins.run(`row ${i} ${"x".repeat(100)}`);
    expect(statSync(`${controlDbPath(dir)}-wal`).size).toBeGreaterThan(4 * 1024 * 1024);
    expect((s.db.prepare("SELECT COUNT(*) AS n FROM t").get() as { n: number }).n).toBe(10_000);
  });

  test("checkpoint_worker_truncates_the_wal_once_writes_stop", { timeout: 60_000 }, async () => {
    const s = store({ checkpoint: { pollMs: 20, passiveMs: 60_000, idleTruncateMs: 1_500, walLimitBytes: 1 << 30 } });
    s.db.exec("CREATE TABLE t (x TEXT)");
    const ins = s.db.prepare("INSERT INTO t (x) VALUES (?)");
    const wal = () => (existsSync(`${controlDbPath(dir)}-wal`) ? statSync(`${controlDbPath(dir)}-wal`).size : 0);
    for (let i = 0; i < 300; i++) ins.run(`row ${i} ${"x".repeat(100)}`);
    expect(wal()).toBeGreaterThan(0);
    // The worker truncates once writes stop (polling to a deadline; the idle time is the worker's).
    const deadline = Date.now() + 45_000;
    while (wal() > 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
    expect(wal()).toBe(0);
    expect((s.db.prepare("SELECT COUNT(*) AS n FROM t").get() as { n: number }).n).toBe(300);
  });

  test("server_ids_map_every_server_id", async () => {
    const envId = store({ serverId: "sid-a" }).envId;
    await closeAll();
    store({ serverId: "sid-a" });
    await closeAll();
    const s = store({ serverId: "sid-b" });
    const rows = s.db.prepare("SELECT serverId, envId FROM server_ids ORDER BY serverId").all();
    expect(rows.map((r) => ({ ...r }))).toEqual([
      { serverId: "sid-a", envId },
      { serverId: "sid-b", envId },
    ]);
  });

  test("migrations_run_once_in_order_with_backup", async () => {
    const order: string[] = [];
    const step = (n: number): Migration => ({ n, up: (db) => (order.push(`a${n}`), db.exec(`CREATE TABLE a${n} (x INTEGER) STRICT`)) });
    store({ chains: registry(["alpha", [step(1)]]) });
    await closeAll();
    expect(existsSync(`${controlDbPath(dir)}.pre-migrate.bak`)).toBe(false); // a new database needs none
    store({ chains: registry(["alpha", [step(1), step(2), step(3)]]) });
    await closeAll();
    expect(existsSync(`${controlDbPath(dir)}.pre-migrate.bak`)).toBe(true);
    const s = store({ chains: registry(["alpha", [step(1), step(2), step(3)]]) });
    expect(order).toEqual(["a1", "a2", "a3"]);
    const rows = s.db.prepare("SELECT chain, n FROM migrations ORDER BY chain, n").all().map((r) => ({ ...r }));
    expect(rows).toEqual([
      { chain: "alpha", n: 1 },
      { chain: "alpha", n: 2 },
      { chain: "alpha", n: 3 },
      { chain: "core", n: 1 },
    ]);
    // The backup is a database as it was before the migration.
    const bak = new DatabaseSync(`${controlDbPath(dir)}.pre-migrate.bak`);
    expect(bak.prepare("SELECT MAX(n) AS n FROM migrations WHERE chain = 'alpha'").get()).toEqual({ n: 1 });
    bak.close();
    await closeAll();
    // A fourth step: the newest two backups are kept.
    store({ chains: registry(["alpha", [step(1), step(2), step(3), step(4)]]) });
    await closeAll();
    const baks = readdirSync(dir).filter((f) => f.includes("pre-migrate"));
    expect(baks.sort()).toEqual(["control.db.pre-migrate.bak", "control.db.pre-migrate.bak.1"]);
  });

  test("a failing step rolls back the whole start", async () => {
    store();
    await closeAll();
    const bad: Migration = { n: 1, up: () => { throw new Error("boom"); } };
    expect(() => store({ chains: registry(["alpha", [table("x1", 1)]], ["beta", [bad]]) })).toThrow(/boom/);
    const s = store();
    expect(s.db.prepare("SELECT 1 FROM sqlite_master WHERE name = 'x1'").get()).toBeUndefined();
  });

  test("chains_are_independent", async () => {
    // Registered in either order, each chain runs once, `core` first, the others by name.
    const ran: string[] = [];
    const mk = (name: string): Migration => ({ n: 1, up: (db) => (ran.push(name), db.exec(`CREATE TABLE ${name}_t (x INTEGER) STRICT`)) });
    store({ chains: registry(["zeta", [mk("zeta")]], ["beta", [mk("beta")]]) });
    await closeAll();
    store({ chains: registry(["beta", [mk("beta")]], ["zeta", [mk("zeta")]]) });
    expect(ran).toEqual(["beta", "zeta"]);
    await closeAll();
    // Another chain's newer reader requirement refuses the start and names that chain.
    const s = store({ chains: registry(["beta", [mk("beta")]]) });
    s.db.prepare("UPDATE meta SET value = '9' WHERE key = 'minReaderVersion.zeta'").run();
    await closeAll();
    // An unknown chain (newer code's) is ignored…
    store({ chains: registry(["beta", [mk("beta")]]) });
    await closeAll();
    // …but a known one too new refuses.
    let err: unknown;
    try {
      store({ chains: registry(["beta", [mk("beta")]], ["zeta", [mk("zeta")]]) });
    } catch (e) {
      err = e;
    }
    expect(err).toMatchObject({ code: "CONTROL_DB_TOO_NEW", details: { chains: [{ chain: "zeta", minReaderVersion: 9, readerVersion: 1 }] } });
  });

  test("newer_min_reader_refuses_start", async () => {
    const s = store();
    s.db.prepare("UPDATE meta SET value = '2' WHERE key = 'minReaderVersion.core'").run();
    await closeAll();
    expect(() => store()).toThrow(expect.objectContaining({ code: "CONTROL_DB_TOO_NEW" }));
  });

  test("corrupt_db_is_moved_aside_and_audited", async () => {
    const before = store().envId;
    await closeAll();
    for (const f of ["control.db-wal", "control.db-shm"]) rmSync(join(dir, f), { force: true });
    writeFileSync(controlDbPath(dir), Buffer.alloc(8192, 7));
    const s = store();
    expect(s.envId).not.toBe(before);
    expect(readdirSync(dir).some((f) => /^control\.db\.corrupt-[0-9a-z]{26}$/.test(f))).toBe(true);
    const first = s.db.prepare("SELECT kind, principalKind, details FROM audit ORDER BY id LIMIT 1").get();
    expect({ ...first }).toEqual({ kind: "control.reset", principalKind: "system", details: '{"reason":"notADatabase"}' });
  });

  test("an unreadable schema refuses the start and resets nothing", async () => {
    const envId = store().envId;
    await closeAll(); // the final checkpoint leaves everything in control.db
    const file = readFileSync(controlDbPath(dir));
    const sql = "CREATE INDEX audit_at ON audit (at)";
    const at = file.indexOf(sql);
    expect(at).toBeGreaterThan(0);
    file.write("CREATE INDEX audit_at ON audit (at(", at); // same length, no longer parses
    writeFileSync(controlDbPath(dir), file);
    expect(() => store()).toThrow(expect.objectContaining({ code: "CONTROL_DB_TOO_NEW" }));
    expect(readdirSync(dir).some((f) => f.includes("corrupt"))).toBe(false);
    file.write(sql, at);
    writeFileSync(controlDbPath(dir), file);
    expect(store().envId).toBe(envId);
  });

  test("busy_maps_to_control_db_busy", () => {
    const s = store();
    const other = new DatabaseSync(controlDbPath(dir));
    other.exec("BEGIN IMMEDIATE");
    try {
      const started = Date.now();
      expect(() => s.tx(() => s.db.exec("CREATE TABLE b (x)"))).toThrow(expect.objectContaining({ code: "CONTROL_DB_BUSY" }));
      expect(Date.now() - started).toBeGreaterThanOrEqual(200);
    } finally {
      other.exec("ROLLBACK");
      other.close();
    }
    // The store is usable again.
    s.tx(() => s.db.exec("CREATE TABLE b (x)"));
  });

  test("tx rolls back on throw and nests with savepoints", () => {
    const s = store();
    s.db.exec("CREATE TABLE t (x INTEGER)");
    expect(() =>
      s.tx(() => {
        s.db.exec("INSERT INTO t VALUES (1)");
        throw new Error("no");
      }),
    ).toThrow("no");
    s.tx(() => {
      s.db.exec("INSERT INTO t VALUES (2)");
      expect(() => s.tx(() => (s.db.exec("INSERT INTO t VALUES (3)"), (() => { throw new Error("inner"); })()))).toThrow("inner");
    });
    expect(s.db.prepare("SELECT x FROM t").all().map((r) => r.x)).toEqual([2]);
  });

  test("deleteInBatches yields between batches of at most 500", async () => {
    const s = store();
    s.db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)");
    s.tx(() => {
      const ins = s.db.prepare("INSERT INTO t (id) VALUES (?)");
      for (let i = 1; i <= 2600; i++) ins.run(i);
    });
    let turns = 0;
    let running = true;
    const spin = () => {
      if (!running) return;
      turns++;
      setImmediate(spin);
    };
    setImmediate(spin);
    const n = await Effect.runPromise(
      s.deleteInBatches("DELETE FROM t WHERE id IN (SELECT id FROM t WHERE id > ? ORDER BY id LIMIT ?)", [100], 10_000),
    );
    running = false;
    expect(n).toBe(2500);
    expect(turns).toBeGreaterThanOrEqual(4); // 5 batches of 500, 4 turns between them
  });

  test("budgeted_statements_use_their_indexes", () => {
    // The budget below is generous unless `FIN_STRICT_TIMING=1` (load adds as much as a missing
    // index does), so what a statement that lost its index would show regardless of load is its plan:
    // a scan of the whole table.
    const s = store();
    const plan = (sql: string, ...args: Array<string | number>) =>
      s.db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all(...args).map((r) => String(r.detail)).join("\n");
    expect(plan("SELECT id FROM audit WHERE at < ? ORDER BY at LIMIT ?", 1, 500)).toMatch(/SEARCH audit USING (COVERING )?INDEX audit_at/);
    expect(plan("SELECT id FROM audit WHERE kind = ? ORDER BY id DESC LIMIT ?", "x", 50)).toMatch(/SEARCH audit USING (COVERING )?INDEX audit_kind/);
    expect(plan("SELECT id FROM audit WHERE id <= ? ORDER BY id LIMIT ?", 1, 500)).not.toMatch(/SCAN audit(?! USING)/);
  });

  test("control_store_budget", { timeout: 120_000 }, async () => {
    // Load from other processes only adds time: the best of up to three runs per statement.
    // Under a full-suite run that load reaches a statement's tail (a 100k-row scan would move
    // the median too), so the strict p99 ≤ 5 ms is asked only with `FIN_STRICT_TIMING=1`;
    // otherwise the bounds are generous (a median ≤ 25 ms, a p99 ≤ 250 ms: at load average 20 a
    // median of 7 ms was seen), still far under a statement that lost its index and scans.
    const strict = process.env.FIN_STRICT_TIMING === "1";
    const p99Bound = strict ? 5 : 250;
    const p50Bound = strict ? 1 : 25;
    let best: Record<string, number> = {};
    let bestP50: Record<string, number> = {};
    const min = (a: Record<string, number>, b: Record<string, number>) =>
      Object.fromEntries(Object.entries(b).map(([k, v]) => [k, Math.min(v, a[k] ?? Infinity)]));
    for (let attempt = 0; attempt < 3; attempt++) {
      const run = mkdtempSync(join(dir, `budget-${attempt}-`));
      const r = await measureControlBudget(run);
      best = min(best, r.p99Ms);
      bestP50 = min(bestP50, r.p50Ms);
      console.log(
        `control store p99 (ms), run ${attempt + 1}: ${JSON.stringify(r.p99Ms)}; p50 ${JSON.stringify(r.p50Ms)}; a whole sweep ${r.sweepMs} ms`,
      );
      if (Object.values(best).every((v) => v <= 5)) break;
    }
    for (const [k, p50] of Object.entries(bestP50)) expect(p50, `${k} median`).toBeLessThanOrEqual(p50Bound);
    for (const [k, p99] of Object.entries(best)) expect(p99, k).toBeLessThanOrEqual(p99Bound);
  });
});
