import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError, probe } from "../index.js";

let tmp: string;
let serverData: string;
let projectDir: string;
let engine: Engine | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-"));
  serverData = join(tmp, "server-data");
  projectDir = join(tmp, "My Project");
});

afterEach(async () => {
  await engine?.close();
  engine = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

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

function insert(at: number, duration = 1): object {
  return { op: "clip.insert", id: "$c", track: "t1", at: String(at), asset: "a1", sourceIn: "0", duration: String(duration) };
}

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

describe("Engine", () => {
  test("create, commit, snapshot and opsSince round-trip; reopen sees the same state", async () => {
    engine = await Engine.create(serverData, projectDir, "Demo", "30000/1001");
    expect(engine.projectId).toMatch(/^[0-9a-z]{26}$/);
    expect(await engine.head("main")).toBe(0);

    const c1 = JSON.parse(await engine.commit(tx("tx-1", 0, [addAsset])));
    expect(c1.rev).toBe(1);
    expect(c1.bindings).toEqual({ $a: "a1" });
    const c2 = JSON.parse(await engine.commit(tx("tx-2", 1, [insert(0, 5)])));
    expect(c2.bindings).toEqual({ $c: "c1" });

    const snap = JSON.parse(await engine.snapshot("main"));
    expect(snap.rev).toBe(2);
    expect(snap.project.rate).toBe("30000/1001");
    expect(snap.project.timeline.tracks[0].items).toMatchObject([{ id: "c1", start: "0", duration: "5" }]);

    const since = JSON.parse(await engine.opsSince("main", 1));
    expect(since.map((c: { rev: number }) => c.rev)).toEqual([2]);
    expect(since[0].tx.txId).toBe("tx-2");

    const id = engine.projectId;
    await engine.close();
    expect((await rejection(engine.head("main"))).code).toBe("ENGINE_CLOSED");
    engine = await Engine.open(serverData, projectDir);
    expect(engine.projectId).toBe(id);
    expect(JSON.parse(await engine.snapshot("main"))).toEqual(snap);
  });

  test("a conflicting stale commit rejects with CONFLICT and details.againstTx", async () => {
    engine = await Engine.create(serverData, projectDir, "Demo", "30");
    await engine.commit(tx("tx-asset", 0, [addAsset]));
    await engine.commit(tx("tx-base", 1, [insert(0, 5)]));
    await engine.commit(tx("tx-del", 2, [{ op: "clip.delete", clip: "c1" }]));

    const err = await rejection(engine.commit(tx("tx-late", 2, [{ op: "clip.delete", clip: "c1" }])));
    expect(err).toBeInstanceOf(Error);
    expect(err.code).toBe("CONFLICT");
    expect(err.details.againstTx).toBe("tx-del");
    expect(err.details.txId).toBe("tx-late");
    expect(err.message).not.toContain("FIN_CORE_ERROR");
    expect(await engine.head("main")).toBe(3);
  });

  test("errors carry code, message, details and fix", async () => {
    const bad = await rejection(Engine.create(serverData, projectDir, "Demo", "29.97"));
    expect(bad.code).toBe("INVALID_TIME");
    expect(bad.message).toContain("29.97");

    engine = await Engine.create(serverData, projectDir, "Demo", "30");
    const exists = await rejection(Engine.create(serverData, projectDir, "Demo", "30"));
    expect(exists.code).toBe("PROJECT_EXISTS");
    expect(typeof exists.fix).toBe("string");

    expect((await rejection(engine.commit("{not json"))).code).toBe("INVALID_OP");
    expect((await rejection(engine.opsSince("main", -1))).code).toBe("INVALID_RANGE");
    expect((await rejection(engine.snapshot("nope"))).code).toBe("BRANCH_NOT_FOUND");
    expect((await rejection(Engine.open(serverData, join(tmp, "empty")))).code).toBe("PROJECT_NOT_FOUND");
  });

  test("the event loop keeps ticking during 500 commits", async () => {
    engine = await Engine.create(serverData, projectDir, "Demo", "30");
    await engine.commit(tx("tx-asset", 0, [addAsset]));

    const gaps: number[] = [];
    let last = performance.now();
    const timer = setInterval(() => {
      const now = performance.now();
      gaps.push(now - last);
      last = now;
    }, 1);
    try {
      for (let i = 0; i < 500; i++) {
        await engine.commit(tx(`tx-${i}`, i + 1, [insert(i)]));
      }
    } finally {
      clearInterval(timer);
    }
    expect(await engine.head("main")).toBe(501);
    expect(gaps.length).toBeGreaterThan(10);
    expect(Math.max(...gaps)).toBeLessThan(20);
  }, 120_000);
});

describe("probe", () => {
  test("a missing path rejects with ASSET_NOT_FOUND", async () => {
    const err = await rejection(probe(join(tmp, "missing.mp4")));
    expect(err.code).toBe("ASSET_NOT_FOUND");
    expect(err.details.path).toBe(join(tmp, "missing.mp4"));
  });

  test("a non-MP4 file rejects with UNSUPPORTED_CONTAINER", async () => {
    const file = join(tmp, "notes.txt");
    writeFileSync(file, "not a movie");
    expect((await rejection(probe(file))).code).toBe("UNSUPPORTED_CONTAINER");
  });
});
