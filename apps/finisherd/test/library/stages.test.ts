// The stage framework (plan 5D Task 11; Decisions 20, 39–42): the library service over a real
// engine, the test-hooks `fin-workers` and real temp roots. Stages run the `test.stage` kind,
// whose worker opens each file through `open_for_read` with the `StageFile` it was given.
import { readFileSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { addLibrary, scanLibrary, setLibrary } from "../../src/library/admin.ts";
import type { FileEvent, StageSpec } from "../../src/library/api.ts";
import type { FileRow } from "../../src/library/catalog/core/queries.ts";
import { exec } from "../../src/library/service.ts";
import { until } from "../support/jobs.ts";
import { PNG_1X1, overlay, tree } from "./support/kinds.ts";
import { type LiveRig, type LiveRigOptions, closeLiveRigs, liveRig, liveRoots } from "./support/live.ts";
import { ManualTime } from "./support/service.ts";
import { type Seen, type TestStageKnobs, registerTestStageKind, testStage } from "./support/stages.ts";

afterEach(closeLiveRigs, 60_000);

const ACTOR = { kind: "local", id: "test" } as const;
const WAIT = 60_000;
const BASE_S = 1_700_000_000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function eventually<A>(f: () => Promise<A | undefined | false>, ms = WAIT): Promise<A> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() > end) throw new Error("condition not met in time");
    await sleep(50);
  }
}

/** Writes `names` at the root, each with its own mtime (distinct fingerprints even on a coarse clock). */
function files(root: string, names: ReadonlyArray<string>, offset = 0): void {
  tree(root, { "": names });
  names.forEach((n, i) => utimesSync(join(root, n), BASE_S + offset + i, BASE_S + offset + i));
}

interface Setup {
  readonly rig: LiveRig;
  readonly root: string;
  readonly lib: string;
}

async function setup(
  opts: { names?: string[]; stages?: StageSpec[]; service?: LiveRigOptions["service"]; root?: string; rig?: LiveRig } = {},
): Promise<Setup> {
  const root = opts.root ?? join(liveRoots(), "lib");
  if (opts.root === undefined) files(root, opts.names ?? ["a.png", "b.png"]);
  const rig = opts.rig ?? (await liveRig({ register: (k) => registerTestStageKind(k), ...(opts.service === undefined ? {} : { service: opts.service }) }));
  for (const s of opts.stages ?? []) rig.svc.registerStage(s);
  const { library, job } = await addLibrary(rig.svc, { root }, ACTOR);
  expect((await rig.t.wait(job!.id, WAIT)).state).toBe("succeeded");
  return { rig, root, lib: library.id };
}

/** The root's files of `lib`, by name, once each probe-able one has a fingerprint. */
async function probed(s: Setup, names: ReadonlyArray<string>): Promise<Record<string, FileRow>> {
  return eventually(async () => {
    const rows = (await exec(s.rig.catalogs.reader.files(s.lib, { limit: 500 }))).files;
    const by = Object.fromEntries(rows.map((r) => [r.nameNfc, r]));
    return names.every((n) => by[n]?.fp != null) ? by : undefined;
  });
}

const stageRows = (s: Setup, stage: string) => exec(s.rig.catalogs.reader.stageRows(s.lib, { stage })).then((r) => r.rows);

/** The stage's rows once every one of `fps` is in a final state. */
async function settled(s: Setup, stage: string, fps: ReadonlyArray<string>, states = ["done", "skipped"]) {
  return eventually(async () => {
    const rows = await stageRows(s, stage);
    return fps.every((fp) => rows.some((r) => r.fp === fp && states.includes(r.state))) ? rows : undefined;
  });
}

const stageJobs = (rig: LiveRig): JobView[] => rig.t.jobs.list({ kind: "test.stage", limit: 500 }).jobs;
const active = (rig: LiveRig): JobView[] => stageJobs(rig).filter((j) => j.finishedAt === undefined);

describe("pumping", () => {
  test("stage_runs_after_probe_for_applicable_files", { timeout: 90_000 }, async () => {
    const seen: Seen = [];
    const s = await setup({ names: ["a.png", "b.png", "c.bin"], stages: [testStage("t.one", { seen, appliesTo: (f) => !f.display.endsWith("b.png") })] });
    const by = await probed(s, ["a.png", "b.png"]);
    const rows = await settled(s, "t.one", [by["a.png"]!.fp!, by["b.png"]!.fp!]);
    expect(rows.find((r) => r.fp === by["a.png"]!.fp)).toMatchObject({ state: "done", version: "v1", attempts: 0 });
    expect(rows.find((r) => r.fp === by["b.png"]!.fp)).toMatchObject({ state: "skipped", code: "NOT_APPLICABLE" });
    // `c.bin` is not in the library's kinds: never probed, never fingerprinted, never staged.
    expect(by["c.bin"]?.fp ?? null).toBeNull();
    expect(rows).toHaveLength(2);
    expect(seen.flat().map((f) => f.fp)).toEqual([by["a.png"]!.fp]);
    // The stage job came after the probe that fingerprinted its file (once its outcome landed,
    // possibly before the probe job itself ended).
    const probes = s.rig.t.jobs.list({ kind: "library.probe", library: s.lib, limit: 50 }).jobs;
    const [job] = stageJobs(s.rig);
    expect(probes.length).toBeGreaterThan(0);
    expect(job!.createdAt).toBeGreaterThanOrEqual(Math.min(...probes.map((p) => p.createdAt)));
    expect(job).toMatchObject({ owner: { kind: "system", id: "library" }, priority: "background", library: s.lib });
  });

  test("stage_file_carries_raw_path_and_expect", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, { "": [] });
    // Linux takes any bytes: a Latin-1 name (not UTF-8); elsewhere an NFD name (shown NFC).
    const raw = process.platform === "linux" ? Buffer.concat([Buffer.from("caf"), Buffer.from([0xe9]), Buffer.from(".png")]) : Buffer.from("café.png");
    const path = Buffer.concat([Buffer.from(`${root}/`), raw]);
    writeFileSync(path, PNG_1X1);
    const st = statSync(path, { bigint: true });
    const seen: Seen = [];
    const s = await setup({ root, stages: [testStage("t.raw", { seen })] });
    const file = await eventually(async () => seen.flat()[0]);
    expect(file).toMatchObject({
      library: s.lib,
      root,
      rel: { components: [raw.toString("base64")] },
      display: process.platform === "linux" ? "caf�.png" : "café.png",
      expect: { size: Number(st.size), mtimeNs: st.mtimeNs.toString() },
      hydrate: false,
      size: PNG_1X1.length,
      kind: "image",
      storage: "local",
    });
    expect(file.expect.fileId).toMatch(/\S/);
    expect(file.probe).toMatchObject({ kind: "image" });
    // The worker opened the file by that raw chain and that `Expect`.
    const job = await eventually(async () => stageJobs(s.rig).find((j) => j.state === "succeeded"));
    expect((job.result as { items: Record<string, unknown> }).items[file.fp]).toEqual({ fileId: file.fileId, bytes: PNG_1X1.length });
    expect(readFileSync(path).equals(PNG_1X1)).toBe(true);
  });

  test("depends_on_orders_stages", { timeout: 90_000 }, async () => {
    const events: FileEvent[] = [];
    const rig = await liveRig({ register: (k) => registerTestStageKind(k) });
    rig.svc.onFile((e) => void (e.type === "stage" && events.push(e)));
    // Registered dependent first: it waits for its dependency's registration and rows.
    rig.svc.registerStage(testStage("t.b", { dependsOn: ["t.a"] }));
    rig.svc.registerStage(testStage("t.a", { knobs: () => ({ delayMs: 200 }) }));
    const s = await setup({ rig, names: ["a.png", "b.png", "c.png"] });
    const by = await probed(s, ["a.png", "b.png", "c.png"]);
    const fps = Object.values(by).map((f) => f.fp!);
    await settled(s, "t.b", fps);
    for (const fp of fps) {
      const a = events.findIndex((e) => e.fp === fp && e.stage === "t.a");
      const b = events.findIndex((e) => e.fp === fp && e.stage === "t.b");
      expect(a).toBeGreaterThanOrEqual(0);
      expect(b).toBeGreaterThan(a);
    }
    expect(() => rig.svc.registerStage(testStage("t.c", { dependsOn: ["t.c"] }))).toThrow(/dependsOn/);
    expect(() => rig.svc.registerStage(testStage("probe"))).toThrow(/built in/);
    expect(() => rig.svc.registerStage(testStage("t.a"))).toThrow(/registered already/);
    expect(() => rig.svc.registerStage(testStage("t.big", { batch: 257 }))).toThrow(/batch/);
  });

  test("max_in_flight_per_library", { timeout: 90_000 }, async () => {
    const names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
    const s = await setup({ names, stages: [testStage("t.max", { batch: 1, maxInFlight: 2, knobs: () => ({ delayMs: 300 }) })] });
    const by = await probed(s, names);
    let most = 0;
    const fps = names.map((n) => by[n]!.fp!);
    await eventually(async () => {
      most = Math.max(most, active(s.rig).length);
      const rows = await stageRows(s, "t.max");
      return fps.every((fp) => rows.some((r) => r.fp === fp && r.state === "done"));
    });
    expect(most).toBe(2);
    expect(stageJobs(s.rig)).toHaveLength(6);
  });

  test("same_fp_in_two_libraries_never_in_two_jobs", { timeout: 120_000 }, async () => {
    const events: FileEvent[] = [];
    const rig = await liveRig({ register: (k) => registerTestStageKind(k) });
    rig.svc.onFile((e) => void (e.type === "stage" && events.push(e)));
    rig.svc.registerStage(testStage("t.share", { knobs: () => ({ delayMs: 4000 }) }));
    const rootA = join(liveRoots(), "a");
    const rootB = join(liveRoots(), "b");
    files(rootA, ["x.png"]);
    files(rootB, ["x.png"]); // same bytes, size and mtime: the same fp1
    const a = await setup({ rig, root: rootA });
    const fp = (await probed(a, ["x.png"]))["x.png"]!.fp!;
    const running = await eventually(async () => active(rig)[0]);
    const b = await setup({ rig, root: rootB });
    expect((await probed(b, ["x.png"]))["x.png"]!.fp).toBe(fp);
    // B waits on A's job: its row records that job.
    await eventually(async () => (await stageRows(b, "t.share")).find((r) => r.state === "queued" && r.jobId === running.id));
    await settled(a, "t.share", [fp]);
    await settled(b, "t.share", [fp]);
    expect(stageJobs(rig)).toHaveLength(1);
    expect(events.map((e) => e.library).sort()).toEqual([a.lib, b.lib].sort());
  });
});

describe("failures", () => {
  test("offline_and_stuck_failures_cost_no_attempt", { timeout: 120_000 }, async () => {
    const calls = { stuck: 0, off: 0 };
    const s = await setup({
      names: ["a.png"],
      stages: [
        testStage("t.stuck", { knobs: (): TestStageKnobs => (calls.stuck++ === 0 ? { jobError: "RESOURCE_STUCK" } : {}) }),
        testStage("t.off", { knobs: (): TestStageKnobs => (calls.off++ === 0 ? { jobError: "LIBRARY_OFFLINE" } : {}) }),
      ],
    });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    const stuck = await settled(s, "t.stuck", [fp], ["done"]);
    const off = await settled(s, "t.off", [fp], ["done"]);
    expect(stuck[0]).toMatchObject({ state: "done", attempts: 0 });
    expect(off[0]).toMatchObject({ state: "done", attempts: 0 });
    expect(calls.stuck).toBeGreaterThanOrEqual(2);
    expect(calls.off).toBeGreaterThanOrEqual(2);
    // The offline failure put the library offline; a re-probe brought it back.
    await until(() => s.rig.svc.registry.get(s.lib)?.state !== "offline");
  });

  test("offline_and_paused_stop_the_pump", { timeout: 120_000 }, async () => {
    const s = await setup({ names: ["a.png"], stages: [testStage("t.pause", { knobs: () => ({ delayMs: 3000 }) })] });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    const first = await eventually(async () => active(s.rig).find((j) => j.state === "running"));
    await setLibrary(s.rig.svc, s.lib, { paused: true }, ACTOR);
    expect((await s.rig.t.wait(first.id, WAIT)).state).toBe("cancelled");
    const back = await eventually(async () => (await stageRows(s, "t.pause")).find((r) => r.state === "stale"));
    expect(back).toMatchObject({ attempts: 0, jobId: null });
    await sleep(800);
    expect(active(s.rig)).toEqual([]);
    await setLibrary(s.rig.svc, s.lib, { paused: false }, ACTOR);
    const second = await eventually(async () => active(s.rig).find((j) => j.state === "running"));
    await s.rig.svc.goOffline(s.lib, "ROOT_UNREACHABLE", "unreachable", true);
    expect((await s.rig.t.wait(second.id, WAIT)).state).toBe("cancelled");
    await eventually(async () => (await stageRows(s, "t.pause")).find((r) => r.state === "stale" && r.attempts === 0));
    await sleep(800);
    expect(active(s.rig)).toEqual([]);
    await s.rig.svc.goOnline(s.lib);
    expect((await settled(s, "t.pause", [fp], ["done"]))[0]).toMatchObject({ attempts: 0 });
  });

  test("cloud_only_skipped_unless_hydrate", { timeout: 120_000 }, async () => {
    const s = await setup({ names: ["a.png", "b.png"] });
    const by = await probed(s, ["a.png", "b.png"]);
    overlay(s.root, { paths: { "b.png": { placeholder: "dataless" } } });
    const scan = await scanLibrary(s.rig.svc, s.lib, { full: true });
    expect((await s.rig.t.wait(scan.id, WAIT)).state).toBe("succeeded");
    await eventually(async () => (await exec(s.rig.catalogs.reader.file(s.lib, { fileId: by["b.png"]!.id }))).matches[0]?.state === "cloudOnly");
    const seen: Seen = [];
    s.rig.svc.registerStage(testStage("t.cloud", { seen }));
    await settled(s, "t.cloud", [by["a.png"]!.fp!]);
    await sleep(500);
    expect((await stageRows(s, "t.cloud")).map((r) => r.fp)).toEqual([by["a.png"]!.fp]);
    await setLibrary(s.rig.svc, s.lib, { hydrate: true }, ACTOR);
    await settled(s, "t.cloud", [by["b.png"]!.fp!], ["done"]);
    expect(seen.flat().find((f) => f.fp === by["b.png"]!.fp)).toMatchObject({ hydrate: true });
  });

  test("is_done_skips_the_job", { timeout: 90_000 }, async () => {
    const events: FileEvent[] = [];
    const rig = await liveRig({ register: (k) => registerTestStageKind(k) });
    rig.svc.onFile((e) => void (e.type === "stage" && events.push(e)));
    const s = await setup({ rig, names: ["a.png", "b.png"], stages: [testStage("t.cached", { isDone: () => Effect.succeed(true) })] });
    const by = await probed(s, ["a.png", "b.png"]);
    const rows = await settled(s, "t.cached", [by["a.png"]!.fp!, by["b.png"]!.fp!]);
    expect(rows.every((r) => r.state === "done" && r.code === "CACHED")).toBe(true);
    expect(stageJobs(s.rig)).toEqual([]);
    await eventually(async () => events.length === 2);
  });

  test("poison_item_fails_permanently_for_version", { timeout: 120_000 }, async () => {
    const s = await setup({ names: ["a.png"], stages: [testStage("t.poison", { knobs: (files) => ({ crashOn: files.map((f) => f.fp) }) })] });
    await probed(s, ["a.png"]);
    const rows = await eventually(async () => (await stageRows(s, "t.poison")).find((r) => r.state === "failed" || r.state === "done"));
    expect(rows).toMatchObject({ state: "failed", code: "POISON_ITEM" });
    expect(rows.attempts).toBeGreaterThanOrEqual(3);
    const n = stageJobs(s.rig).length;
    s.rig.svc.stages.kick(s.lib);
    await sleep(1000);
    expect(stageJobs(s.rig)).toHaveLength(n);
  });

  test("failed_job_retried_after_1h_three_times", { timeout: 120_000 }, async () => {
    const time = new ManualTime();
    time.now = Date.now();
    const s = await setup({
      names: ["a.png"],
      service: { clock: () => time.now, timers: time },
      stages: [testStage("t.retry", { knobs: () => ({ jobError: "TEST_FAIL" }) })],
    });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    for (let attempt = 1; attempt <= 3; attempt++) {
      const row = await eventually(async () => (await stageRows(s, "t.retry")).find((r) => r.fp === fp && r.state === "failed" && r.attempts === attempt));
      expect(row.code).toBe("TEST_FAIL");
      expect(stageJobs(s.rig)).toHaveLength(attempt);
      // Not before the hour.
      await time.advance(30 * 60_000);
      await sleep(300);
      expect(stageJobs(s.rig)).toHaveLength(attempt);
      // The pump's sweep (every 5 minutes) after the hour takes it again.
      await time.advance(40 * 60_000);
    }
    await sleep(800);
    expect(stageJobs(s.rig)).toHaveLength(3);
  });
});

describe("review fixes", () => {
  test("items_whose_file_changed_cost_no_attempt_and_rescan_their_dir", { timeout: 120_000 }, async () => {
    let calls = 0;
    const s = await setup({
      names: ["a.png"],
      stages: [testStage("t.changed", { knobs: (files): TestStageKnobs => (calls++ === 0 ? { failItems: Object.fromEntries(files.map((f) => [f.fp, "FILE_CHANGED"])) } : {}) })],
    });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    const rows = await settled(s, "t.changed", [fp], ["done"]);
    expect(rows[0]).toMatchObject({ state: "done", attempts: 0 });
    expect(calls).toBeGreaterThanOrEqual(2);
    // The file's directory (the root) was listed again: nothing else lists a directory here.
    const dirs = s.rig.t.jobs.list({ kind: "library.scan", library: s.lib, limit: 500 }).jobs.filter((j) => (j.params as { mode: string }).mode === "dirs");
    expect(dirs.map((j) => (j.params as { dirs: unknown[] }).dirs)).toEqual([[{ components: [] }]]);
  });

  test("a_stage_job_that_ends_before_its_submit_answers_still_settles", { timeout: 120_000 }, async () => {
    const s = await setup({ names: ["a.png"] });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    const engine = s.rig.t.engine as unknown as { submit: (...a: unknown[]) => unknown };
    const submit = engine.submit.bind(engine);
    // A submit that answers only after the job ended (its `onDone` came first).
    engine.submit = (owner, spec) =>
      Effect.flatMap(submit(owner, spec) as Effect.Effect<JobView, never>, (view) =>
        (spec as { kind: string }).kind === "test.stage" ? Effect.promise(() => s.rig.t.wait(view.id, WAIT).then(() => view)) : Effect.succeed(view),
      );
    s.rig.svc.registerStage(testStage("t.quick"));
    const rows = await settled(s, "t.quick", [fp], ["done"]);
    expect(rows[0]).toMatchObject({ state: "done", jobId: null });
    expect(s.rig.svc.stages.inflight.jobsOf(s.lib).filter((c) => c.stage === "t.quick")).toEqual([]);
  });

  test("a_prioritized_stage_is_dropped_when_its_dependency_fails_for_good", { timeout: 120_000 }, async () => {
    const s = await setup({
      names: ["a.png"],
      stages: [
        // Slow to crash, so the prioritize below comes while its job runs: the background pump
        // takes `t.dep` once the file is probed.
        testStage("t.dep", { knobs: (files) => ({ crashOn: files.map((f) => f.fp), delayMs: 2000 }) }),
        testStage("t.after", { dependsOn: ["t.dep"] }),
      ],
    });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    await eventually(async () => active(s.rig).some((j) => j.state === "running"));
    await exec(s.rig.svc.prioritize(fp, "t.after"));
    expect(s.rig.svc.stages.wanted()).toBe(1);
    await eventually(async () => (await stageRows(s, "t.dep")).find((r) => r.state === "failed" && r.attempts >= 3));
    await eventually(async () => s.rig.svc.stages.wanted() === 0);
    expect((await stageRows(s, "t.after")).filter((r) => r.state === "done")).toEqual([]);
  });

  test("a_stage_prioritized_after_its_dependency_failed_for_good_is_not_kept", { timeout: 120_000 }, async () => {
    const s = await setup({
      names: ["a.png"],
      stages: [testStage("t.dep", { knobs: (files) => ({ crashOn: files.map((f) => f.fp) }) }), testStage("t.after", { dependsOn: ["t.dep"] })],
    });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    await eventually(async () => (await stageRows(s, "t.dep")).find((r) => r.state === "failed" && r.attempts >= 3));
    // Nothing will make `t.dep` done: the entry would wait forever.
    await exec(s.rig.svc.prioritize(fp, "t.after"));
    expect(s.rig.svc.stages.wanted()).toBe(0);
    expect(await stageRows(s, "t.after")).toEqual([]);
  });
});

describe("a dependency that does not apply", () => {
  test("a_stage_prioritized_on_a_skipped_dependency_is_not_kept", { timeout: 120_000 }, async () => {
    // `t.dep` never applies, so its row is `skipped` and `t.after` can never run: whether the
    // prioritize itself skips the dependency, or it was skipped before, no entry is kept.
    const s = await setup({
      names: ["a.png", "b.png"],
      stages: [testStage("t.dep", { appliesTo: () => false }), testStage("t.after", { dependsOn: ["t.dep"] })],
    });
    const by = await probed(s, ["a.png", "b.png"]);
    await exec(s.rig.svc.prioritize(by["a.png"]!.fp!, "t.after"));
    await eventually(async () => s.rig.svc.stages.wanted() === 0);
    await eventually(async () => (await stageRows(s, "t.dep")).filter((r) => r.state === "skipped").length === 2);
    await exec(s.rig.svc.prioritize(by["b.png"]!.fp!, "t.after"));
    expect(s.rig.svc.stages.wanted()).toBe(0);
    expect((await stageRows(s, "t.after")).filter((r) => r.state === "done")).toEqual([]);
  });
});

describe("priority and restarts", () => {
  test("prioritize_jumps_the_queue_with_dependencies", { timeout: 120_000 }, async () => {
    const names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
    // No stage yet: the probes (they share the read pool) run to their end first.
    const s = await setup({ names });
    const by = await probed(s, names);
    const last = by["f.png"]!.fp!;
    // The read pool is shut while the stages are registered and the file is prioritized, and has
    // one slot when it opens: the queue is in the state the test wants, and the engine's order
    // (priority first) decides what runs first, never how fast this process happens to be.
    const read = s.rig.svc.book.pools(s.lib)!.read;
    s.rig.t.engine.definePool(read, 0);
    s.rig.svc.registerStage(testStage("t.a", { batch: 1, maxInFlight: 1, knobs: () => ({ delayMs: 200 }) }));
    s.rig.svc.registerStage(testStage("t.b", { batch: 1, maxInFlight: 1, dependsOn: ["t.a"] }));
    await eventually(async () => active(s.rig).length > 0);
    await exec(s.rig.svc.prioritize(last, "t.b"));
    // One background `t.a` (max in flight 1) and the prioritized one: nothing else was submitted.
    expect(active(s.rig)).toHaveLength(2);
    s.rig.t.engine.definePool(read, 1);
    const done = await settled(s, "t.b", [last], ["done"]);
    expect(done.find((r) => r.fp === last)?.state).toBe("done");
    // Both of its jobs ran at `normal`, and its `t.a` started before every other job did: with one
    // slot a `normal` job always goes before a `background` one, however slow this process is.
    const all = stageJobs(s.rig);
    const mine = all.filter((j) => (j.params as { items: string[] }).items.includes(last)).sort((x, y) => x.createdAt - y.createdAt);
    expect(mine.map((j) => j.priority)).toEqual(["normal", "normal"]);
    for (const j of all.filter((x) => !mine.includes(x))) {
      expect(j.startedAt === undefined || j.startedAt >= mine[0]!.startedAt!, `${j.id} started before the prioritized file's t.a`).toBe(true);
    }
    // Unknown stages and fingerprints are refused.
    expect((await Effect.runPromise(Effect.flip(s.rig.svc.prioritize(last, "t.none")))).code).toBe("INVALID_REQUEST");
    expect((await Effect.runPromise(Effect.flip(s.rig.svc.prioritize("fp1:nope", "t.a")))).code).toBe("FILE_NOT_FOUND");
    const states = await Effect.runPromise(s.rig.svc.stageStates(last));
    expect(states.map((x) => x.stage)).toEqual(["probe", "t.a", "t.b"]);
    expect(states.every((x) => x.library === s.lib && x.state === "done")).toBe(true);
  });

  test("restart_reconciles_queued_and_running", { timeout: 120_000 }, async () => {
    const stage = () => testStage("t.long", { knobs: () => ({ delayMs: 2500 }) });
    let s = await setup({ names: ["a.png"], stages: [stage()] });
    const fp = (await probed(s, ["a.png"]))["a.png"]!.fp!;
    const job = await eventually(async () => active(s.rig).find((j) => j.state === "running"));
    expect((await stageRows(s, "t.long"))[0]).toMatchObject({ state: "queued", jobId: job.id });
    await s.rig.crash();
    const rig = await liveRig({ serverData: s.rig.serverData, register: (k) => registerTestStageKind(k) });
    s = { ...s, rig };
    rig.svc.registerStage(stage());
    const rows = await settled(s, "t.long", [fp], ["done"]);
    expect(rows[0]).toMatchObject({ state: "done", attempts: 0 });
    // The same job finished it: nothing was submitted twice.
    expect(stageJobs(rig).map((j) => j.id)).toEqual([job.id]);
  });

  test("version_bump_requeues_only_that_stage", { timeout: 120_000 }, async () => {
    let s = await setup({ names: ["a.png", "b.png"], stages: [testStage("t.v"), testStage("t.other")] });
    const by = await probed(s, ["a.png", "b.png"]);
    const fps = [by["a.png"]!.fp!, by["b.png"]!.fp!];
    await settled(s, "t.v", fps, ["done"]);
    await settled(s, "t.other", fps, ["done"]);
    await s.rig.crash();
    const rig = await liveRig({ serverData: s.rig.serverData, register: (k) => registerTestStageKind(k) });
    s = { ...s, rig };
    const seenOther: Seen = [];
    rig.svc.registerStage(testStage("t.v", { version: "v2" }));
    rig.svc.registerStage(testStage("t.other", { seen: seenOther }));
    const v2 = await eventually(async () => {
      const rows = await stageRows(s, "t.v");
      return rows.length === 2 && rows.every((r) => r.version === "v2" && r.state === "done") ? rows : undefined;
    });
    expect(v2).toHaveLength(2);
    await sleep(500);
    expect(seenOther).toEqual([]);
    expect((await stageRows(s, "t.other")).every((r) => r.version === "v1" && r.state === "done")).toBe(true);
  });
});

describe("file events", () => {
  test("file_events_for_every_change", { timeout: 120_000 }, async () => {
    const events: FileEvent[] = [];
    const rig = await liveRig({ register: (k) => registerTestStageKind(k) });
    rig.svc.onFile((e) => void events.push(e));
    rig.svc.registerStage(testStage("t.ev"));
    const root = join(liveRoots(), "ev");
    files(root, ["keep.png", "edit.png", "gone.png", "old.png", "moved.png"]);
    const s = await setup({ rig, root });
    const by = await probed(s, ["keep.png", "edit.png", "gone.png", "old.png", "moved.png"]);
    const ids = Object.fromEntries(Object.entries(by).map(([n, r]) => [n, r.id]));
    const of = (type: FileEvent["type"], id: number) => events.some((e) => e.type === type && e.fileId === id && e.library === s.lib);
    for (const id of Object.values(ids)) expect(of("added", id)).toBe(true);
    await eventually(async () => Object.values(ids).every((id) => of("probed", id) && of("stage", id)));
    expect(events.find((e) => e.type === "probed" && e.fileId === ids["keep.png"])?.fp).toBe(by["keep.png"]!.fp);
    // Edit one, delete one, rename one (same inode), move one by content (a new inode).
    const { renameSync, rmSync, copyFileSync } = await import("node:fs");
    writeFileSync(join(root, "edit.png"), Buffer.concat([PNG_1X1, Buffer.from("x")]));
    rmSync(join(root, "gone.png"));
    renameSync(join(root, "old.png"), join(root, "new.png"));
    copyFileSync(join(root, "moved.png"), join(root, "moved-copy.png"));
    utimesSync(join(root, "moved-copy.png"), BASE_S + 4, BASE_S + 4);
    rmSync(join(root, "moved.png"));
    events.length = 0;
    const scan = await scanLibrary(rig.svc, s.lib, { full: true });
    expect((await rig.t.wait(scan.id, WAIT)).state).toBe("succeeded");
    await eventually(async () => of("changed", ids["edit.png"]!) && of("missing", ids["gone.png"]!) && of("renamed", ids["old.png"]!) && of("moved", ids["moved.png"]!));
    expect(of("changed", ids["keep.png"]!)).toBe(false);
    // The edited file is probed again; the moved one went missing, then kept its id and history.
    await eventually(async () => of("probed", ids["edit.png"]!));
    expect(events.filter((e) => e.type === "missing").map((e) => e.fileId).sort()).toEqual([ids["gone.png"], ids["moved.png"]].sort());
    expect(events.some((e) => e.type === "added" && e.fileId === ids["old.png"])).toBe(false);
  });
});
