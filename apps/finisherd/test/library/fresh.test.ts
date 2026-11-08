// Freshness before answers (plan 5D Decisions 28, 46; Task 12's freshness part, after Task 10)
// and offline answers from workers, over a real engine: a search or listing of a network
// library whose last poll is older than 5 minutes waits ≤ 5 s for an `interactive` poll, then
// answers `stale: true`; a full scan on the volume never holds the poll up.
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { addLibrary, removeLibrary } from "../../src/library/admin.ts";
import { filesFresh, searchFresh } from "../../src/library/fresh.ts";
import { searchLibraries } from "../../src/library/search.ts";
import { exec } from "../../src/library/service.ts";
import { run, until } from "../support/jobs.ts";
import { grid, overlay, tree } from "./support/kinds.ts";
import { closeLiveRigs, type LiveRig, liveRig, liveRoots } from "./support/live.ts";

afterEach(closeLiveRigs, 60_000);

const ACTOR = { kind: "local", id: "test" } as const;
const LONG_AGO = 10 * 60_000;

async function networkLibrary(rig: LiveRig, root: string): Promise<string> {
  const { library, job } = await addLibrary(rig.svc, { root, storage: "network" }, ACTOR);
  expect((await rig.t.wait(job!.id, 30_000)).state).toBe("succeeded");
  await until(() => rig.svc.status(library.id).jobs.length === 0);
  return library.id;
}

/** Makes the last poll 10 minutes old. */
const backdate = (rig: LiveRig, id: string) => rig.svc.registry.touch(id, { lastPollAt: Date.now() - LONG_AGO, lastFullAt: Date.now() - LONG_AGO });

const polls = (rig: LiveRig, id: string) =>
  rig.t.jobs.list({ kind: "library.scan", library: id }).jobs.filter((j) => (j.params as { mode: string }).mode !== "full");

describe("freshness", () => {
  test("search_waits_for_freshness_then_stale", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(2, 2, "mov"));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    // Fresh enough: no pass.
    const quiet = await searchFresh(rig.svc, { libraries: [id], query: "f000" });
    expect(quiet.stale).toBe(false);
    expect(polls(rig, id)).toHaveLength(0);
    // Ten minutes old: the search waits for an interactive poll and sees the new file.
    backdate(rig, id);
    writeFileSync(join(root, "fresh_take.mov"), "x");
    const fresh = await searchFresh(rig.svc, { libraries: [id], query: "fresh_take" });
    expect(fresh.stale).toBe(false);
    expect(fresh.hits.map((h) => h.relKey)).toEqual(["fresh_take.mov"]);
    expect(polls(rig, id)).toHaveLength(1);
    expect(polls(rig, id)[0]!.priority).toBe("interactive");
    expect(fresh.lastPollAt).toBeGreaterThan(Date.now() - 60_000);
    // A root that takes 8 s to answer: the search answers after ≈ 5 s, stale, and the poll goes on.
    overlay(root, { statDelayMs: 8000 });
    backdate(rig, id);
    writeFileSync(join(root, "later_take.mov"), "x");
    const t0 = Date.now();
    const stale = await searchFresh(rig.svc, { libraries: [id], query: "later_take" });
    const took = Date.now() - t0;
    expect(stale.stale).toBe(true);
    expect(stale.hits).toHaveLength(0);
    expect(took).toBeGreaterThanOrEqual(4_900);
    expect(took).toBeLessThan(7_500);
    await until(async () => (await run(searchLibraries(rig.catalogs.reader, { libraries: [id], query: "later_take" }))).hits.length === 1, 30_000);
  });

  test("freshness_pass_bounded_5s_then_stale", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(3, 2));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    // A listing lists exactly its directory (a targeted scan); this one takes 8 s.
    overlay(root, { paths: { d01: { readDelayMs: 8000 } } });
    backdate(rig, id);
    const t0 = Date.now();
    const page = await filesFresh(rig.svc, id, { dir: "d01", limit: 50 });
    const took = Date.now() - t0;
    expect(page.stale).toBe(true);
    expect(page.offline).toBe(false);
    expect(page.files).toHaveLength(2);
    expect(took).toBeGreaterThanOrEqual(4_900);
    expect(took).toBeLessThan(7_500);
    const [pass] = polls(rig, id);
    expect(pass).toMatchObject({ priority: "interactive", params: { mode: "dirs" } });
    expect((pass!.params as { dirs: Array<{ components: string[] }> }).dirs).toEqual([{ components: [Buffer.from("d01").toString("base64")] }]);
    expect((await rig.t.wait(pass!.id, 30_000)).state).toBe("succeeded");
  });

  test("freshness_pass_not_blocked_by_full_scan", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(4, 2));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    const paths: Record<string, unknown> = {};
    for (let d = 0; d < 4; d++) paths[`d0${d}`] = { readDelayMs: 20_000 };
    overlay(root, { paths });
    const full = await rig.svc.submitScan(id, { mode: "full", trigger: "demand" });
    await until(() => rig.t.jobs.get(full!.id)?.state === "running");
    backdate(rig, id);
    writeFileSync(join(root, "beside.mov"), "x");
    const t0 = Date.now();
    const answer = await searchFresh(rig.svc, { libraries: [id], query: "beside" });
    expect(answer.stale).toBe(false);
    expect(answer.hits).toHaveLength(1);
    expect(Date.now() - t0).toBeLessThan(5_000);
    expect(rig.t.jobs.get(full!.id)?.state).toBe("running");
    await run(rig.t.engine.cancel(full!.id));
  });
});

describe("offline from workers", () => {
  test("root_empty_listing_puts_library_offline", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(2, 2));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    for (const d of ["d00", "d01"]) rmSync(join(root, d), { recursive: true });
    const job = await rig.svc.submitScan(id, { mode: "full", trigger: "demand" });
    const v = await rig.t.wait(job!.id, 60_000);
    expect(v.state).toBe("failed");
    await until(() => rig.svc.registry.get(id)?.state === "offline");
    expect(rig.svc.registry.get(id)!.offlineReason).toBe("ROOT_EMPTY_SUSPECT");
    // Nothing was marked missing, and searches say the hits are offline.
    const counts = await exec(rig.catalogs.reader.counts(id));
    expect(counts.files.missing ?? 0).toBe(0);
    const found = await searchFresh(rig.svc, { libraries: [id], query: "f000" });
    expect(found.hits.length).toBeGreaterThan(0);
    expect(found.hits.every((h) => h.offline)).toBe(true);
    // The walker's first stat saw the empty root (Decision 17's `empty`): the job says LIBRARY_OFFLINE.
    const failed = await until(() => rig.journal.find((j) => j.payload.outcome === "failed"));
    expect(failed.payload).toMatchObject({ full: true, outcome: "failed", code: "LIBRARY_OFFLINE" });
  });

  test("unreachable_root_from_a_scan_goes_offline", { timeout: 60_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(1, 2));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    rmSync(root, { recursive: true });
    const job = await rig.svc.submitScan(id, { mode: "poll", trigger: "demand" });
    const v = await rig.t.wait(job!.id, 30_000);
    expect(v.error?.code).toBe("LIBRARY_OFFLINE");
    await until(() => rig.svc.registry.get(id)?.state === "offline");
    expect(rig.svc.registry.get(id)!.offlineReason).toBe("ROOT_UNREACHABLE");
    expect(await run(rig.svc.online(id))).toBe(false);
  });
});

describe("owner actions", () => {
  test("remove_keeps_vectors_dir", { timeout: 60_000 }, async () => {
    const root = join(liveRoots(), "nas");
    tree(root, grid(1, 2));
    const rig = await liveRig();
    const id = await networkLibrary(rig, root);
    const dir = join(rig.serverData, "libraries", id);
    mkdirSync(join(dir, "vectors"));
    writeFileSync(join(dir, "vectors", "index.bin"), "v");
    await removeLibrary(rig.svc, rig.serverData, id, ACTOR);
    expect(existsSync(join(dir, "catalog.db"))).toBe(false);
    expect(readdirSync(dir)).toEqual(["vectors"]);
    expect(rig.svc.registry.get(id)).toBeUndefined();
    expect(() => rig.svc.pools(id)).toThrow();
    expect(rig.svc.list()).toEqual([]);
  });
});
