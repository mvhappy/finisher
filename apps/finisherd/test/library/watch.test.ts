// Local roots' watches (plan 5D Decision 30): the indexer thread's `Watches` (raw names, quiet
// time, dot-files, overflow to a whole-library poll) and, over a real engine, a hint that
// becomes a targeted scan after 2 s of quiet, and a failing watch that degrades the library.
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { addLibrary } from "../../src/library/admin.ts";
import { exec } from "../../src/library/service.ts";
import { type WatchEvent, Watches, dotPath, splitRaw, watchNeed } from "../../src/library/watch.ts";
import { until } from "../support/jobs.ts";
import { grid, tree } from "./support/kinds.ts";
import { closeLiveRigs, liveRig, liveRoots } from "./support/live.ts";
import { ManualTime } from "./support/service.ts";

/** A deadline for what the OS and a loaded machine decide (a hang guard, not a product limit). */
const WAIT = 30_000;

/**
 * Resolves once `n()` is above 0 and has not changed for `quietMs` of real time. A watch's events
 * reach it within milliseconds of the writes that caused them (one thread, in order); the gap only
 * has to be longer than the machine's hiccups, and nothing is asserted about its length.
 */
async function arrived(n: () => number, quietMs = 300): Promise<void> {
  await until(() => n() > 0, WAIT);
  for (let last = n(), since = Date.now(); Date.now() - since < quietMs; ) {
    await new Promise((r) => setTimeout(r, 20));
    if (n() !== last) (last = n()), (since = Date.now());
  }
}

/** Timers the test moves by hand, counting how often the quiet time was (re)armed, i.e. events taken. */
function counted() {
  const time = new ManualTime();
  const armed = { n: 0 };
  return { time, armed, timers: { after: (ms: number, f: () => void) => (armed.n++, time.after(ms, f)) } };
}

const b64 = (s: string) => Buffer.from(s).toString("base64");
const temps: string[] = [];
afterEach(async () => {
  await closeLiveRigs();
  for (const d of temps.splice(0)) rmSync(d, { recursive: true, force: true });
}, 60_000);

/**
 * Until the watch of `lib` has reported a write in `dir`, then forgets what it reported. macOS's
 * FSEvents delivers the tail of what happened just before a recursive `fs.watch` started (the
 * test's own `mkdir`s), and its stream starts after `watch` returns, so neither a sleep nor
 * `ready` covers it; events come in order, so that tail is in the same report as the write.
 */
async function settle(w: Watches, events: WatchEvent[], lib: string, dir: string): Promise<void> {
  await until(() => w.ready(lib), WAIT);
  for (let i = 0; events.length === 0; i++) {
    if (i === 10) throw new Error("the watch never reported a write");
    writeFileSync(join(dir, `settle-${i}.mov`), "x");
    await until(() => events.length > 0, 1500).catch(() => undefined);
  }
  events.length = 0;
}

/** `settle` for a watch whose quiet time `time` moves: the sentinel's events are counted, then flushed and forgotten. */
async function settleCounted(w: Watches, lib: string, dir: string, armed: { n: number }, time: ManualTime, events: WatchEvent[]): Promise<void> {
  await until(() => w.ready(lib), WAIT);
  for (let i = 0; armed.n === 0; i++) {
    if (i === 10) throw new Error("the watch never reported a write");
    writeFileSync(join(dir, `settle-${i}.mov`), "x");
    await until(() => armed.n > 0, 1500).catch(() => undefined);
  }
  await arrived(() => armed.n);
  await time.advance(10_000);
  events.length = 0;
}

describe("Watches", () => {
  test("split_raw_names", () => {
    expect(splitRaw(Buffer.from("a/b/c.mov"), false).map((p) => Buffer.from(p).toString())).toEqual(["a", "b", "c.mov"]);
    expect(splitRaw(Buffer.from("a\\b/c"), true).map((p) => Buffer.from(p).toString())).toEqual(["a", "b", "c"]);
    expect(splitRaw(Buffer.from([0x61, 0xff, 0x2f, 0x62]), false).map((p) => [...p])).toEqual([[0x61, 0xff], [0x62]]);
  });

  test("events_coalesce_after_quiet_and_skip_dot_files", async () => {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "fin-watch-")));
    temps.push(root);
    for (const d of ["a", "b/c", "b/d", "b/e", "b/f", ".cache/deep"]) mkdirSync(join(root, d), { recursive: true });
    const events: WatchEvent[] = [];
    const { time, armed, timers } = counted();
    const w = new Watches((e) => events.push(e), { quietMs: 400, maxDirs: 3, timers, known: (_lib, parts) => parts.map((p) => Buffer.from(p).toString("base64")) });
    expect(w.watch("01lib", root)).toEqual({ ok: true });
    try {
      await settleCounted(w, "01lib", join(root, "a"), armed, time, events);
      const base = armed.n;
      writeFileSync(join(root, "a", "one.mov"), "x");
      writeFileSync(join(root, "a", "two.mov"), "x");
      writeFileSync(join(root, ".hidden"), "x");
      writeFileSync(join(root, ".cache", "deep", "x.mov"), "x");
      await arrived(() => armed.n - base);
      await time.advance(399);
      expect(events).toHaveLength(0); // still within the quiet time
      await time.advance(1);
      expect(events).toHaveLength(1);
      await time.advance(5_000);
      expect(events).toHaveLength(1);
      const e = events[0]! as Extract<WatchEvent, { type: "watch" }>;
      expect(e.all).toBe(false);
      expect(e.dirs.map((d) => d.components)).toEqual([[b64("a")]]);
      // More directories than the cap: one whole-library poll instead.
      events.length = 0;
      const before = armed.n;
      for (const d of ["c", "d", "e", "f"]) writeFileSync(join(root, "b", d, "x.mov"), "x");
      await arrived(() => armed.n - before);
      await time.advance(400);
      expect(events).toHaveLength(1);
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

  test.runIf(process.platform === "linux")("native_watch_budget_and_failures_degrade", async () => {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "fin-watch-")));
    temps.push(root);
    mkdirSync(join(root, "lib", "sub"), { recursive: true });
    const events: WatchEvent[] = [];
    const known = (_lib: string, parts: ReadonlyArray<Uint8Array>) => parts.map((p) => Buffer.from(p).toString("base64"));
    // 1,000 watches for the user: 900 usable; each library needs its directories + 25% + 256.
    const w = new Watches((e) => events.push(e), { quietMs: 100, known, maxUserWatches: () => 1000 });
    try {
      expect(watchNeed(400)).toBe(756);
      expect(w.watch("01big", join(root, "lib"), 2000)).toEqual({ ok: false, reason: "WATCH_LIMIT" });
      expect(w.watch("01one", join(root, "lib"), 400)).toEqual({ ok: true });
      // What the first one reserved is not there for the second.
      expect(w.watch("01two", join(root, "lib"), 400)).toEqual({ ok: false, reason: "WATCH_LIMIT" });
      w.unwatch("01one");
      expect(w.watch("01two", join(root, "lib"), 400)).toEqual({ ok: true });
      await until(() => w.ready("01two"), WAIT);
      // The root goes away: the native watch fails, and the library degrades.
      rmSync(join(root, "lib"), { recursive: true });
      await until(() => events.some((e) => e.type === "watchDegraded"), WAIT);
      expect(events.find((e) => e.type === "watchDegraded")).toEqual({ type: "watchDegraded", lib: "01two", reason: "WATCH_ROOT_GONE" });
      expect(w.watching()).toEqual([]);
      // A root that cannot be watched at all degrades the same way.
      events.length = 0;
      expect(w.watch("01gone", join(root, "nothing"), 0)).toEqual({ ok: true });
      await until(() => events.length > 0, WAIT);
      expect(events).toEqual([{ type: "watchDegraded", lib: "01gone", reason: "WATCH_ROOT_GONE" }]);
    } finally {
      w.closeAll();
    }
  });

  test("fs_watch_fallback_ignores_dot_paths", async () => {
    const root = realpathSync(mkdtempSync(join(tmpdir(), "fin-watch-")));
    temps.push(root);
    mkdirSync(join(root, "a"));
    mkdirSync(join(root, ".git"));
    // Settled in a folder of its own: a late report of it cannot pass for `a/`'s event.
    mkdirSync(join(root, "settle"));
    expect(dotPath(".git/x")).toBe(true);
    expect(dotPath("a/.x")).toBe(true);
    expect(dotPath("a/b.mov")).toBe(false);
    // `encoding: "buffer"` hands `ignore` bytes where the watch is native (Windows): a string
    // method there threw and ended the catalog thread (CATALOG_THREAD_FAILED, 2026-11-08 run).
    expect(dotPath(Buffer.from(".git/x"))).toBe(true);
    expect(dotPath(Buffer.from("a/.x"))).toBe(true);
    expect(dotPath(Buffer.from("a/b.mov"))).toBe(false);
    const events: WatchEvent[] = [];
    const w = new Watches((e) => events.push(e), { quietMs: 200, native: false, known: (_lib, parts) => parts.map((p) => Buffer.from(p).toString("base64")) });
    expect(w.watch("01lib", root)).toEqual({ ok: true });
    try {
      await settle(w, events, "01lib", join(root, "settle"));
      writeFileSync(join(root, ".git", "index"), "x");
      writeFileSync(join(root, "a", "x.mov"), "x");
      await until(() => events.length > 0, WAIT);
      expect(events[0]).toMatchObject({ type: "watch", all: false, dirs: [{ components: [b64("a")] }] });
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
    // "ok" is the watch started; a change before its first walk has put a watch on every directory can be missed.
    for (const end = Date.now() + WAIT; !(await exec(rig.catalogs.indexer.watchReady(library.id))).ready; await new Promise((r) => setTimeout(r, 20))) {
      expect(Date.now(), "the watch never finished its first walk").toBeLessThan(end);
    }
    const before = rig.t.jobs.list({ kind: "library.scan", library: library.id }).jobs.length;
    const wrote = Date.now();
    writeFileSync(join(root, "d01", "new.mov"), "x");
    const job2 = await until(
      () => rig.t.jobs.list({ kind: "library.scan", library: library.id }).jobs.find((j) => (j.params as { mode: string }).mode === "dirs"),
      WAIT,
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

  test.runIf(process.platform === "linux")("watch_that_would_not_fit_degrades_at_once", { timeout: 60_000 }, async () => {
    const root = join(liveRoots(), "local");
    tree(root, grid(2, 1));
    const rig = await liveRig({ watchMaxUserWatches: 100 });
    const { library, job } = await addLibrary(rig.svc, { root }, { kind: "local", id: "test" });
    expect((await rig.t.wait(job!.id, 30_000)).state).toBe("succeeded");
    // 90 usable watches; the library needs its directories + 25% + 256: never started, polled instead.
    await until(() => rig.svc.status(library.id).watch !== "none");
    expect(rig.svc.status(library.id)).toMatchObject({ watch: "degraded", watchReason: "WATCH_LIMIT" });
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
