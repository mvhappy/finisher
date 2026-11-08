// Local roots' watches (plan 5D Decision 30): the indexer thread's `Watches` (raw names, quiet
// time, dot-files, overflow to a whole-library poll) and, over a real engine, a hint that
// becomes a targeted scan after 2 s of quiet, and a failing watch that degrades the library.
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { addLibrary } from "../../src/library/admin.ts";
import { exec } from "../../src/library/service.ts";
import { type WatchEvent, Watches, splitRaw } from "../../src/library/watch.ts";
import { until } from "../support/jobs.ts";
import { grid, tree } from "./support/kinds.ts";
import { closeLiveRigs, liveRig, liveRoots } from "./support/live.ts";

const b64 = (s: string) => Buffer.from(s).toString("base64");
const temps: string[] = [];
afterEach(async () => {
  await closeLiveRigs();
  for (const d of temps.splice(0)) rmSync(d, { recursive: true, force: true });
}, 60_000);

describe("Watches", () => {
  test("split_raw_names", () => {
    expect(splitRaw(Buffer.from("a/b/c.mov"), false).map((p) => Buffer.from(p).toString())).toEqual(["a", "b", "c.mov"]);
    expect(splitRaw(Buffer.from("a\\b/c"), true).map((p) => Buffer.from(p).toString())).toEqual(["a", "b", "c"]);
    expect(splitRaw(Buffer.from([0x61, 0xff, 0x2f, 0x62]), false).map((p) => [...p])).toEqual([[0x61, 0xff], [0x62]]);
  });

  test("events_coalesce_after_quiet_and_skip_dot_files", async () => {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "fin-watch-")));
    temps.push(root);
    mkdirSync(join(root, "a"));
    mkdirSync(join(root, "b"));
    const events: WatchEvent[] = [];
    const w = new Watches((e) => events.push(e), { quietMs: 400, maxDirs: 3, known: (_lib, parts) => parts.map((p) => Buffer.from(p).toString("base64")) });
    expect(w.watch("01lib", root)).toEqual({ ok: true });
    try {
      await new Promise((r) => setTimeout(r, 100));
      writeFileSync(join(root, "a", "one.mov"), "x");
      writeFileSync(join(root, "a", "two.mov"), "x");
      writeFileSync(join(root, ".hidden"), "x");
      await new Promise((r) => setTimeout(r, 150));
      expect(events).toHaveLength(0); // still within the quiet time
      await until(() => events.length > 0, 5000);
      expect(events).toHaveLength(1);
      const e = events[0]! as Extract<WatchEvent, { type: "watch" }>;
      expect(e.all).toBe(false);
      expect(e.dirs.map((d) => d.components)).toContainEqual([b64("a")]);
      expect(e.dirs.every((d) => !d.components.includes(b64(".hidden")))).toBe(true);
      // More directories than the cap: one whole-library poll instead.
      events.length = 0;
      for (const d of ["c", "d", "e", "f"]) {
        mkdirSync(join(root, "b", d));
        writeFileSync(join(root, "b", d, "x.mov"), "x");
      }
      await until(() => events.length > 0, 5000);
      expect(events[0]).toMatchObject({ type: "watch", all: true, dirs: [] });
      // An error degrades it, once, and the watch is gone.
      events.length = 0;
      expect(w.fault("01lib")).toBe(true);
      expect(events).toEqual([{ type: "watchDegraded", lib: "01lib", reason: "WATCH_OVERFLOW" }]);
      expect(w.watching()).toEqual([]);
    } finally {
      w.closeAll();
    }
  });
});

describe("watched libraries", () => {
  test("local_watch_hint_rescans_after_2s_quiet", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "local");
    tree(root, grid(3, 2));
    const rig = await liveRig();
    const { library, job } = await addLibrary(rig.svc, { root }, { kind: "local", id: "test" });
    expect((await rig.t.wait(job!.id, 30_000)).state).toBe("succeeded");
    await until(() => rig.svc.status(library.id).jobs.length === 0);
    expect(rig.svc.status(library.id).watch).toBe("ok");
    const before = rig.t.jobs.list({ kind: "library.scan", library: library.id }).jobs.length;
    const wrote = Date.now();
    writeFileSync(join(root, "d01", "new.mov"), "x");
    const job2 = await until(
      () => rig.t.jobs.list({ kind: "library.scan", library: library.id }).jobs.find((j) => (j.params as { mode: string }).mode === "dirs"),
      20_000,
    );
    // Submitted only after 2 s without another event.
    expect(job2.createdAt - wrote).toBeGreaterThanOrEqual(1_900);
    expect(rig.t.jobs.list({ kind: "library.scan", library: library.id }).jobs.length).toBe(before + 1);
    expect((await rig.t.wait(job2.id, 30_000)).state).toBe("succeeded");
    await until(() => rig.journal.some((j) => j.payload.trigger === "watch"));
    const counts = await exec(rig.catalogs.reader.counts(library.id));
    // The probe stage (Task 11) may already have probed `new.mov` (one byte: `probeFailed`).
    expect((counts.files.present ?? 0) + (counts.files.probeFailed ?? 0)).toBe(7);
  });

  test("watch_failure_degrades_the_library", { timeout: 60_000 }, async () => {
    const root = join(liveRoots(), "local");
    tree(root, grid(1, 1));
    const rig = await liveRig();
    const { library } = await addLibrary(rig.svc, { root }, { kind: "local", id: "test" });
    await until(() => rig.svc.status(library.id).watch === "ok");
    expect(await exec(rig.catalogs.indexer.watchFault(library.id))).toEqual({ faulted: true });
    await until(() => rig.svc.status(library.id).watch === "degraded");
    expect(rig.svc.status(library.id).watchReason).toBe("WATCH_OVERFLOW");
    expect(rig.svc.view(rig.svc.registry.get(library.id)!).warnings).toContainEqual({ code: "WATCH_DEGRADED" });
  });
});
