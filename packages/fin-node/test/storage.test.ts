// Storage bindings (plan 4A Task 14): status, takeover, verify, rebuild, compact, txGet, the
// maintenance thread and its events. Real clocks: a local folder polls every 2 s and exports
// after 2 s, so a follower sees a commit within ~4–6 s.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
  return JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 1, ops });
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

async function until<T>(f: () => T | undefined, ms: number, what: string): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

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
    const compacted = JSON.parse(await engine.compact());
    expect(compacted).toMatchObject({ packsWritten: 0, deleted: 0 });
    expect(Array.isArray(compacted.manifests)).toBe(true);
  });

  test("a commit while verify runs fails fast with ENGINE_BUSY", async () => {
    const engine = keep(await Engine.create(serverData, projectDir, "Demo", "30"));
    engine.delayNextLongCallForTest(600);
    const verifying = engine.verify();
    await sleep(100);
    const busy = await rejection(engine.commit(tx("tx-1", 0, [addAsset])));
    expect(busy.code).toBe("ENGINE_BUSY");
    expect(busy.details.operation).toBe("verify");
    expect(typeof busy.fix).toBe("string");
    expect((await rejection(engine.status())).code).toBe("ENGINE_BUSY");
    expect(JSON.parse(await verifying).ok).toBe(true);
    expect(JSON.parse(await engine.commit(tx("tx-1", 0, [addAsset]))).rev).toBe(1);
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
});
