// The library service's scheduler, offline rules and journal (plan 5D Task 10; Decisions 17,
// 18, 28–30, 50) over a fake engine and a manual clock (`support/service.ts`).
import { mkdtempSync, rmSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import type { Envelope, ExportPage } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { Journal } from "@finisher/fin-node";
import { afterEach, describe, expect, test } from "vitest";
import { acceptVolume } from "../../src/library/admin.ts";
import { emptyCounts } from "../../src/library/catalog/core/apply.ts";
import { libraryScanPayload, log2Bucket, recordScan } from "../../src/library/events.ts";
import { librariesHolding, underRoot } from "../../src/library/layer.ts";
import { ACTOR, cleanupHarnesses, type Harness, harness, identity, settle } from "./support/service.ts";

const S = 1000;
const MIN = 60 * S;
const H = 60 * MIN;

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

/** Ends every active job of `lib` as succeeded. */
async function finishAll(h: Harness): Promise<void> {
  for (const j of h.active()) await h.finish(j.id);
}

const offlineJob = (lib: string, state: unknown): JobView =>
  ({ id: "jx", kind: "library.probe", family: "library", library: lib, params: { library: lib }, state: "failed", error: { code: "LIBRARY_OFFLINE", message: "", details: { state } } }) as unknown as JobView;

describe("demand polling", () => {
  test("demand_cadence_with_injected_clock", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await h.time.advance(59 * S);
    expect(scans(h)).toHaveLength(0);
    // A project referencing it opens: polled every 60 s.
    const hold = h.svc.use([lib.id]);
    await h.time.advance(1 * S);
    expect(scans(h, "poll")).toHaveLength(1);
    await finishAll(h);
    await h.time.advance(59 * S);
    expect(scans(h, "poll")).toHaveLength(1);
    await h.time.advance(1 * S);
    expect(scans(h, "poll")).toHaveLength(2);
    await finishAll(h);
    // Released: back to 6 hours.
    hold.release();
    await h.time.advance(10 * MIN);
    expect(scans(h, "poll")).toHaveLength(2);
    // A watch stream holds it the same way; a search keeps it in use for 10 minutes.
    h.svc.usage.touch(lib.id);
    await h.time.advance(60 * S);
    expect(scans(h, "poll")).toHaveLength(3);
    for (let i = 0; i < 12; i++) {
      await finishAll(h);
      await h.time.advance(60 * S);
    }
    // Polled each minute until 10 minutes after the touch, then not.
    const n = scans(h, "poll").length;
    expect(n).toBeGreaterThanOrEqual(11);
    expect(n).toBeLessThanOrEqual(13);
    await finishAll(h);
    await h.time.advance(30 * MIN);
    expect(scans(h, "poll")).toHaveLength(n);
  });

  test("unused_library_polls_every_6h", async () => {
    const h = await rig();
    await h.add("/nas/a");
    await h.time.advance(6 * H - S);
    expect(scans(h)).toHaveLength(0);
    await h.time.advance(S);
    expect(scans(h, "poll")).toHaveLength(1);
    await finishAll(h);
    await h.time.advance(6 * H - S);
    expect(scans(h, "poll")).toHaveLength(1);
    await h.time.advance(S);
    expect(scans(h, "poll")).toHaveLength(2);
    // One timer, set for the next tick: the NAS is not woken in between.
    await finishAll(h);
    expect((h.time.next() ?? 0) - h.time.now).toBe(6 * H);
  });

  test("daily_full_runs_at_first_tick_after_due", async () => {
    const h = await rig();
    await h.add("/nas/a");
    // Shift the ticks off the day boundary: used for a minute at 5 h.
    await h.time.advance(5 * H);
    const hold = h.svc.use(h.registry.list().map((l) => l.id));
    await h.time.advance(0);
    expect(scans(h, "poll")).toHaveLength(1);
    await finishAll(h);
    hold.release();
    // Ticks now at 11 h, 17 h, 23 h, 29 h: the full scan was due at 24 h and waits for 29 h.
    for (const at of [11, 17, 23]) {
      await h.time.advance(at * H - (h.time.now - (h.registry.list()[0]!.createdAt)));
      await finishAll(h);
    }
    expect(scans(h, "full")).toHaveLength(0);
    await h.time.advance(5 * H);
    expect(scans(h, "full")).toHaveLength(0);
    await h.time.advance(1 * H);
    expect(scans(h, "full")).toHaveLength(1);
    expect(scans(h, "poll")).toHaveLength(4);
    await finishAll(h);
    expect(h.registry.list()[0]!.lastFullAt).toBe(h.time.now);
  });

  test("fixed_interval_override", async () => {
    const h = await rig();
    await h.add("/nas/a", { scan: { poll: 120 } });
    await h.time.advance(119 * S);
    expect(scans(h)).toHaveLength(0);
    await h.time.advance(S);
    expect(scans(h, "poll")).toHaveLength(1);
    await finishAll(h);
    await h.time.advance(120 * S);
    expect(scans(h, "poll")).toHaveLength(2);
  });
});

describe("offline and back", () => {
  test("offline_library_rechecked_every_60s", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    h.probeAnswers = Array.from({ length: 10 }, () => ({ state: "unreachable" as const, code: "EHOSTDOWN" }));
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "unreachable", code: "EHOSTDOWN" }));
    await settle();
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_UNREACHABLE" });
    expect(h.offlineFlags.at(-1)).toEqual({ lib: lib.id, offline: true });
    expect(h.probes).toHaveLength(1); // re-probed at once
    await h.time.advance(59 * S);
    expect(h.probes).toHaveLength(1);
    await h.time.advance(S);
    expect(h.probes).toHaveLength(2);
    await h.time.advance(60 * S);
    expect(h.probes).toHaveLength(3);
    // Offline: no scan, whatever is in use.
    h.svc.use([lib.id]);
    await h.time.advance(5 * MIN);
    expect(scans(h)).toHaveLength(0);
    expect(h.svc.view(h.registry.get(lib.id)!).state).toBe("offline");
  });

  test("return_online_triggers_mount_scan", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    h.probeAnswers = [{ state: "timeout" }];
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "timeout" }));
    await settle();
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_TIMEOUT" });
    // The next re-probe answers online (with the stored identity).
    await h.time.advance(60 * S);
    expect(h.registry.get(lib.id)!.state).toBe("scanning");
    expect(h.offlineFlags.at(-1)).toEqual({ lib: lib.id, offline: false });
    expect(scans(h, "full")).toHaveLength(1);
    await finishAll(h);
    expect(h.registry.get(lib.id)!.state).toBe("online");
    expect(h.journal.at(-1)).toMatchObject({ kind: "library.scan", payload: { full: true, trigger: "mount", outcome: "succeeded" } });
  });

  test("volume_changed_waits_for_accept", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    h.probeAnswers = Array.from({ length: 5 }, () => ({ state: "volumeChanged" as const, key: "bb" }));
    h.svc.kindEvents.offline?.(lib.id, offlineJob(lib.id, { state: "volumeChanged", key: "bb" }));
    await settle();
    await h.time.advance(4 * MIN);
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_VOLUME_CHANGED" });
    expect(h.svc.view(h.registry.get(lib.id)!).warnings).toContainEqual({ code: "ROOT_VOLUME_CHANGED" });
    expect(scans(h)).toHaveLength(0);
    // The owner accepts: the volume is measured again and stored, and a full scan runs.
    h.measured = identity("bb");
    await acceptVolume(h.svc, lib.id, ACTOR);
    expect(h.registry.get(lib.id)!.volume.key).toBe("bb");
    expect(h.registry.get(lib.id)!.state).toBe("scanning");
    expect(scans(h, "full")).toHaveLength(1);
    expect(h.svc.pools(lib.id).read).toBe("io:bb.read");
  });

  test("root_empty_suspect_goes_offline_until_files_return", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a");
    await h.svc.submitScan(lib.id, { mode: "full", trigger: "demand" });
    h.probeAnswers = [{ state: "empty" }, { state: "empty" }];
    await h.finish(h.active()[0]!.id, "failed", { error: { code: "ROOT_EMPTY_SUSPECT", message: "", details: {} } });
    expect(h.registry.get(lib.id)).toMatchObject({ state: "offline", offlineReason: "ROOT_EMPTY_SUSPECT" });
    await h.time.advance(60 * S);
    expect(h.registry.get(lib.id)!.state).toBe("offline");
    await h.time.advance(60 * S); // the files are back
    expect(h.registry.get(lib.id)!.state).toBe("scanning");
  });
});

describe("watches", () => {
  test("watch_hint_becomes_a_targeted_scan", async () => {
    const h = await rig();
    const lib = await h.add("/data/media", {}, "local");
    expect(h.watches).toEqual([lib.id]);
    h.emitWatch({ type: "watch", lib: lib.id, dirs: [{ components: ["YQ=="] }], all: false });
    await settle();
    expect(scans(h, "dirs")).toHaveLength(1);
    h.emitWatch({ type: "watch", lib: lib.id, dirs: [], all: true });
    await settle();
    expect(scans(h, "poll")).toHaveLength(1);
    // A watch rescan that found something is journaled; one that found nothing is only counted.
    const [dirsJob, pollJob] = h.active().map((j) => j.id);
    h.svc.kindEvents.batchApplied?.(lib.id, dirsJob!, { ...emptyCounts(), added: 1, transactions: 1 });
    await h.finish(dirsJob!);
    await h.finish(pollJob!);
    expect(h.journal.map((j) => (j.payload as { trigger: string }).trigger)).toEqual(["watch"]);
    expect(h.svc.status(lib.id).noChangePolls).toBe(1);
  });

  test("watch_overflow_degrades_to_5min_polls", async () => {
    const h = await rig();
    const lib = await h.add("/data/media", {}, "local");
    // Watched: only the 6-hour safety-net poll.
    await h.time.advance(6 * H - S);
    expect(scans(h, "poll")).toHaveLength(0);
    await h.time.advance(S);
    expect(scans(h, "poll")).toHaveLength(1);
    await finishAll(h);
    await h.time.advance(H);
    expect(scans(h, "poll")).toHaveLength(1);
    h.emitWatch({ type: "watchDegraded", lib: lib.id, reason: "ENOSPC" });
    await settle();
    expect(h.svc.status(lib.id)).toMatchObject({ watch: "degraded", watchReason: "ENOSPC" });
    expect(h.svc.view(h.registry.get(lib.id)!).warnings).toContainEqual({ code: "WATCH_DEGRADED" });
    await h.time.advance(5 * MIN);
    expect(scans(h, "poll")).toHaveLength(2);
    await finishAll(h);
    await h.time.advance(5 * MIN);
    expect(scans(h, "poll")).toHaveLength(3);
  });
});

describe("journal", () => {
  test("no_change_polls_not_journaled", async () => {
    const h = await rig();
    const lib = await h.add("/nas/a", { scan: { poll: 60 } });
    await h.time.advance(60 * S);
    await finishAll(h);
    expect(h.journal).toHaveLength(0);
    expect(h.svc.status(lib.id).noChangePolls).toBe(1);
    // A poll that found something is journaled.
    await h.time.advance(60 * S);
    const job = h.active()[0]!.id;
    h.svc.kindEvents.batchApplied?.(lib.id, job, { ...emptyCounts(), added: 2, transactions: 1 });
    await h.finish(job);
    expect(h.journal).toHaveLength(1);
    expect(h.journal[0]!.payload).toMatchObject({ full: false, trigger: "schedule", added: 2, files: log2Bucket(3) });
    // A poll that failed is too.
    await h.time.advance(60 * S);
    await h.finish(h.active()[0]!.id, "failed", { error: { code: "IO_ERROR", message: "", details: {} } });
    expect(h.journal.at(-1)!.payload).toMatchObject({ outcome: "failed", code: "IO_ERROR" });
  });

  test("library_scan_events_payload_closed", async () => {
    const h = await rig();
    const lib = await h.add("/nas/Secret Show", { name: "Secret Show" });
    await h.svc.submitScan(lib.id, { mode: "full", trigger: "demand" });
    await finishAll(h);
    const payload = h.journal.at(-1)!.payload as Record<string, unknown>;
    expect(Object.keys(payload).sort()).toEqual(
      ["added", "changed", "dirs", "files", "full", "micros", "missing", "moved", "outcome", "placeholders", "storage", "trigger"].sort(),
    );
    const text = JSON.stringify(payload);
    for (const secret of [lib.id, "Secret", "/nas"]) expect(text).not.toContain(secret);
    expect(payload).toMatchObject({ storage: "network", full: true, trigger: "demand", files: 2, dirs: 1 });
    // The real journal accepts it (its schema is the Rust payload's) and refuses nothing.
    const dir = mkdtempSync(join(tmpdir(), "fin-libsvc-journal-"));
    const journal = Journal.open(dir, JSON.stringify({ diagnostics: "on" }), hostname());
    try {
      const refused: string[] = [];
      recordScan(journal, libraryScanPayload({ storage: "sync", mode: "poll", trigger: "watch", outcome: "succeeded", applied: { ...emptyCounts(), missing: 1 }, micros: 5 }), (m) => refused.push(m));
      expect(refused).toEqual([]);
      const page = JSON.parse(await journal.export(JSON.stringify({ kinds: ["library.scan"] }), null, 10)) as ExportPage;
      const lines = page.lines as Envelope[];
      expect(lines).toHaveLength(1);
      expect(lines[0]).toMatchObject({ kind: "library.scan", source: "finisherd", payload: { storage: "sync", trigger: "watch", missing: 1 } });
      // The payload is closed: an unknown field is refused (logged, never thrown), and nothing lands.
      const good = libraryScanPayload({ storage: "local", mode: "full", trigger: "add", outcome: "succeeded", applied: emptyCounts(), micros: 1 });
      for (const extra of [{ library: "01abc" }, { path: "/nas/Secret Show" }, { files: 3, extra: true }]) {
        recordScan(journal, { ...good, ...extra } as typeof good, (m) => refused.push(m));
      }
      expect(refused).toHaveLength(3);
      for (const m of refused) expect(m).toMatch(/refused a library\.scan/);
      const after = JSON.parse(await journal.export(JSON.stringify({ kinds: ["library.scan"] }), null, 10)) as ExportPage;
      expect(after.lines).toHaveLength(1);
      // Past `recordScan`, the journal's typed decode drops them: they never reach the disk.
      journal.record("library.scan", 1, null, JSON.stringify({ ...good, library: "01abc", path: "/nas/Secret Show" }), "finisherd");
      const direct = JSON.parse(await journal.export(JSON.stringify({ kinds: ["library.scan"] }), null, 10)) as ExportPage;
      expect(direct.lines).toHaveLength(2);
      const text = JSON.stringify(direct.lines);
      for (const leaked of ["01abc", "Secret", "library\""]) expect(text).not.toContain(leaked);
    } finally {
      await journal.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("use", () => {
  test("open_projects_hold_the_libraries_their_assets_live_in", async () => {
    const h = await rig();
    const a = await h.add("/nas/a", { aliases: ["/Volumes/media/a"] });
    const b = await h.add("/nas/b");
    expect(underRoot("/nas/a", "/nas/a/x.mov")).toBe(true);
    expect(underRoot("/nas/a", "/nas/ab/x.mov")).toBe(false);
    expect(librariesHolding(h.registry, ["/nas/ab/x.mov", "/Volumes/media/a/clip.mov"])).toEqual([a.id]);
    expect(librariesHolding(h.registry, ["/nas/b/y.mov", "/nas/a/z.mov"]).sort()).toEqual([a.id, b.id].sort());
    const held = h.svc.use(librariesHolding(h.registry, ["/nas/b/y.mov"]));
    expect(h.svc.status(b.id)).toMatchObject({ inUse: true, holders: 1 });
    held.release();
    held.release();
    expect(h.svc.status(b.id)).toMatchObject({ inUse: false, holders: 0 });
  });
});
