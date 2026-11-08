// The library service after the Tasks 9–11 review (plan 5D Decisions 17, 18, 28, 30): watches
// stop offline and restart online, the owner's accept is refused on an empty root and guarded
// against a mass drop, targeted scans are merged per library, freshness listings are bounded,
// nothing is submitted offline, and a job that ends before its submit answers does not stick.
// A fake engine and a manual clock (`support/service.ts`).
import type { JobView } from "@finisher/contracts";
import { afterEach, describe, expect, test } from "vitest";
import { acceptEmptyRoot, acceptUnmounted, acceptVolume, scanLibrary } from "../../src/library/admin.ts";
import { MAX_MERGED_DIRS } from "../../src/library/dirs-queue.ts";
import { freshen } from "../../src/library/fresh.ts";
import { ACTOR, cleanupHarnesses, type Harness, harness, identity, settle } from "./support/service.ts";

const S = 1000;
const MIN = 60 * S;

const open: Harness[] = [];
afterEach(() => {
  for (const h of open.splice(0)) h.close();
  cleanupHarnesses();
});

async function rig(): Promise<Harness> {
  const h = await harness();
  open.push(h);
  return h;
}

const scans = (h: Harness, mode?: string) => h.submits.filter((s) => s.kind === "library.scan" && (mode === undefined || s.mode === mode));
const dir = (name: string) => ({ components: [Buffer.from(name).toString("base64")] });
const offlineJob = (lib: string, state: unknown): JobView =>
  ({ id: "jx", kind: "library.probe", family: "library", library: lib, params: { library: lib }, state: "failed", error: { code: "LIBRARY_OFFLINE", message: "", details: { state } } }) as unknown as JobView;

async function finishAll(h: Harness): Promise<void> {
  for (const j of h.active()) await h.finish(j.id);
}

describe("watches and offline", () => {
  test("offline_stops_the_watch_and_online_restarts_it", async () => {
    const h = await rig();
    const lib = await h.add("/data/media", {}, "local");
    await finishAll(h);
    expect(h.svc.status(lib.id).watch).toBe("ok");
    h.probeAnswers = [{ state: "unmounted" }];
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "unmounted" }));
    await settle();
    expect(h.registry.get(lib.id)!.state).toBe("offline");
    expect(h.unwatches).toEqual([lib.id]);
    expect(h.svc.status(lib.id).watch).toBe("none");
    // A degraded watch is forgotten too: back online, it is tried again.
    await h.time.advance(60 * S);
    expect(h.registry.get(lib.id)!.state).toBe("scanning");
    expect(h.watches).toEqual([lib.id, lib.id]);
    expect(h.svc.status(lib.id).watch).toBe("ok");
  });

  test("nothing_is_submitted_offline_and_failures_keep_the_probe_cadence", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    h.probeAnswers = Array.from({ length: 10 }, () => ({ state: "timeout" as const }));
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "timeout" }));
    await settle();
    expect(h.probes).toHaveLength(1);
    const before = h.submits.length;
    expect(await h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir("a")], trigger: "demand" })).toBeUndefined();
    expect(await h.svc.submitScan(lib.id, { mode: "poll", trigger: "demand" })).toBeUndefined();
    h.svc.kindEvents.rescan?.(lib.id, [dir("b")]);
    await settle();
    expect(h.submits).toHaveLength(before);
    // Probe or hash jobs failing `LIBRARY_OFFLINE` meanwhile do not start more re-probes.
    for (let i = 0; i < 5; i++) h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "timeout" }));
    await settle();
    expect(h.probes).toHaveLength(1);
    await h.time.advance(60 * S);
    expect(h.probes).toHaveLength(2);
  });
});

describe("accepting a volume", () => {
  test("accept_refused_on_an_unmounted_or_empty_root_unless_forced", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    for (const state of ["unmounted", "empty"] as const) {
      h.probeAnswers = [{ state }];
      await expect(acceptVolume(h.svc, lib.id, ACTOR)).rejects.toMatchObject({ code: "LIBRARY_ROOT_SUSPECT", details: { state } });
    }
    expect(h.dropGuards).toEqual([]);
    h.probeAnswers = [{ state: "empty" }];
    h.measured = identity("cc");
    await acceptVolume(h.svc, lib.id, ACTOR, { force: true });
    expect(h.registry.get(lib.id)!.volume.key).toBe("cc");
    // Forced: the owner confirmed, so no guard.
    expect(h.dropGuards).toEqual([{ lib: lib.id, on: false }]);
  });

  test("mass_drop_after_accept_is_held_until_confirmed", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    h.measured = identity("bb");
    await acceptVolume(h.svc, lib.id, ACTOR);
    expect(h.dropGuards.at(-1)).toEqual({ lib: lib.id, on: true });
    // The first full scan's root listing would drop most of the library: held, offline.
    const full = h.active().find((j) => j.mode === "full")!;
    await h.finish(full.id, "failed", { error: { code: "ROOT_DROP_SUSPECT", message: "", details: {} } });
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_DROP_SUSPECT" });
    expect(h.svc.view(h.registry.get(lib.id)!).warnings).toContainEqual({ code: "ROOT_DROP_SUSPECT" });
    // Re-probes answer online, but only the owner lifts it.
    await h.time.advance(5 * MIN);
    expect(h.registry.get(lib.id)!.state).toBe("offline");
    expect(scans(h, "full")).toHaveLength(1);
    await acceptVolume(h.svc, lib.id, ACTOR, { force: true });
    expect(h.dropGuards.at(-1)).toEqual({ lib: lib.id, on: false });
    expect(h.registry.get(lib.id)!.state).toBe("scanning");
    await finishAll(h);
    expect(h.registry.get(lib.id)!.state).toBe("online");
  });

  test("a_successful_full_scan_lifts_the_guard", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    await acceptVolume(h.svc, lib.id, ACTOR);
    expect(h.dropGuards.at(-1)).toEqual({ lib: lib.id, on: true });
    await finishAll(h);
    expect(h.dropGuards.at(-1)).toEqual({ lib: lib.id, on: false });
  });

  test("catalog_reasons_survive_go_online_and_accept", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    h.registry.setState(lib.id, "offline", "CATALOG_TOO_NEW");
    await h.svc.goOnline(lib.id);
    await acceptVolume(h.svc, lib.id, ACTOR);
    await h.time.advance(5 * MIN);
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "CATALOG_TOO_NEW" });
    expect(scans(h, "full")).toHaveLength(0);
    // A later offline cause does not replace it either.
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "timeout" }));
    await settle();
    expect(h.registry.get(lib.id)!.offlineReason).toBe("CATALOG_TOO_NEW");
  });

  test("accept_unmounted_lists_the_former_mount_points", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    const r = await acceptUnmounted(h.svc, lib.id);
    expect(r.dirs).toEqual([{ components: ["ZA=="] }]);
    expect(r.job).toBeDefined();
    expect(scans(h, "dirs")).toEqual([expect.objectContaining({ id: r.job!.id, mode: "dirs" })]);
    await expect(acceptUnmounted(h.svc, "01none")).rejects.toMatchObject({ code: "LIBRARY_NOT_FOUND" });
    // Offline: the mounts are forgotten, but nothing is listed until the library is back.
    await finishAll(h);
    h.registry.setState(lib.id, "offline", "ROOT_UNREACHABLE");
    expect((await acceptUnmounted(h.svc, lib.id)).job).toBeUndefined();
    expect(scans(h, "dirs")).toHaveLength(1);
  });
});

describe("targeted scans", () => {
  test("dirs_scans_merge_into_one_queued_job_per_library", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    const a = await h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir("a")], trigger: "watch" });
    // The same directory again: the queued job answers.
    expect((await h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir("a")], trigger: "watch" }))?.id).toBe(a!.id);
    expect(scans(h, "dirs")).toHaveLength(1);
    // A new one: the queued job is replaced by one listing both, at the higher priority.
    const ab = await h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir("b")], trigger: "demand", priority: "interactive" });
    expect(ab!.id).not.toBe(a!.id);
    expect((ab!.params as { dirs: unknown[] }).dirs).toEqual([dir("a"), dir("b")]);
    expect(ab!.priority).toBe("interactive");
    expect(h.active().filter((j) => j.mode === "dirs")).toHaveLength(1);
    await settle();
    // The merged-away job is not a scan of its own: nothing journaled for it.
    expect(h.journal).toHaveLength(0);
    // Concurrent requests are serialized into one job too.
    await Promise.all(["c", "d", "e"].map((n) => h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir(n)], trigger: "watch" })));
    const queued = h.active().filter((j) => j.mode === "dirs");
    expect(queued).toHaveLength(1);
    // Too many directories at once: a poll instead.
    const many = Array.from({ length: MAX_MERGED_DIRS + 1 }, (_, i) => dir(`d${i}`));
    const poll = await h.svc.submitScan(lib.id, { mode: "dirs", dirs: many, trigger: "watch" });
    expect((poll!.params as { mode: string }).mode).toBe("poll");
    expect(h.active().filter((j) => j.mode === "dirs")).toHaveLength(0);
  });

  test("freshness_dirs_scans_are_bounded", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    await h.time.advance(10 * MIN); // the last poll is older than 5 minutes
    const at = async () => ({ components: [Buffer.from("x").toString("base64")] });
    const pass = freshen(h.svc, [lib.id], at);
    await settle();
    expect(scans(h, "dirs")).toHaveLength(1);
    await finishAll(h);
    expect((await pass).stale).toBe(false);
    // Listed by a targeted scan just now: fresh, nothing submitted.
    expect((await freshen(h.svc, [lib.id], at)).stale).toBe(false);
    expect(scans(h, "dirs")).toHaveLength(1);
    // Past 5 minutes it is listed again, but at most once a minute while that job is pending.
    await h.time.advance(6 * MIN);
    h.svc.use([lib.id]); // keep it from a schedule poll racing the assertions
    const second = freshen(h.svc, [lib.id], at);
    await settle();
    expect(scans(h, "dirs")).toHaveLength(2);
    expect((await freshen(h.svc, [lib.id], at)).stale).toBe(true);
    expect(scans(h, "dirs")).toHaveLength(2);
    await finishAll(h);
    await second;
  }, 20_000);

  test("library_scan_dirs_answers_a_job_that_lists_them", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    await h.svc.submitScan(lib.id, { mode: "dirs", dirs: [dir("a")], trigger: "watch" });
    const job = await scanLibrary(h.svc, lib.id, { dirs: [dir("b")] });
    expect((job.params as { dirs: unknown[] }).dirs).toEqual([dir("a"), dir("b")]);
  });
});

describe("submit and end", () => {
  test("a_scan_that_ends_before_its_submit_answers_does_not_stick", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    h.endOnSubmit = true;
    const view = await h.svc.submitScan(lib.id, { mode: "full", trigger: "demand" });
    await settle();
    expect(view?.state).toBe("queued");
    expect(h.svc.status(lib.id).jobs).toEqual([]);
    expect(h.registry.get(lib.id)!.state).toBe("online");
    h.endOnSubmit = false;
    // The next full scan is not blocked by a job that is already over.
    await h.svc.submitScan(lib.id, { mode: "full", trigger: "demand" });
    expect(scans(h, "full")).toHaveLength(2);
  });
});

describe("accepting an empty root", () => {
  test("accept_empty_root_needs_an_empty_root_and_brings_the_library_back", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await finishAll(h);
    // An unmounted, changed or unreachable root is not an empty one: refused, nothing set.
    for (const state of ["unmounted", "timeout"] as const) {
      h.probeAnswers = [{ state }];
      await expect(acceptEmptyRoot(h.svc, lib.id, ACTOR)).rejects.toMatchObject({ code: "LIBRARY_ROOT_SUSPECT", details: { state } });
    }
    expect(h.emptyRoots).toEqual([]);
    await expect(acceptEmptyRoot(h.svc, "01none", ACTOR)).rejects.toMatchObject({ code: "LIBRARY_NOT_FOUND" });
    // The scan that found the root empty put the library offline.
    h.probeAnswers = [{ state: "empty" }];
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "empty" }));
    await settle();
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_EMPTY_SUSPECT" });
    const before = scans(h, "full").length;
    h.probeAnswers = [{ state: "empty" }];
    const view = await acceptEmptyRoot(h.svc, lib.id, ACTOR);
    expect(h.emptyRoots).toEqual([{ lib: lib.id, on: true }]);
    expect(view.state).toBe("scanning");
    expect(scans(h, "full")).toHaveLength(before + 1);
  });
});
