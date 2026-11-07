// Pools and the pure scheduler (5C-1 plan Task 8, Decisions 33–35). Properties run 1,000
// times each; fast-check prints the seed and the shrunk counterexample on a failure.
import type { Priority } from "@finisher/contracts";
import fc from "fast-check";
import { describe, expect, test } from "vitest";
import { defaultPools, makePools, type Pools, slotCounts } from "../../src/jobs/pools.ts";
import {
  AGING_MS,
  compareJobs,
  type Dispatch,
  QueueIndex,
  type QueuedJob,
  type SchedState,
  schedule,
  type WarmWorkerView,
  warmOverrun,
} from "../../src/jobs/schedule.ts";

const RUNS = { numRuns: 1000 };
const NOW = 10_000_000;
const MIN = 60_000;

function job(id: string, over: Partial<QueuedJob> = {}): QueuedJob {
  return { id, priority: "normal", createdAt: NOW - 1000, resources: ["cpu"], ...over };
}

function state(jobs: ReadonlyArray<QueuedJob>, pools: Pools, warm: SchedState["warm"] = new Map()): SchedState {
  const queue = new QueueIndex();
  for (const j of jobs) queue.add(j);
  return { queue, pools, warm };
}

const ids = (ds: Dispatch[]): string[] => ds.flatMap((d) => ("job" in d ? [d.job] : []));

// ---- generators ----

const POOL_NAMES = ["cpu", "gpu", "net", "io:a"] as const;
const RESOURCE = fc.constantFrom("cpu", "cpu", "gpu", "net", "io:a", "io:missing");
const PRIORITY = fc.constantFrom<Priority>("interactive", "normal", "background");

const FAMILIES = ["f", "g"] as const;

interface World {
  readonly sizes: Record<string, number>;
  readonly used: Record<string, number>;
  readonly stuck: string[];
  readonly jobs: QueuedJob[];
  readonly warm: Record<string, { slots: number; workers: WarmWorkerView[] }>;
}

const workerArb = fc.record({
  key: fc.constantFrom("A", "B", "C"),
  state: fc.constantFrom<WarmWorkerView["state"]>("spawning", "ready", "busy"),
  idle: fc.integer({ min: 0, max: 1000 }),
});
const familyArb = fc.record({ slots: fc.integer({ min: 1, max: 2 }), workers: fc.array(workerArb, { maxLength: 2 }) });

const world: fc.Arbitrary<World> = fc
  .record({
    sizes: fc.record({ cpu: fc.integer({ min: 1, max: 4 }), gpu: fc.integer({ min: 0, max: 2 }), net: fc.integer({ min: 0, max: 3 }), "io:a": fc.integer({ min: 0, max: 2 }) }),
    usedFrac: fc.array(fc.double({ min: 0, max: 1, noNaN: true }), { minLength: 4, maxLength: 4 }),
    stuck: fc.subarray([...POOL_NAMES], { maxLength: 1 }),
    raw: fc.array(
      fc.record({
        priority: PRIORITY,
        age: fc.integer({ min: 0, max: 45 * MIN }),
        resources: fc.array(RESOURCE, { minLength: 0, maxLength: 3 }),
        delay: fc.option(fc.integer({ min: -5000, max: 5000 }), { nil: undefined }),
        warm: fc.option(fc.record({ family: fc.constantFrom(...FAMILIES), key: fc.constantFrom("A", "B", "C") }), { nil: undefined, freq: 3 }),
        warmGpu: fc.boolean(),
      }),
      { maxLength: 60 },
    ),
    f: familyArb,
    g: fc.option(familyArb, { nil: undefined }),
  })
  .map(({ sizes, usedFrac, stuck, raw, f, g }) => {
    const used: Record<string, number> = {};
    POOL_NAMES.forEach((n, i) => (used[n] = Math.floor((usedFrac[i] ?? 0) * ((sizes as Record<string, number>)[n] ?? 0))));
    const jobs = raw.map((r, i) => {
      const j: QueuedJob = {
        id: `j${String(i).padStart(3, "0")}`,
        priority: r.warm !== undefined ? "interactive" : r.priority,
        createdAt: NOW - r.age,
        resources: r.warm !== undefined ? (r.warmGpu ? ["cpu", "gpu"] : ["cpu"]) : r.resources,
        ...(r.delay !== undefined ? { nextRunAt: NOW + r.delay } : {}),
        ...(r.warm !== undefined ? { warm: r.warm } : {}),
      };
      return j;
    });
    const fam = (x: { slots: number; workers: Array<{ key: string; state: WarmWorkerView["state"]; idle: number }> }) => ({
      slots: x.slots,
      workers: x.workers.slice(0, x.slots).map((w) => ({ key: w.key, state: w.state, ...(w.state === "ready" ? { idleSince: w.idle } : {}) })),
    });
    const warm: World["warm"] = { f: fam(f) };
    if (g !== undefined) warm.g = fam(g);
    return { sizes, used, stuck, jobs, warm };
  });

function warmMap(w: World): SchedState["warm"] {
  return new Map(Object.entries(w.warm));
}

function build(w: World): SchedState {
  const pools = makePools(w.sizes);
  for (const [name, n] of Object.entries(w.used)) pools.take(Array(n).fill(name), { [name]: n });
  for (const s of w.stuck) pools.markStuck(s, true);
  return state(w.jobs, pools, warmMap(w));
}

/**
 * An independent brute force: every due job in global order, the rules applied one by one;
 * warm spawns and retirements in a second pass over the warm jobs left waiting.
 */
function reference(w: World, now: number): Dispatch[] {
  const used = new Map(Object.entries(w.used));
  const overrun = Object.values(w.warm).reduce((n, f) => n + f.slots, 0);
  const fams = new Map(Object.entries(w.warm).map(([k, f]) => [k, { slots: f.slots, workers: f.workers.map((x) => ({ ...x, retiring: false })), spawning: new Set<string>() }]));
  const out: Dispatch[] = [];
  const fits = (names: ReadonlyArray<string>, extraCpu: number): boolean =>
    [...slotCounts(names)].every(([name, n]) => {
      const cap = w.sizes[name];
      if (cap === undefined || w.stuck.includes(name)) return false;
      return (used.get(name) ?? 0) + n <= cap + (name === "cpu" ? extraCpu : 0);
    });
  const take = (names: ReadonlyArray<string>): void => {
    for (const n of names) used.set(n, (used.get(n) ?? 0) + 1);
  };
  const due = w.jobs.filter((j) => (j.nextRunAt ?? 0) <= now).sort((a, b) => compareJobs(a, b, now));
  const dueKeys = new Set(due.flatMap((j) => (j.warm ? [`${j.warm.family}/${j.warm.key}`] : [])));
  const waiting: QueuedJob[] = [];
  for (const j of due) {
    if (j.warm === undefined) {
      if (fits(j.resources, 0)) {
        take(j.resources);
        out.push({ job: j.id, mode: "cold" });
      }
      continue;
    }
    const fam = fams.get(j.warm.family);
    if (fam === undefined || !fits(j.resources, overrun)) continue;
    const ready = fam.workers.find((x) => x.key === j.warm?.key && x.state === "ready" && !x.retiring);
    if (ready === undefined) {
      waiting.push(j);
      continue;
    }
    ready.state = "busy";
    take(j.resources);
    out.push({ job: j.id, mode: "warm", family: j.warm.family, key: j.warm.key });
  }
  for (const j of waiting) {
    const { family, key } = j.warm!;
    const fam = fams.get(family)!;
    if (fam.spawning.has(key) || fam.workers.some((x) => x.key === key && x.state === "spawning")) continue;
    if (fam.workers.filter((x) => !x.retiring).length + fam.spawning.size < fam.slots) {
      fam.spawning.add(key);
      out.push({ mode: "spawnWarm", family, key });
      continue;
    }
    if (fam.workers.some((x) => x.key === key && !x.retiring)) continue;
    const candidates = fam.workers
      .map((x, i) => ({ x, i }))
      .filter(({ x }) => x.state === "ready" && !x.retiring && x.key !== key && !dueKeys.has(`${family}/${x.key}`));
    if (candidates.length === 0) continue;
    const oldest = candidates.reduce((a, b) => ((b.x.idleSince ?? Infinity) < (a.x.idleSince ?? Infinity) ? b : a));
    oldest.x.retiring = true;
    fam.spawning.add(key);
    out.push({ mode: "spawnWarm", family, key, retire: oldest.i });
  }
  return out;
}

describe("pools", () => {
  test("default_pools_and_take_give", () => {
    expect(defaultPools(8).snapshot()).toEqual([
      { name: "cpu", size: 7, used: 0, stuck: false },
      { name: "gpu", size: 1, used: 0, stuck: false },
      { name: "hwenc", size: 2, used: 0, stuck: false },
      { name: "net", size: 4, used: 0, stuck: false },
    ]);
    expect(defaultPools(1).size("cpu")).toBe(1);
    const p = makePools({ cpu: 2, gpu: 1 });
    expect(p.take(["cpu", "gpu"])).toBe(true);
    expect(p.take(["cpu", "gpu"])).toBe(false);
    expect(p.used("cpu")).toBe(1);
    expect(p.take(["cpu", "cpu"])).toBe(false);
    expect(p.take(["cpu", "cpu"], { cpu: 1 })).toBe(true);
    expect(p.used("cpu")).toBe(3);
    p.give(["cpu", "cpu", "cpu", "cpu"]);
    expect(p.used("cpu")).toBe(0);
    expect(p.take(["io:root1"])).toBe(false);
    p.define("io:root1", 1);
    expect(p.take(["io:root1"])).toBe(true);
    p.markStuck("gpu", true);
    p.give(["gpu"]);
    expect(p.take(["gpu"])).toBe(false);
    expect(p.stuck("gpu")).toBe(true);
    p.markStuck("gpu", false);
    expect(p.take(["gpu"])).toBe(true);
    expect(() => p.define("disk", 1)).toThrow(/bad pool name/);
    expect(() => p.define("cpu", -1)).toThrow(/size/);
    p.define("cpu", 5);
    expect(p.size("cpu")).toBe(5);
    expect(p.take([])).toBe(true);
  });
});

describe("schedule", () => {
  test("pools_never_over_committed", () => {
    fc.assert(
      fc.property(world, (w) => {
        const s = build(w);
        const ds = schedule(s, NOW);
        const byId = new Map(w.jobs.map((j) => [j.id, j]));
        for (const d of ds) {
          if (!("job" in d)) continue;
          const j = byId.get(d.job)!;
          for (const name of j.resources) expect(s.pools.stuck(name)).toBe(false);
          const overrun = d.mode === "warm" ? warmOverrun(s.warm) : {};
          expect(s.pools.take(j.resources, overrun), `${d.mode} ${d.job}`).toBe(true);
        }
        for (const name of POOL_NAMES) {
          const cap = (w.sizes as Record<string, number>)[name] ?? 0;
          const extra = name === "cpu" ? warmOverrun(s.warm).cpu : 0;
          expect(s.pools.used(name)).toBeLessThanOrEqual(Math.max(cap + extra, w.used[name] ?? 0));
        }
        for (const [family, f] of Object.entries(w.warm)) {
          const warm = ds.filter((d) => d.mode === "warm" && d.family === family).length;
          expect(warm).toBeLessThanOrEqual(f.workers.filter((x) => x.state === "ready").length);
        }
      }),
      RUNS,
    );
  });

  test("priority_then_fifo", () => {
    const pools = makePools({ cpu: 100 });
    const jobs = [
      job("n-old", { createdAt: NOW - 5000 }),
      job("b", { priority: "background", createdAt: NOW - 9000 }),
      job("i-new", { priority: "interactive", createdAt: NOW - 10 }),
      job("n-b", { createdAt: NOW - 3000 }),
      job("n-a", { createdAt: NOW - 3000 }),
    ];
    expect(ids(schedule(state(jobs, pools), NOW))).toEqual(["i-new", "n-old", "n-a", "n-b", "b"]);
    const one = makePools({ cpu: 1 });
    expect(ids(schedule(state(jobs, one), NOW))).toEqual(["i-new"]);
    // With room for all, the order is the sorted order of every due job.
    fc.assert(
      fc.property(world, (w) => {
        const cold = w.jobs.filter((j) => j.warm === undefined).map((j) => ({ ...j, resources: ["cpu"] }));
        const ds = schedule(state(cold, makePools({ cpu: 1000 })), NOW);
        const want = cold.filter((j) => (j.nextRunAt ?? 0) <= NOW).sort((a, b) => compareJobs(a, b, NOW)).map((j) => j.id);
        expect(ids(ds)).toEqual(want);
      }),
      RUNS,
    );
  });

  test("aging_raises_one_level_per_10_minutes", () => {
    const one = (): Pools => makePools({ cpu: 1 });
    const first = (jobs: QueuedJob[]): string | undefined => ids(schedule(state(jobs, one()), NOW))[0];
    // Background after 20 minutes competes as interactive; the older wins the tie.
    expect(first([job("i", { priority: "interactive", createdAt: NOW - 1000 }), job("b", { priority: "background", createdAt: NOW - 20 * MIN })])).toBe("b");
    // After 10 minutes it is only normal.
    expect(first([job("i", { priority: "interactive", createdAt: NOW - 1000 }), job("b", { priority: "background", createdAt: NOW - 10 * MIN })])).toBe("i");
    expect(first([job("n", { createdAt: NOW - 11 * MIN }), job("i", { priority: "interactive", createdAt: NOW - 1000 })])).toBe("n");
    expect(first([job("n", { createdAt: NOW - 9 * MIN }), job("i", { priority: "interactive", createdAt: NOW - 1000 })])).toBe("i");
    // Never above interactive: an hour-old background job does not pass an older interactive one.
    expect(first([job("b", { priority: "background", createdAt: NOW - 60 * MIN }), job("i", { priority: "interactive", createdAt: NOW - 61 * MIN })])).toBe("i");
    expect(AGING_MS).toBe(10 * MIN);
  });

  test("blocked_head_does_not_hide_runnable_jobs", () => {
    const pools = makePools({ cpu: 2, gpu: 1 });
    pools.take(["gpu"]);
    const jobs = [job("gpu-old", { resources: ["gpu", "cpu"], createdAt: NOW - 9000 }), job("gpu-2", { resources: ["gpu", "cpu"] }), job("cpu", { createdAt: NOW - 10 })];
    expect(ids(schedule(state(jobs, pools), NOW))).toEqual(["cpu"]);
    // An undefined pool blocks only the jobs that need it.
    const p2 = makePools({ cpu: 1 });
    expect(ids(schedule(state([job("io", { resources: ["io:x"], createdAt: 0 }), job("c")], p2), NOW))).toEqual(["c"]);
  });

  test("warm_pool_is_separate", () => {
    const pools = makePools({ cpu: 1 });
    pools.take(["cpu"]);
    const warm = new Map([["f", { slots: 1, workers: [{ key: "A", state: "ready" as const, idleSince: 5 }] }]]);
    const jobs = [job("cold", { createdAt: 0 }), job("w", { priority: "interactive", warm: { family: "f", key: "A" } })];
    expect(schedule(state(jobs, pools, warm), NOW)).toEqual([{ job: "w", mode: "warm", family: "f", key: "A" }]);
    // No worker yet: room in the pool spawns one; the job waits for its hello.
    const empty = new Map([["f", { slots: 1, workers: [] }]]);
    expect(schedule(state(jobs.slice(1), pools, empty), NOW)).toEqual([{ mode: "spawnWarm", family: "f", key: "A" }]);
    const spawning = new Map([["f", { slots: 2, workers: [{ key: "A", state: "spawning" as const }] }]]);
    expect(schedule(state([...jobs.slice(1), job("w2", { priority: "interactive", warm: { family: "f", key: "A" } })], pools, spawning), NOW)).toEqual([]);
  });

  test("warm_keys_never_share_a_worker", () => {
    fc.assert(
      fc.property(world, (w) => {
        const ds = schedule(build(w), NOW);
        const byId = new Map(w.jobs.map((j) => [j.id, j]));
        const served = new Map<string, number>();
        for (const d of ds) {
          if (d.mode === "warm") {
            expect(byId.get(d.job)?.warm).toEqual({ family: d.family, key: d.key });
            served.set(`${d.family}/${d.key}`, (served.get(`${d.family}/${d.key}`) ?? 0) + 1);
          }
        }
        for (const [fk, n] of served) {
          const [family, key] = fk.split("/") as [string, string];
          expect(n).toBeLessThanOrEqual(w.warm[family]!.workers.filter((x) => x.key === key && x.state === "ready").length);
        }
        const spawns = ds.filter((d) => d.mode === "spawnWarm");
        expect(new Set(spawns.map((d) => `${d.family}/${d.key}`)).size).toBe(spawns.length);
        const dueKeys = new Set(w.jobs.filter((j) => j.warm && (j.nextRunAt ?? 0) <= NOW).map((j) => `${j.warm!.family}/${j.warm!.key}`));
        for (const d of spawns) {
          if (d.retire === undefined) continue;
          const retired = w.warm[d.family]!.workers[d.retire]!;
          expect(retired.key).not.toBe(d.key);
          expect(retired.state).toBe("ready");
          expect(dueKeys.has(`${d.family}/${retired.key}`), "a key with a due job is never retired").toBe(false);
        }
        // Spawns and retirements come after every dispatch.
        const firstSpawn = ds.findIndex((d) => d.mode === "spawnWarm");
        if (firstSpawn >= 0) expect(ds.slice(firstSpawn).every((d) => d.mode === "spawnWarm")).toBe(true);
      }),
      RUNS,
    );
  });

  test("full_pool_retires_the_longest_idle_other_key", () => {
    const pools = makePools({ cpu: 4 });
    const wj = job("b", { priority: "interactive", warm: { family: "f", key: "B" } });
    const one = new Map([["f", { slots: 1, workers: [{ key: "A", state: "ready" as const, idleSince: 100 }] }]]);
    expect(schedule(state([wj], pools, one), NOW)).toEqual([{ mode: "spawnWarm", family: "f", key: "B", retire: 0 }]);
    const two = new Map([
      ["f", { slots: 2, workers: [{ key: "C", state: "ready" as const, idleSince: 100 }, { key: "A", state: "ready" as const, idleSince: 50 }] }],
    ]);
    expect(schedule(state([wj], pools, two), NOW)).toEqual([{ mode: "spawnWarm", family: "f", key: "B", retire: 1 }]);
    const busy = new Map([["f", { slots: 1, workers: [{ key: "A", state: "busy" as const }] }]]);
    expect(schedule(state([wj], pools, busy), NOW)).toEqual([]);
  });

  test("warm_overrun_covers_every_family", () => {
    // cpu 1 held by a cold job and by f's busy warm job; g has a ready worker.
    const pools = makePools({ cpu: 1 });
    pools.take(["cpu"]);
    const warm = new Map([
      ["f", { slots: 1, workers: [{ key: "A", state: "busy" as const }] }],
      ["g", { slots: 1, workers: [{ key: "A", state: "ready" as const, idleSince: 1 }] }],
    ]);
    expect(warmOverrun(warm)).toEqual({ cpu: 2 });
    pools.take(["cpu"], warmOverrun(warm));
    const g1 = job("g1", { priority: "interactive", warm: { family: "g", key: "A" } });
    expect(schedule(state([g1], pools, warm), NOW)).toEqual([{ job: "g1", mode: "warm", family: "g", key: "A" }]);
    expect(pools.take(["cpu"], warmOverrun(warm)), "what schedule returned is takeable").toBe(true);
    expect(pools.used("cpu")).toBe(3);
    // At size + every family's slots, nothing more runs, warm or cold.
    expect(schedule(state([g1, job("c")], pools, warm), NOW)).toEqual([]);
  });

  test("retirement_never_takes_a_worker_a_due_job_could_use", () => {
    const pools = makePools({ cpu: 4, gpu: 1 });
    const warm = () => new Map([["f", { slots: 1, workers: [{ key: "A", state: "ready" as const, idleSince: 5 }] }]]);
    const b = job("b", { priority: "interactive", createdAt: NOW - 5000, warm: { family: "f", key: "B" } });
    const a = job("a", { priority: "interactive", createdAt: NOW - 10, warm: { family: "f", key: "A" } });
    // The older B job does not retire A's worker while A has a due job: A runs on it.
    expect(schedule(state([b, a], pools, warm()), NOW)).toEqual([{ job: "a", mode: "warm", family: "f", key: "A" }]);
    // Also when A's job is blocked on another pool: its key keeps its worker.
    pools.markStuck("gpu", true);
    const aGpu = job("a", { ...a, resources: ["cpu", "gpu"] });
    expect(schedule(state([b, aGpu], pools, warm()), NOW)).toEqual([]);
    pools.markStuck("gpu", false);
    // Without a due A job, B retires it.
    expect(schedule(state([b], pools, warm()), NOW)).toEqual([{ mode: "spawnWarm", family: "f", key: "B", retire: 0 }]);
  });

  test("alternating_keys_do_not_ping_pong", () => {
    // One slot, a ready A worker, and B, A, B, A queued: ticks until the queue is empty, each
    // warm job finishing at once and each spawn becoming ready at the next tick.
    const pools = makePools({ cpu: 4 });
    const queue = new QueueIndex();
    ["B", "A", "B", "A"].forEach((key, i) => queue.add(job(`${key}${i}`, { priority: "interactive", createdAt: NOW - 100 + i, warm: { family: "f", key } })));
    let workers: WarmWorkerView[] = [{ key: "A", state: "ready", idleSince: 0 }];
    let retirements = 0;
    for (let tick = 0; tick < 10 && queue.size() > 0; tick++) {
      const ds = schedule({ queue, pools, warm: new Map([["f", { slots: 1, workers }]]) }, NOW + tick);
      for (const d of ds) {
        if (d.mode === "warm") queue.remove(d.job);
        else if (d.mode === "spawnWarm") {
          if (d.retire !== undefined) retirements++;
          workers = [...workers.filter((_, i) => i !== d.retire), { key: d.key, state: "ready", idleSince: tick }];
        }
      }
    }
    expect(queue.size()).toBe(0);
    expect(retirements).toBe(1);
  });

  test("next_run_at_is_respected", () => {
    const pools = makePools({ cpu: 4 });
    const jobs = [job("later", { createdAt: 0, nextRunAt: NOW + 1 }), job("due", { createdAt: 5, nextRunAt: NOW }), job("plain", { createdAt: 10 })];
    expect(ids(schedule(state(jobs, pools), NOW))).toEqual(["due", "plain"]);
    expect(ids(schedule(state(jobs, pools), NOW + 1))).toEqual(["later", "due", "plain"]);
  });

  test("stuck_pool_never_dispatched", () => {
    const pools = makePools({ cpu: 4, "io:r": 2 });
    pools.markStuck("io:r", true);
    const jobs = [job("io", { resources: ["io:r", "cpu"], createdAt: 0 }), job("c")];
    expect(ids(schedule(state(jobs, pools), NOW))).toEqual(["c"]);
  });

  test("matches_brute_force_reference", () => {
    fc.assert(
      fc.property(world, fc.integer({ min: -3000, max: 3000 }), (w, shift) => {
        const now = NOW + shift;
        expect(schedule(build(w), now)).toEqual(reference(w, now));
      }),
      RUNS,
    );
  });

  test("queue_index_add_remove", () => {
    fc.assert(
      fc.property(world, fc.array(fc.nat(), { maxLength: 30 }), (w, picks) => {
        const q = new QueueIndex();
        for (const j of w.jobs) q.add(j);
        if (w.jobs[0] !== undefined) q.add(w.jobs[0]);
        const removed = new Set<string>();
        for (const p of picks) {
          const j = w.jobs[p % Math.max(1, w.jobs.length)];
          if (j === undefined) continue;
          q.remove(j.id);
          removed.add(j.id);
        }
        const left = w.jobs.filter((j) => !removed.has(j.id));
        expect(q.size()).toBe(left.length);
        for (const j of left) expect(q.get(j.id)).toBe(j);
        const s: SchedState = { queue: q, pools: makePools({ cpu: 1000, gpu: 1000, net: 1000, "io:a": 1000 }), warm: new Map() };
        const cold = left.filter((j) => j.warm === undefined && !j.resources.includes("io:missing") && (j.nextRunAt ?? 0) <= NOW);
        expect(ids(schedule(s, NOW)).sort()).toEqual(cold.map((j) => j.id).sort());
      }),
      RUNS,
    );
  });

  test("schedule_is_fast", () => {
    const pools = makePools({ cpu: 7, gpu: 1, net: 4, "io:a": 2, "io:b": 2 });
    const sets = Array.from({ length: 20 }, (_, i) => [["cpu"], ["cpu", "gpu"], ["net"], ["cpu", "io:a"], ["io:b", "cpu"]][i % 5]!.concat(i >= 5 ? [`io:s${i}`] : []));
    for (let i = 5; i < 20; i++) pools.define(`io:s${i}`, 1);
    const q = new QueueIndex();
    const prios: Priority[] = ["interactive", "normal", "background"];
    for (let i = 0; i < 50_000; i++) q.add({ id: `j${i}`, priority: prios[i % 3]!, createdAt: NOW - (i % 7919) * 100, resources: sets[i % 20]! });
    const s: SchedState = { queue: q, pools, warm: new Map() };
    const times: number[] = [];
    for (let i = 0; i < 25; i++) {
      const t = performance.now();
      schedule(s, NOW);
      times.push(performance.now() - t);
    }
    times.sort((a, b) => a - b);
    const median = times[Math.floor(times.length / 2)]!;
    console.log(`schedule over 50,000 queued, 20 resource sets: median ${median.toFixed(3)} ms`);
    expect(median).toBeLessThanOrEqual(5);
  });
});
