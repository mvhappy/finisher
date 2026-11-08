// Storage bindings (plan 4A Task 14): status, takeover, verify, rebuild, compact, txGet, the
// maintenance thread and its events. Real clocks: a local folder polls every 2 s and exports
// after 2 s, so a follower sees a commit within ~4–6 s.
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError } from "../index.js";

let tmp: string;
let serverData: string;
let projectDir: string;
let engines: Engine[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-storage-"));
  serverData = join(tmp, "server-data");
  projectDir = join(tmp, "My Project");
});

afterEach(async () => {
  for (const e of engines) await e.close().catch(() => undefined);
  engines = [];
  rmSync(tmp, { recursive: true, force: true });
});

function keep(e: Engine): Engine {
  engines.push(e);
  return e;
}

function tx(txId: string, baseRev: number, ops: unknown[]): string {
  return JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });
}

const addAsset = {
  op: "asset.add",
  id: "$a",
  path: "/media/a.mov",
  duration: "600",
  video: { width: 1920, height: 1080, rate: "30" },
  audio: null,
  fingerprint: "fp",
};

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

type Event = { type: string; [k: string]: unknown };

/** Collects an engine's events. */
function listen(engine: Engine): Event[] {
  const events: Event[] = [];
  engine.onEvent((json) => events.push(JSON.parse(json) as Event));
  return events;
}

async function until<T>(f: () => T | undefined | Promise<T | undefined>, ms: number, what: string): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The engine's daily backups. */
function dailies(engine: Engine): string[] {
  try {
    return readdirSync(join(serverData, "projects", engine.projectId, "backups")).filter((n) => n.startsWith("daily-"));
  } catch {
    return [];
  }
}

/** Polls `status()` until a long call named `operation` has the store; returns that status. */
async function busyStatus(engine: Engine, operation: string) {
  return until(
    async () => {
      const st = JSON.parse(await engine.status());
      return st.busy?.operation === operation ? st : undefined;
    },
    3_000,
    `${operation} to run`,
  );
}

describe("storage", () => {
  test("status reports lease held and storage kind", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const status = JSON.parse(await engine.status());
    expect(status.projectId).toBe(engine.projectId);
    expect(status.storage).toBe("local");
    expect(status.lease.state).toBe("held");
    expect(status.readOnly).toBeNull();
    expect(status.branches).toEqual([expect.objectContaining({ branch: "main", head: 0 })]);
    expect(typeof status.incarnation).toBe("string");
    // Holding already: the status, unchanged.
    expect(JSON.parse(await engine.takeover(false)).lease).toEqual(status.lease);
  });

  test("second engine on same folder follows and emits applied events", async () => {
    const a = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const b = keep(await Engine.open(join(tmp, "server-data-2"), projectDir));
    const events = listen(b);
    const sb = JSON.parse(await b.status());
    expect(sb.lease.state).toBe("heldElsewhere");
    expect(sb.readOnly.reason).toBe("LEASE_HELD");
    expect((await rejection(b.commit(tx("tx-b", 0, [addAsset])))).code).toBe("PROJECT_READ_ONLY");
    const refused = await rejection(b.takeover(true));
    expect(refused.code).toBe("LEASE_TAKEOVER_REFUSED");

    await a.commit(tx("tx-1", 0, [addAsset]));
    const applied = await until(() => events.find((e) => e.type === "applied"), 10_000, "an applied event");
    expect((applied.tx as { tx: { txId: string }; rev: number }).tx.txId).toBe("tx-1");
    expect((applied.tx as { rev: number }).rev).toBe(1);
    expect(await b.head("main")).toBe(1);
    // The follower's first tick reports its status.
    expect(events.some((e) => e.type === "status")).toBe(true);
  }, 20_000);

  test("txGet returns committed tx or null", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const committed = await engine.commit(tx("tx-1", 0, [addAsset]));
    const got = await engine.txGet("tx-1");
    expect(got).not.toBeNull();
    expect(JSON.parse(got!)).toEqual(JSON.parse(committed));
    expect(await engine.txGet("tx-unknown")).toBeNull();
  });

  test("commit accepts an expected generation", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const { generation, ops } = JSON.parse(await engine.opsSinceAt("main", 0));
    expect(ops).toEqual([]);
    expect(generation).toEqual({ incarnation: expect.any(String), n: 0 });
    const c = JSON.parse(await engine.commit(tx("tx-1", 0, [addAsset]), JSON.stringify(generation)));
    expect(c.rev).toBe(1);
    const stale = { incarnation: "someotherincarnation", n: 0 };
    const err = await rejection(engine.commit(tx("tx-2", 1, [addAsset]), JSON.stringify(stale)));
    expect(err.code).toBe("HISTORY_REWOUND");
    expect((await rejection(engine.commit(tx("tx-3", 1, [addAsset]), "{bad"))).code).toBe("INVALID_OP");
    expect(await engine.acknowledgeRewind("main", 7)).toBe(false);
    expect(await engine.acknowledgeSalvage("project.fdb.corrupt-x")).toBe(false);
  });

  test("verify returns report", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    await engine.commit(tx("tx-1", 0, [addAsset]));
    const report = JSON.parse(await engine.verify());
    expect(report.ok).toBe(true);
    expect(report.checks.map((c: { name: string }) => c.name)).toContain("integrity_check");
  });

  test("rebuild and compact return reports", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    await engine.commit(tx("tx-1", 0, [addAsset]));
    const rebuilt = JSON.parse(await engine.rebuild());
    expect(rebuilt.reason).toBe("manual");
    expect(rebuilt.rebuiltThrough).toBe(1);
    expect(await engine.head("main")).toBe(1);
    const events = listen(engine);
    const rebuiltEvent = await until(() => events.find((e) => e.type === "rebuilt"), 3_000, "a rebuilt event");
    expect(rebuiltEvent.incarnation).toBe(JSON.parse(await engine.status()).incarnation);
    const compacted = JSON.parse(await engine.compact());
    expect(compacted).toMatchObject({ packsWritten: 0, deleted: 0 });
    expect(Array.isArray(compacted.manifests)).toBe(true);
  });

  test("a commit while verify runs fails with ENGINE_BUSY; status answers from its cache", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    expect(JSON.parse(await engine.status()).busy).toBeNull();
    engine.delayNextLongCallForTest(1_000);
    const verifying = engine.verify();
    const during = await busyStatus(engine, "verify");
    expect(during.projectId).toBe(engine.projectId);
    expect(during.lease.state).toBe("held");
    expect(typeof during.busy.sinceMs).toBe("number");
    const busy = await rejection(engine.commit(tx("tx-1", 0, [addAsset])));
    expect(busy.code).toBe("ENGINE_BUSY");
    expect(busy.details.operation).toBe("verify");
    expect(typeof busy.fix).toBe("string");
    expect(JSON.parse(await verifying).ok).toBe(true);
    expect(JSON.parse(await engine.commit(tx("tx-1", 0, [addAsset]))).rev).toBe(1);
  });

  test("the daily backup due at open never makes a call ENGINE_BUSY", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    // The worst case: the backup starts at the first tick (not after 30 quiet seconds), and
    // stays inside its run for 1.5 s while the calls go on.
    engine.setAutoIdleForTest(0);
    engine.pauseAutoBackupForTest(1_500);
    let rev = 0;
    let during = 0;
    const call = async (i: number) => {
      const running = engine.autoBackupRunningForTest();
      if (i % 2 === 0) {
        rev = JSON.parse(await engine.commit(tx(`tx-${i}`, rev, [addAsset]))).rev;
      } else {
        expect(await engine.head("main")).toBe(rev);
      }
      if (running && engine.autoBackupRunningForTest()) during += 1;
    };
    // 50 short calls from the open on: the first right away, the rest once the backup runs.
    for (let i = 0; i < 10; i++) await call(i);
    await until(() => (engine.autoBackupRunningForTest() ? true : undefined), 5_000, "the backup to start");
    for (let i = 10; i < 50; i++) await call(i);
    expect(during).toBeGreaterThan(0);
    expect(JSON.parse(await engine.status()).busy).toBeNull();
    await until(() => (engine.autoBackupRunningForTest() ? undefined : true), 10_000, "the backup to finish");
    const dir = join(serverData, "projects", engine.projectId, "backups");
    const daily = readdirSync(dir).filter((n) => n.startsWith("daily-"));
    expect(daily).toHaveLength(1);
    expect(readdirSync(dir).filter((n) => n.endsWith(".tmp"))).toEqual([]);
    const db = new DatabaseSync(join(dir, daily[0]!), { readOnly: true });
    try {
      expect(db.prepare("PRAGMA integrity_check").get()).toEqual({ integrity_check: "ok" });
      // One snapshot: what was committed when the copy was taken.
      const { n } = db.prepare("SELECT COUNT(*) AS n FROM txs").get() as { n: number };
      expect(n).toBeLessThanOrEqual(25);
    } finally {
      db.close();
    }
  });

  test("a long call cancels a running automatic backup; it is taken later", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    engine.setAutoIdleForTest(0);
    engine.pauseAutoBackupForTest(60_000);
    await until(() => (engine.autoBackupRunningForTest() ? true : undefined), 5_000, "the backup to start");
    const started = Date.now();
    expect(JSON.parse(await engine.verify()).ok).toBe(true);
    expect(Date.now() - started).toBeLessThan(10_000);
    expect(engine.autoBackupRunningForTest()).toBe(false);
    const dir = join(serverData, "projects", engine.projectId, "backups");
    expect(readdirSync(dir).filter((n) => n.startsWith("daily-") || n.endsWith(".tmp"))).toEqual([]);
    engine.pauseAutoBackupForTest(0);
    await until(
      () => (readdirSync(dir).some((n) => n.startsWith("daily-")) ? true : undefined),
      10_000,
      "the backup taken again",
    );
  });

  test("automatic work waits for a quiet engine", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    // Due at the first tick, but the engine has not been quiet for 30 s.
    for (let i = 0; i < 6; i++) {
      await sleep(250);
      expect(engine.autoBackupRunningForTest()).toBe(false);
    }
    expect(dailies(engine)).toEqual([]);
    // Quiet long enough (now 1 s): it starts.
    engine.setAutoIdleForTest(1_000);
    await until(() => (dailies(engine).length === 1 ? true : undefined), 10_000, "the backup");
  });

  test("restart with the same server data is writable at once", async () => {
    const first = await Engine.create(serverData, projectDir, "Demo", "30");
    await first.commit(tx("tx-1", 0, [addAsset]));
    await first.close();
    const again = keep(await Engine.open(serverData, projectDir));
    expect(JSON.parse(await again.status()).lease.state).toBe("held");
    expect(JSON.parse(await again.commit(tx("tx-2", 1, [addAsset]))).rev).toBe(2);
  });

  test("events include status on lease change", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const events = listen(engine);
    const first = await until(() => events.find((e) => e.type === "status"), 3_000, "the first status event");
    const epoch = (first.status as { lease: { epoch: number } }).lease.epoch;
    // Losing lease.json fences this server; the next poll re-acquires at a new epoch.
    rmSync(join(projectDir, ".finisher", "lease.json"));
    const changed = await until(
      () =>
        events.find((e) => {
          const lease = (e.status as { lease?: { state: string; epoch?: number } } | undefined)?.lease;
          return e.type === "status" && lease !== undefined && (lease.state !== "held" || lease.epoch !== epoch);
        }),
      8_000,
      "a status event with another lease",
    );
    expect(changed.type).toBe("status");
  }, 15_000);

  test("close stops maintenance", async () => {
    const engine = await Engine.create(serverData, projectDir, "Demo", "30");
    const events = listen(engine);
    expect(engine.maintenanceRunningForTest()).toBe(true);
    await until(() => events.find((e) => e.type === "status"), 3_000, "a status event");
    await engine.close();
    expect(engine.maintenanceRunningForTest()).toBe(false);
    const seen = events.length;
    await sleep(1_200);
    expect(events.length).toBe(seen);
    expect((await rejection(engine.status())).code).toBe("ENGINE_CLOSED");
    // Closing twice is a no-op.
    await engine.close();
  });

  test("a panic in tick marks the engine panicked", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    await engine.commit(tx("tx-1", 0, [addAsset]));
    const events = listen(engine);
    engine.panicNextTickForTest();
    const fatal = await until(() => events.find((e) => e.type === "error"), 3_000, "the error event");
    expect(fatal.fatal).toBe(true);
    expect((fatal.error as { code: string }).code).toBe("INTERNAL");
    const dead = await rejection(engine.head("main"));
    expect(dead.code).toBe("ENGINE_CLOSED");
    expect(dead.details.reason).toBe("panicked");
    expect(engine.maintenanceRunningForTest()).toBe(false);
    // The project reopens intact.
    const again = keep(await Engine.open(serverData, projectDir));
    expect(await again.head("main")).toBe(1);
  });

  test("records imported at open are announced as applied", async () => {
    const a = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    await a.commit(tx("tx-1", 0, [addAsset]));
    const otherData = join(tmp, "server-data-2");
    // A first open builds B's database (announced as rebuilt).
    const fresh = await Engine.open(otherData, projectDir);
    const first = listen(fresh);
    await until(() => first.find((e) => e.type === "rebuilt"), 3_000, "the rebuilt event");
    await fresh.close();
    await a.commit(tx("tx-2", 1, [addAsset]));
    await a.close();
    const b = keep(await Engine.open(otherData, projectDir));
    expect(await b.head("main")).toBe(2);
    const events = listen(b);
    const ids = () => events.filter((e) => e.type === "applied").map((e) => (e.tx as { tx: { txId: string } }).tx.txId);
    // tx-1 may not have been exported when the first open built B's database.
    await until(() => (ids().includes("tx-2") ? true : undefined), 3_000, "tx-2 applied");
    expect(ids().slice(-1)).toEqual(["tx-2"]);
  });

  test("events before onEvent are buffered", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    await until(async () => (engine.maintenanceRunningForTest() ? true : undefined), 1_000, "maintenance");
    await sleep(1_200);
    const events = listen(engine);
    expect(events.length).toBe(0);
    await until(() => events.find((e) => e.type === "status"), 1_000, "the buffered status event");
    expect(events.some((e) => e.type === "lagged")).toBe(false);
  });

  test("rewound events carry the generation and incarnation", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const events = listen(engine);
    engine.emitRewoundForTest("main", 3);
    const rewound = await until(() => events.find((e) => e.type === "rewound"), 1_000, "a rewound event");
    expect(rewound).toEqual({
      type: "rewound",
      branch: "main",
      fromRev: 3,
      quarantined: 1,
      own: false,
      generation: 1,
      incarnation: JSON.parse(await engine.status()).incarnation,
    });
  });

  test("a throwing onEvent callback becomes a process warning", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    const warnings: Error[] = [];
    const onWarning = (w: Error) => warnings.push(w);
    process.on("warning", onWarning);
    try {
      engine.onEvent(() => {
        throw new Error("callback bug");
      });
      engine.emitRewoundForTest("main", 1);
      const w = await until(() => warnings.find((x) => x.message.includes("callback bug")), 2_000, "the warning");
      expect((w as Error & { code?: string }).code).toBe("FIN_NODE_EVENT_CALLBACK");
    } finally {
      process.off("warning", onWarning);
    }
  });

  test("close inside the onEvent callback", async () => {
    const engine = await Engine.create(serverData, projectDir, "Demo", "30");
    let closing: Promise<void> | undefined;
    engine.onEvent(() => {
      closing ??= engine.close();
    });
    await until(() => (closing ? true : undefined), 3_000, "an event");
    await closing;
    expect(engine.maintenanceRunningForTest()).toBe(false);
    expect((await rejection(engine.head("main"))).code).toBe("ENGINE_CLOSED");
  });

  test("close during a long call waits for it; double close is a no-op", async () => {
    const engine = await Engine.create(serverData, projectDir, "Demo", "30");
    await engine.commit(tx("tx-1", 0, [addAsset]));
    engine.delayNextLongCallForTest(500);
    const verifying = engine.verify();
    await busyStatus(engine, "verify");
    await Promise.all([engine.close(), engine.close()]);
    expect(JSON.parse(await verifying).ok).toBe(true);
    expect((await rejection(engine.verify())).code).toBe("ENGINE_CLOSED");
    // Flushed on close: a reopen finds the record in the folder too.
    const again = keep(await Engine.open(join(tmp, "server-data-2"), projectDir));
    expect(await again.head("main")).toBe(1);
  });
});
