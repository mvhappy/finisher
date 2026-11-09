// The library kinds on the shared engine (plan 5D Task 9; Decisions 26, 27, 32–34, 38): a real
// engine, the test-hooks `fin-workers`, real catalog threads and overlay roots.
import { appendFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { JobView } from "@finisher/contracts";
import type { LibraryScanResult } from "@finisher/core-types";
import { libraryProbeVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { probeParams } from "../../src/library/kind-params.ts";
import { libraryPools } from "../../src/library/pools.ts";
import { OWNER, run, until } from "../support/jobs.ts";
import { closeRigs, counts, FAST, grid, libraryRig, overlay, pendingIds, rootsDir, tree } from "./support/kinds.ts";

afterEach(closeRigs, 60_000);

const WAIT = 60_000;

async function probeAll(rig: Awaited<ReturnType<typeof libraryRig>>, lib: Parameters<typeof probeParams>[0]): Promise<JobView> {
  const ids = await pendingIds(rig, lib.id);
  const params = probeParams(lib, ids)!;
  const v = await run(rig.t.engine.submit(OWNER, { kind: "library.probe", params, library: lib.id }));
  return rig.t.wait(v.id, WAIT);
}

describe("library.scan", () => {
  test("full_scan_of_10k_tree_lands_in_catalog", { timeout: 120_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "big");
    tree(root, grid(100, 100));
    const rig = await libraryRig();
    const lib = await rig.library(root);
    const v = await rig.t.wait((await rig.scan(lib, { timing: { ...FAST, flush: 5000 } })).id, WAIT);
    expect(v.state).toBe("succeeded");
    const result = v.result as LibraryScanResult;
    expect(result.listed).toBe(101);
    expect(result.entries).toBe(10_100);
    const c = await counts(rig, lib.id);
    expect(c.files.present).toBe(10_000);
    expect(c.files.missing ?? 0).toBe(0);
    expect(c.dirs.present).toBe(100); // the root row is not counted
    expect(rig.seen.batches.length).toBeGreaterThanOrEqual(3); // ≤ 4,096 entries per batch file
    expect(rig.kinds.live()).toBe(0);
  });

  test("scan_resumes_after_finisherd_restart_without_relisting", { timeout: 120_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "slow");
    tree(root, grid(20, 5));
    const paths: Record<string, unknown> = {};
    for (let d = 0; d < 20; d++) paths[`d${String(d).padStart(2, "0")}`] = { readDelayMs: 250 };
    overlay(root, { paths });
    let rig = await libraryRig();
    const lib = await rig.library(root);
    const job = (await rig.scan(lib, { threads: 1 })).id;
    // Crash once some listings landed (each batch flushes within 50 ms of its listing).
    await until(() => rig.seen.batches.length >= 6, WAIT);
    await rig.crash();
    rig = await libraryRig({ serverData: rig.serverData });
    const v = await rig.t.wait(job, WAIT);
    expect(v.state).toBe("succeeded");
    expect(v.attempts).toBeGreaterThanOrEqual(2);
    const result = v.result as LibraryScanResult;
    // The restarted attempt skipped what the first one had listed (and still descended it).
    expect(result.listed).toBeLessThan(21);
    expect(result.listed).toBeGreaterThan(0);
    const c = await counts(rig, lib.id);
    expect(c.files.present).toBe(100);
    expect(c.files.missing ?? 0).toBe(0);
  });

  test("poll_job_touches_only_changed_dirs", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "poll");
    tree(root, grid(10, 3));
    const rig = await libraryRig();
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    // A new file in d03 moves that directory's mtime only.
    await new Promise((r) => setTimeout(r, 20));
    appendFileSync(join(root, "d03", "new.bin"), "x");
    const v = await rig.t.wait((await rig.scan(lib, { mode: "poll" })).id, WAIT);
    expect(v.state).toBe("succeeded");
    const result = v.result as LibraryScanResult;
    expect(result.listed).toBe(1);
    expect(result.unchanged).toBeGreaterThanOrEqual(10);
    const c = await counts(rig, lib.id);
    expect(c.files.present).toBe(31);
  });

  test("poll_runs_beside_a_full_scan", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "beside");
    tree(root, grid(6, 2));
    const rig = await libraryRig();
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    const paths: Record<string, unknown> = {};
    for (let d = 0; d < 6; d++) paths[`d${String(d).padStart(2, "0")}`] = { readDelayMs: 2000 };
    overlay(root, { paths });
    const full = await rig.scan(lib, { threads: 1, gen: 2 });
    await until(() => rig.t.jobs.get(full.id)?.state === "running");
    const poll = await rig.t.wait((await rig.scan(lib, { mode: "poll" })).id, WAIT);
    expect(poll.state).toBe("succeeded");
    expect(rig.t.jobs.get(full.id)?.state).toBe("running");
    const pools = libraryPools(lib.volume.key);
    const view = rig.t.engine.workers().pools;
    expect(view.find((p) => p.name === pools.scan)).toMatchObject({ size: 1, used: 1 });
    expect(view.find((p) => p.name === pools.poll)).toMatchObject({ size: 1, used: 0 });
    await run(rig.t.engine.cancel(full.id));
  });

  test("offline_mid_scan_marks_nothing_missing", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "dying");
    tree(root, grid(10, 3));
    const rig = await libraryRig();
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    // Halfway through the next scan the root stops answering (every read thread stuck): files
    // the directories it never listed no longer hold must not be marked missing.
    rmSync(join(root, "d07", "f000.bin"));
    for (const f of ["f000.bin", "f001.bin", "f002.bin"]) rmSync(join(root, "d08", f));
    const paths: Record<string, unknown> = {};
    for (let d = 5; d < 10; d++) paths[`d${String(d).padStart(2, "0")}`] = { readDelayMs: 5000 };
    overlay(root, { paths });
    const v = await rig.t.wait((await rig.scan(lib, { threads: 1, gen: 2, timing: { ...FAST, entry: 300 } })).id, WAIT);
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("LIBRARY_OFFLINE");
    expect(v.attempts).toBe(1); // retry: never
    expect(rig.seen.offline).toEqual([{ lib: lib.id, job: v.id }]);
    const c = await counts(rig, lib.id);
    expect(c.files.missing ?? 0).toBe(0);
    expect(c.files.present).toBe(30);
    expect(c.dirs.missing ?? 0).toBe(0);
  });
});

describe("library.probe and library.hash", () => {
  test("probe_poison_file_isolated_and_recorded", { timeout: 120_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "poison");
    tree(root, { "": ["p1.png", "p2.png", "p3.png", "p4.png", "p5.png", "p6.png"] });
    overlay(root, { paths: { "p3.png": { crashOnOpen: true } } });
    const rig = await libraryRig();
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    const v = await probeAll(rig, lib);
    expect(v.state).toBe("succeeded");
    const errors = v.itemErrors ?? [];
    expect(errors).toHaveLength(1);
    expect(errors[0]?.error.code).toBe("POISON_ITEM");
    const poisonId = Number(errors[0]?.item);
    const poison = await run(rig.catalogs.reader.file(lib.id, { fileId: poisonId }));
    expect(poison.matches[0]).toMatchObject({ nameNfc: "p3.png", state: "probeFailed", code: "POISON_ITEM", probeVersion: libraryProbeVersion() });
    const rows = await run(rig.catalogs.reader.files(lib.id, { limit: 500 }));
    const good = rows.files.filter((f) => f.id !== poisonId);
    expect(good).toHaveLength(5);
    for (const f of good) expect(f).toMatchObject({ state: "present", kind: "image", width: 1, height: 1, probeVersion: libraryProbeVersion() });
    for (const f of good) expect(f.fp).toMatch(/\S/);
    // Nothing is left for the probe stage: the poisoned file waits for a change.
    expect(await pendingIds(rig, lib.id)).toEqual([]);
    expect((v.result as { states: Record<string, number> }).states.present).toBe(5);
  });

  test("resource_unhealthy_never_poisons", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "sick");
    tree(root, { "": ["a.png", "b.png"] });
    overlay(root, { paths: { "a.png": { crashOnOpen: true } } });
    let healthy = true;
    const rig = await libraryRig({ online: () => Effect.sync(() => healthy) });
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    healthy = false;
    const ids = await pendingIds(rig, lib.id);
    const v = await run(rig.t.engine.submit(OWNER, { kind: "library.probe", params: probeParams(lib, ids)!, library: lib.id }));
    // The crash on an unhealthy root is a resource fault: backoff, never isolation.
    const queued = await until(() => {
      const j = rig.t.jobs.get(v.id);
      return j?.state === "queued" && j.nextRunAt !== undefined ? j : undefined;
    }, WAIT);
    expect(queued.attempts).toBe(1);
    expect([...rig.t.jobs.items(v.id).values()].some((o) => "error" in o && o.error.code === "POISON_ITEM")).toBe(false);
    expect((await run(rig.catalogs.reader.files(lib.id, { limit: 500 }))).files.every((f) => f.code !== "POISON_ITEM")).toBe(true);
    await run(rig.t.engine.cancel(v.id));
  });

  test("hash_items_land_in_the_catalog", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const root = join(roots, "hash");
    tree(root, { "": ["a.bin", "b.bin"] }, 1000);
    const rig = await libraryRig();
    const lib = await rig.library(root);
    expect((await rig.t.wait((await rig.scan(lib, {})).id, WAIT)).state).toBe("succeeded");
    const ids = await pendingIds(rig, lib.id);
    const { hashParams } = await import("../../src/library/kind-params.ts");
    const v = await rig.t.wait((await run(rig.t.engine.submit(OWNER, { kind: "library.hash", params: hashParams(lib, ids)!, library: lib.id }))).id, WAIT);
    expect(v.state).toBe("succeeded");
    const rows = (await run(rig.catalogs.reader.files(lib.id, { limit: 500 }))).files;
    expect(rows.map((r) => r.fullHash)).toEqual([expect.stringMatching(/^blake3:[0-9a-f]{64}$/), expect.stringMatching(/^blake3:[0-9a-f]{64}$/)]);
    expect(rows[0]?.fullHash).toBe(rows[1]?.fullHash); // same bytes
  });
});

describe("pools", () => {
  test("pools_shared_per_volume", { timeout: 60_000 }, async () => {
    const roots = rootsDir();
    const a = join(roots, "a");
    const b = join(roots, "b");
    tree(a, grid(4, 1));
    tree(b, grid(1, 1));
    const paths: Record<string, unknown> = {};
    for (let d = 0; d < 4; d++) paths[`d${String(d).padStart(2, "0")}`] = { readDelayMs: 2000 };
    overlay(a, { paths });
    const rig = await libraryRig();
    const la = await rig.library(a);
    const lb = await rig.library(b, { name: "B" });
    expect(la.volume.key).toBe(lb.volume.key);
    expect(rig.book.pools(la.id)).toEqual(rig.book.pools(lb.id));
    const ja = await rig.scan(la, { threads: 1 });
    await until(() => rig.t.jobs.get(ja.id)?.state === "running");
    const jb = await rig.scan(lb, {});
    // One full scan per volume: B's waits for A's slot.
    await new Promise((r) => setTimeout(r, 300));
    expect(rig.t.jobs.get(jb.id)?.state).toBe("queued");
    await run(rig.t.engine.cancel(ja.id));
    expect((await rig.t.wait(jb.id, WAIT)).state).toBe("succeeded");
  });
});
