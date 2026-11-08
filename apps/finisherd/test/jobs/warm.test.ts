// Warm workers (5C-1 plan Task 11, Decisions 6, 20, 23, 31, 35, 36; spec §6.3), mostly with
// real `fin-workers test --warm` processes. `proxied()` wraps fin-node's host: it records
// groups, spawns and `start` times, may change events (a `ready`'s memory figure) and may hold
// a process's events back (a replacement whose `hello` has not arrived yet).
import type { HostMsg, JobGroupSpec, JobHostEvent, JobSpawnRequest, TestWarmResult } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { Engine } from "../../src/jobs/engine.ts";
import type { JobHostFactory } from "../../src/jobs/host.ts";
import { makeKindRegistry } from "../../src/jobs/kinds.ts";
import { makePools } from "../../src/jobs/pools.ts";
import { QueueIndex, type QueuedJob, schedule } from "../../src/jobs/schedule.ts";
import { FakeHost } from "../support/fake-host.ts";
import { closeTestEngines, failure, hostOf, OWNER, run, type TestEngine, testClock, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

const linux = process.platform === "linux";
const MiB = 1024 * 1024;

interface Rec {
  readonly groups: Array<{ group: number; spec: JobGroupSpec }>;
  readonly spawns: Array<{ group: number; req: JobSpawnRequest; proc: number }>;
  readonly sent: Array<{ proc: number; msg: HostMsg }>;
  /** `performance.now()` at each job's `start`. */
  readonly starts: Map<string, number>;
  /** Holds every event of the next process spawned until `release()`. */
  holdNext(): void;
  release(): void;
  /** Each `ready` forwarded with this memory figure. */
  readyMemory: number | undefined;
}

function proxied(): { factory: JobHostFactory; rec: Rec } {
  let deliver: (e: JobHostEvent) => void = () => {};
  let holdNext = false;
  const holding = new Set<number>();
  const held: JobHostEvent[] = [];
  const rec: Rec = {
    groups: [],
    spawns: [],
    sent: [],
    starts: new Map(),
    readyMemory: undefined,
    holdNext: () => void (holdNext = true),
    release() {
      holding.clear();
      for (const e of held.splice(0)) deliver(e);
    },
  };
  const map = (e: JobHostEvent): JobHostEvent | null => {
    if ("proc" in e && holding.has(e.proc)) {
      held.push(e);
      return null;
    }
    if (e.type === "message" && e.msg.t === "ready" && rec.readyMemory !== undefined) return { ...e, memory: rec.readyMemory };
    return e;
  };
  const factory: JobHostFactory = async (opts, onEvent) => {
    const { JobHost } = await import("@finisher/fin-node");
    deliver = onEvent;
    const h = await JobHost.start(opts, (e) => {
      const m = map(e);
      if (m !== null) onEvent(m);
    });
    return {
      caps: () => h.caps(),
      openGroup: (s) => {
        const group = h.openGroup(s);
        rec.groups.push({ group, spec: s });
        return group;
      },
      spawn: async (g, r) => {
        const hold = holdNext;
        holdNext = false;
        const proc = await h.spawn(g, r);
        if (hold) holding.add(proc);
        rec.spawns.push({ group: g, req: r, proc });
        return proc;
      },
      send: (p, m) => {
        if (m.t === "start") rec.starts.set(m.job.id, performance.now());
        rec.sent.push({ proc: p, msg: m });
        return h.send(p, m);
      },
      watch: (p, w) => h.watch(p, w),
      closeStdin: (p) => h.closeStdin(p),
      killGroup: (g, r) => h.killGroup(g, r),
      closeGroup: (g) => h.closeGroup(g),
      shutdown: (ms) => h.shutdown(ms),
      killProcForTest: h.killProcForTest,
      abandonForTest: h.abandonForTest,
    };
  };
  return { factory, rec };
}

const resultOf = (v: JobView): TestWarmResult => v.result as TestWarmResult;
const warmList = (t: TestEngine) => t.engine.workers().warm;
const readyWorker = (t: TestEngine, key = "") => until(() => warmList(t).find((w) => w.state === "ready" && w.key === key));
const warmExits = (t: TestEngine) => t.sink.exits.filter((x) => x.role === "worker" && x.family === "test");
/** The engine's process id of the warm worker with `pid`. */
const procOfPid = (t: TestEngine, pid: number) => until(() => t.engine.workers().processes.find((p) => p.pid === pid)?.proc);

describe("hand-off", () => {
  test("back_to_back_on_one_process", async () => {
    const t = await testEngine();
    const results: TestWarmResult[] = [];
    for (let i = 0; i < 100; i++) {
      const v = await t.runJob("test.warm", { value: i });
      expect(v).toMatchObject({ state: "succeeded", warm: true });
      results.push(resultOf(v));
    }
    expect(new Set(results.map((r) => r.pid)).size).toBe(1);
    expect(results.map((r) => r.served)).toEqual(Array.from({ length: 100 }, (_, i) => i + 1));
    expect(results.map((r) => r.value)).toEqual(Array.from({ length: 100 }, (_, i) => i));
    expect(warmList(t)).toMatchObject([{ family: "test", key: "", state: "ready", served: 100, pid: results[0]?.pid }]);
    expect(t.sink.done.every((d) => d.warm)).toBe(true);
  });

  // Measured from `submit` to the `start` handed to the host's `send` (the message is then on
  // the process's writer queue; the writer thread's `write(2)` is not part of the figure). 500
  // samples keep it light for CI; `scripts/jobs-budget.ts` (release builds) records 1,000 on an
  // otherwise idle machine. Beside the rest of the suite the tail can measure the machine (a few
  // scheduler stalls are the p99 of 500), so one pass over budget is measured again once.
  test("dispatch_p99_under_5ms_under_load", { timeout: 180_000 }, async () => {
    const { factory, rec } = proxied();
    // cpu 4: four short cold jobs run at a time, so cold spawns are always in flight while 10,000 wait.
    const t = await testEngine({ jobHost: factory, cores: 5 });
    for (let i = 0; i < 10_000; i++) await t.submit("test.sleep", { ms: 20 });
    expect(t.engine.workers().pools.find((p) => p.name === "cpu")).toMatchObject({ size: 4, used: 4 });
    await t.runJob("test.warm", { value: "warm-up" });
    const N = 500;
    const pass = async (): Promise<{ lat: number[]; p50: number; p99: number }> => {
      const lat: number[] = [];
      for (let i = 0; i < N; i++) {
        const t0 = performance.now();
        const v = await t.submit("test.warm", { value: i });
        expect(await t.wait(v.id)).toMatchObject({ state: "succeeded" });
        lat.push((rec.starts.get(v.id) ?? Infinity) - t0);
      }
      lat.sort((a, b) => a - b);
      const cold = t.engine.workers().processes.filter((p) => p.family === "test" && p.job !== undefined).length;
      const p50 = lat[Math.floor(N * 0.5)] as number;
      const p99 = lat[Math.ceil(N * 0.99) - 1] as number;
      console.log(`warm hand-off on ${process.platform}: p50 ${p50.toFixed(3)} ms, p99 ${p99.toFixed(3)} ms, max ${(lat[N - 1] as number).toFixed(3)} ms (cold processes now: ${cold})`);
      return { lat, p50, p99 };
    };
    let m = await pass();
    if (linux && m.p99 > 5) m = await pass();
    if (linux) expect(m.p99).toBeLessThanOrEqual(5);
    expect(m.lat.every(Number.isFinite)).toBe(true);
  });

  test("ready_worker_bypasses_schedule", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory });
    await t.runJob("test.warm", { value: 0 });
    const engine = t.engine as Engine;
    const real = engine.schedule;
    let id: string | undefined;
    // Scheduling passes before the job's `start` was written (later ones follow its end).
    let calls = 0;
    engine.schedule = (s, now) => {
      if (id === undefined || !rec.starts.has(id)) calls++;
      return real(s, now);
    };
    const v = Effect.runSync(engine.submit(OWNER, { kind: "test.warm", params: { value: 1 } }));
    id = v.id;
    // Decided in the submit's own turn, without a scheduling pass.
    expect(calls).toBe(0);
    expect(v.state).toBe("running");
    expect(resultOf(await t.wait(v.id)).served).toBe(2);
    expect(rec.starts.has(v.id)).toBe(true);
    expect(calls).toBe(0);
  });
});

describe("pool", () => {
  test("first_submit_prewarms", async () => {
    const t = await testEngine();
    expect(warmList(t)).toEqual([]);
    const v = await t.submit("test.warm", { value: 1 });
    expect(warmList(t)).toMatchObject([{ state: "spawning", served: 0 }]);
    const first = resultOf(await t.wait(v.id));
    expect(first.served).toBe(1);
    // The worker stays for the next interactive job.
    expect(await readyWorker(t)).toMatchObject({ pid: first.pid, served: 1 });
    expect(resultOf(await t.runJob("test.warm", { value: 2 }))).toMatchObject({ pid: first.pid, served: 2 });
  });

  test("warm_family_starts_one_ahead", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory });
    await run(t.engine.warm("test"));
    await run(t.engine.warm("test"));
    expect(warmList(t)).toMatchObject([{ state: "spawning", served: 0 }]);
    const w = await readyWorker(t);
    const spawnArgs = rec.spawns.map((s) => s.req.args);
    expect(spawnArgs).toEqual([["test", "--warm"]]);
    const v = await t.runJob("test.warm", { value: 1 });
    expect(resultOf(v)).toMatchObject({ pid: w.pid, served: 1 });
    expect(rec.spawns).toHaveLength(1);
    expect((await failure(t.engine.warm("nope"))).code).toBe("JOB_KIND_UNKNOWN");
    expect(await failure(t.engine.warm("testbad"))).toMatchObject({ code: "WORKER_UNAVAILABLE", details: { reason: "notWarm" } });
  });

  test("slots_default_one_max_two", async () => {
    const one = await testEngine();
    const [a, b] = await Promise.all([one.runJob("test.warm", { value: 1, delayMs: 300 }), one.runJob("test.warm", { value: 2, delayMs: 300 })]);
    expect(resultOf(a as JobView).pid).toBe(resultOf(b as JobView).pid);
    const two = await testEngine({ testKinds: { families: { test: { warm: { slots: 2 } } } } });
    const [c, d] = await Promise.all([two.runJob("test.warm", { value: 1, delayMs: 300 }), two.runJob("test.warm", { value: 2, delayMs: 300 })]);
    expect(resultOf(c as JobView).pid).not.toBe(resultOf(d as JobView).pid);
    expect(warmList(two)).toHaveLength(2);
    const registry = makeKindRegistry();
    expect(() => registry.registerFamily({ family: "x", profile: "worker", program: { binary: "fin-workers", args: ["x"] }, warm: { slots: 3 } })).toThrow(/1 or 2/);
  });

  test("cold_jobs_never_delay_interactive", async () => {
    const t = await testEngine({ cores: 4 });
    const sleeps = await Promise.all([1, 2, 3, 4].map(() => t.submit("test.sleep", { ms: 60_000 })));
    await until(() => t.engine.workers().pools.find((p) => p.name === "cpu")?.used === 3);
    // Every cpu slot runs a cold job, and one more waits: a warm job still runs at once.
    const t0 = Date.now();
    const v = await t.runJob("test.warm", { value: 1 });
    expect(v.state).toBe("succeeded");
    expect(Date.now() - t0).toBeLessThan(10_000);
    expect(t.jobs.get(sleeps[3]?.id as string)?.state).toBe("queued");
    expect(resultOf(await t.runJob("test.warm", { value: 2 })).served).toBe(2);
    for (const s of sleeps) await run(t.engine.cancel(s.id));
  });

  test("warm_kind_at_normal_priority_runs_cold", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory });
    const v = await t.runJob("test.warm", { value: 1 }, { priority: "normal" });
    expect(v).toMatchObject({ state: "succeeded", warm: false });
    expect(rec.spawns.map((s) => s.req.args)).toEqual([["test"]]);
    expect(warmList(t)).toEqual([]);
    await t.runJob("test.warm", { value: 2 });
    expect(rec.spawns.map((s) => s.req.args)).toEqual([["test"], ["test", "--warm"]]);
  });

  test("memory_limit_is_the_interactive_rule", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({
      jobHost: factory,
      testKinds: {
        families: { test: { memory: { interactive: { fixedMiB: 300 }, batch: { fixedMiB: 100 } } } },
        overrides: { "test.helper": { warm: true, defaultPriority: "interactive", memory: { fixedMiB: 50 } } },
      },
    });
    const v = await t.runJob("test.helper", {});
    expect(v).toMatchObject({ state: "succeeded", warm: true });
    const worker = rec.spawns.find((s) => s.req.role === "worker");
    const helper = rec.spawns.find((s) => s.req.role === "helper");
    const limit = (group: number | undefined) => rec.groups.find((g) => g.group === group)?.spec.memoryBytes;
    // The worker's group takes the family's interactive rule; the job's helper group the kind's.
    expect(limit(worker?.group)).toBe(300 * MiB);
    expect(limit(helper?.group)).toBe(50 * MiB);
    const start = rec.sent.find((s) => s.msg.t === "start")?.msg;
    expect(start?.t === "start" ? start.limits.memoryBytes : undefined).toBe(300 * MiB);
  });

  test("close_shuts_down_warm_workers", async () => {
    const t = await testEngine();
    await t.runJob("test.warm", { value: 1 });
    await readyWorker(t);
    await t.close();
    expect(warmExits(t)).toEqual([expect.objectContaining({ reason: "done", exitCode: 0, jobsServed: 1 })]);
  });
});

describe("lifecycle", () => {
  test("idle_expiry_60s", async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    await t.runJob("test.warm", { value: 1 });
    await readyWorker(t);
    await clock.adjust(59_000);
    expect(warmList(t)).toMatchObject([{ state: "ready" }]);
    await clock.adjust(1_000);
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "idleExpired", jobsServed: 1, exitCode: 0 });
    expect(warmList(t)).toEqual([]);
    await t.close();
    await clock.close();
  });

  test("recycled_after_500_jobs", { timeout: 60_000 }, async () => {
    const t = await testEngine();
    let pid = 0;
    for (let i = 0; i < 500; i++) pid = resultOf(await t.runJob("test.warm", { value: i })).pid;
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "recycled", jobsServed: 500 });
    const next = resultOf(await t.runJob("test.warm", { value: 500 }));
    expect(next.pid).not.toBe(pid);
    expect(next.served).toBe(1);
  });

  test("recycled_after_30_minutes", { timeout: 60_000 }, async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    const first = resultOf(await t.runJob("test.warm", { value: 0 }));
    // A job every 59 s keeps the worker from expiring; past 30 minutes its next `ready` recycles it.
    for (let i = 1; i <= 31; i++) {
      await clock.adjust(59_000);
      await t.runJob("test.warm", { value: i });
    }
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "recycled", jobsServed: 32 });
    expect(resultOf(await t.runJob("test.warm", { value: 99 })).pid).not.toBe(first.pid);
    await t.close();
    await clock.close();
  });

  test("recycled_at_80_percent_memory", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory, testKinds: { families: { test: { memory: { interactive: { fixedMiB: 1000 } } } } } });
    rec.readyMemory = 790 * MiB;
    const a = resultOf(await t.runJob("test.warm", { value: 1 }));
    expect(await readyWorker(t)).toMatchObject({ pid: a.pid, memory: 790 * MiB });
    expect(rec.spawns).toHaveLength(1);
    rec.readyMemory = 850 * MiB; // a `ready` reporting 85 % of the limit
    await t.runJob("test.warm", { value: 2 });
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "recycled", jobsServed: 2 });
    rec.readyMemory = undefined;
    expect(resultOf(await t.runJob("test.warm", { value: 3 }))).toMatchObject({ served: 1 });
  });

  test("replacement_starts_before_retirement", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory, testKinds: { families: { test: { warm: { maxJobs: 2 } } } } });
    const old = resultOf(await t.runJob("test.warm", { value: 1 })).pid;
    rec.holdNext(); // the replacement's `hello` waits
    await t.runJob("test.warm", { value: 2 });
    await until(() => warmList(t).length === 2);
    expect(warmList(t).map((w) => w.state).sort()).toEqual(["ready", "spawning"]);
    // The old worker keeps serving: no job waits for the cold start.
    const t0 = Date.now();
    const third = resultOf(await t.runJob("test.warm", { value: 3 }));
    expect(third).toMatchObject({ pid: old, served: 3 });
    expect(Date.now() - t0).toBeLessThan(5000);
    expect(warmExits(t)).toEqual([]);
    rec.release();
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "recycled", jobsServed: 3 });
    expect(resultOf(await t.runJob("test.warm", { value: 4 }))).toMatchObject({ served: 1 });
    expect(resultOf(await t.runJob("test.warm", { value: 5 })).pid).not.toBe(old);
  });
});

describe("failures", () => {
  test("crash_mid_job_retries_on_fresh_worker", async () => {
    const t = await testEngine();
    const first = resultOf(await t.runJob("test.warm", { value: 0 }));
    const v = await t.submit("test.warm", { value: 1, delayMs: 1500 });
    await until(() => warmList(t)[0]?.state === "busy");
    hostOf(t.engine).killProcForTest?.(await procOfPid(t, first.pid));
    const done = await t.wait(v.id);
    expect(done).toMatchObject({ state: "succeeded", attempts: 2 });
    expect(resultOf(done).pid).not.toBe(first.pid);
    expect(resultOf(done).served).toBe(1);
    expect(["crashed", "killed"]).toContain(warmExits(t)[0]?.reason);
    expect(warmExits(t)[0]).toMatchObject({ jobsServed: 2 });
  });

  test("oom_on_reused_warm_worker_retries_fresh", async () => {
    const served = new Map<number, number>();
    const fake = new FakeHost((host, proc) =>
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.warm", version: 1 }], warm: true, pid: 1000 + proc } }),
    );
    fake.onSend = (host, proc, msg) => {
      if (msg.t !== "start") return;
      const n = (served.get(proc) ?? 0) + 1;
      served.set(proc, n);
      if (proc === 1 && n === 2) return host.exit(proc, { reason: "oom", memoryPeak: 999 * MiB });
      host.emit({ type: "message", proc, msg: { t: "result", job: msg.job.id, value: { value: null, pid: 1000 + proc, served: n } } });
      host.emit({ type: "message", proc, msg: { t: "ready" } });
    };
    const t = await testEngine({ jobHost: fake.factory });
    expect(await t.runJob("test.warm", { value: 1 })).toMatchObject({ state: "succeeded" });
    const v = await t.runJob("test.warm", { value: 2 }, { maxAttempts: 5 });
    // Once, at once, on a fresh worker; never isolated.
    expect(v).toMatchObject({ state: "succeeded", attempts: 2 });
    expect(resultOf(v)).toMatchObject({ pid: 1002, served: 1 });
    expect(fake.spawns).toHaveLength(2);
    expect(t.sink.done.at(-1)).toMatchObject({ isolated: false, attempts: 2 });
    expect(t.sink.exits.find((x) => x.reason === "oom")).toMatchObject({ jobsServed: 2 });
    expect(t.jobs.row(v.id)?.isolation).toEqual({ fresh: true });
  });

  test("oom_on_a_first_job_fails", async () => {
    const fake = new FakeHost((host, proc) =>
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.warm", version: 1 }], warm: true, pid: 1000 + proc } }),
    );
    fake.onSend = (host, proc, msg) => {
      if (msg.t === "start") host.exit(proc, { reason: "oom" });
    };
    const t = await testEngine({ jobHost: fake.factory });
    expect(await t.runJob("test.warm", { value: 1 })).toMatchObject({ state: "failed", attempts: 1, error: { code: "WORKER_OOM" } });
  });

  test("a_warm_worker_that_never_comes_up_fails_its_job", async () => {
    const fake = new FakeHost((host, proc) => host.exit(proc, { reason: "noHello" }));
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.warm", { value: 1 });
    expect(v).toMatchObject({ state: "failed", attempts: 2, error: { code: "WORKER_UNAVAILABLE", details: { reason: "noHello" } } });
    expect(fake.spawns).toHaveLength(2);
    expect(t.sink.exits.map((x) => x.reason)).toEqual(["spawnFailed", "spawnFailed"]);
  });

  test("replacement_on_next_submit", async () => {
    const t = await testEngine();
    const first = resultOf(await t.runJob("test.warm", { value: 0 }));
    hostOf(t.engine).killProcForTest?.(await procOfPid(t, first.pid));
    await until(() => warmExits(t).length === 1);
    // The exit's handling (its `worker.exit`, then a `pump`) is one synchronous step, so a
    // respawn would already be listed. Nothing respawns on its own: the next interactive submit does.
    expect(warmList(t)).toEqual([]);
    expect(t.engine.workers().processes.filter((p) => p.role === "worker")).toEqual([]);
    const next = resultOf(await t.runJob("test.warm", { value: 1 }));
    expect(next.pid).not.toBe(first.pid);
    expect(next.served).toBe(1);
  });

  test("cancel_keeps_a_cooperative_worker", async () => {
    const t = await testEngine();
    const v = await t.submit("test.warm", { value: 1, delayMs: 30_000 });
    const w = await until(() => warmList(t).find((x) => x.state === "busy"));
    const cancelled = await run(t.engine.cancel(v.id));
    expect(cancelled).toMatchObject({ state: "cancelled", error: { code: "JOB_CANCELLED" } });
    expect(await readyWorker(t)).toMatchObject({ pid: w.pid });
    expect(resultOf(await t.runJob("test.warm", { value: 2 }))).toMatchObject({ pid: w.pid, served: 2 });
    expect(warmExits(t)).toEqual([]);
  });

  test("cancel_kills_an_uncooperative_worker", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.hang": { warm: true, defaultPriority: "interactive" } } } });
    const v = await t.submit("test.hang", {});
    await until(() => warmList(t).find((x) => x.state === "busy"));
    const t0 = Date.now();
    const cancelled = await run(t.engine.cancel(v.id));
    const took = Date.now() - t0;
    expect(cancelled).toMatchObject({ state: "cancelled" });
    // The interactive grace is 500 ms; then the worker's group is killed.
    expect(took).toBeGreaterThanOrEqual(450);
    expect(took).toBeLessThan(5000);
    await until(() => warmExits(t).length === 1);
    expect(warmExits(t)[0]).toMatchObject({ reason: "cancelled" });
    expect(warmList(t)).toEqual([]);
  });

  test.skipIf(process.platform === "win32")("files_closed_before_ready", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.warm": { files: [{ name: "held.bin", bytes: 64, mode: "read" }] } } } });
    const ok = await t.runJob("test.warm", { value: 1 });
    expect(ok.state).toBe("succeeded");
    const pid = resultOf(ok).pid;
    const v = await t.runJob("test.warm", { value: 2, holdFile: "held.bin" });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "WORKER_CRASHED" } });
    expect((v.error?.details as { heldFiles: string[] }).heldFiles).toEqual([expect.stringMatching(/held\.bin$/)]);
    // The worker broke the contract: it is gone, and the next job gets a new one.
    await until(() => warmExits(t).length === 1);
    expect(resultOf(await t.runJob("test.warm", { value: 3 })).pid).not.toBe(pid);
  });

  test.skipIf(process.platform !== "win32")("files_closed_before_ready (windows: the directory delete succeeds)", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.warm": { files: [{ name: "held.bin", bytes: 64, mode: "read" }] } } } });
    expect(await t.runJob("test.warm", { value: 1 })).toMatchObject({ state: "succeeded" });
    expect(await t.runJob("test.warm", { value: 2 })).toMatchObject({ state: "succeeded" });
  });
});

describe("helpers and keys", () => {
  test("helpers_get_a_per_job_group_on_warm_workers", async () => {
    const { factory, rec } = proxied();
    const t = await testEngine({ jobHost: factory, testKinds: { overrides: { "test.helper": { warm: true, defaultPriority: "interactive" } } } });
    const v = await t.runJob("test.helper", {});
    expect(v).toMatchObject({ state: "succeeded", warm: true, result: { helpers: ["test"] } });
    const worker = rec.spawns.find((s) => s.req.role === "worker");
    const helper = rec.spawns.find((s) => s.req.role === "helper");
    expect(helper?.group).not.toBe(worker?.group);
    // The helper's group died with the job; the worker lives on.
    expect(t.sink.exits.filter((x) => x.role === "helper")).toHaveLength(1);
    expect(t.engine.workers().processes.filter((p) => p.role === "helper")).toEqual([]);
    const w = await readyWorker(t);
    const again = await t.runJob("test.helper", {});
    expect(again.state).toBe("succeeded");
    expect(warmList(t)).toMatchObject([{ pid: w.pid, served: 2 }]);
    expect(warmExits(t)).toEqual([]);
  });

  test("warm_key_separates_workers", async () => {
    const t = await testEngine({ testKinds: { families: { testplugin: { warm: { slots: 2 } } } } });
    const seen: Array<{ key: string; pid: number; served: number }> = [];
    for (const key of ["A", "B", "A", "B", "A"]) {
      const v = await t.runJob("test.pluginMinor", { key }, { priority: "interactive" });
      expect(v).toMatchObject({ state: "succeeded", warm: true });
      const r = v.result as { pid: number; served: number; key: string };
      expect(r.key).toBe(key);
      seen.push({ key, pid: r.pid, served: r.served });
    }
    const pidOf = (k: string) => new Set(seen.filter((s) => s.key === k).map((s) => s.pid));
    expect(pidOf("A").size).toBe(1);
    expect(pidOf("B").size).toBe(1);
    expect([...pidOf("A")][0]).not.toBe([...pidOf("B")][0]);
    expect(seen.map((s) => s.served)).toEqual([1, 1, 2, 2, 3]);
    expect(warmList(t).map((w) => w.key).sort()).toEqual(["A", "B"]);
  });

  test("full_pool_retires_longest_idle_key", async () => {
    const t = await testEngine({ testKinds: { families: { testplugin: { warm: { slots: 2 } } } } });
    const job = async (key: string) => (await t.runJob("test.pluginMinor", { key }, { priority: "interactive" })).result as { pid: number; served: number };
    await job("A");
    const b = await job("B");
    await job("C"); // the pool is full: A's worker, idle the longest, makes room
    await until(() => t.sink.exits.some((x) => x.family === "testplugin"));
    expect(t.sink.exits.find((x) => x.family === "testplugin")).toMatchObject({ reason: "recycled", jobsServed: 1 });
    expect(warmList(t).map((w) => w.key).sort()).toEqual(["B", "C"]);
    expect(await job("B")).toEqual({ ...b, served: 2, reply: expect.anything() });
  });

  test("fresh_and_reused_results_equal", async () => {
    const t = await testEngine();
    const params = { value: { a: [1, 2, 3], b: "x" } };
    const fresh = resultOf(await t.runJob("test.warm", params));
    const reused = resultOf(await t.runJob("test.warm", params));
    expect(fresh.served).toBe(1);
    expect(reused.served).toBe(2);
    const strip = ({ pid: _p, served: _s, ...rest }: TestWarmResult) => rest;
    expect(strip(reused)).toEqual(strip(fresh));
  });
});

describe("schedule with fresh retries", () => {
  const pools = () => makePools({ cpu: 4 });
  const job = (id: string, fresh: boolean): QueuedJob => ({ id, priority: "interactive", createdAt: 1, resources: ["cpu"], warm: { family: "f", key: "k", ...(fresh ? { fresh } : {}) } });
  const queue = (...jobs: QueuedJob[]) => {
    const q = new QueueIndex();
    for (const j of jobs) q.add(j);
    return q;
  };

  test("a_fresh_retry_takes_only_an_unused_worker", () => {
    const warm = (served: number, slots = 1) => new Map([["f", { slots, workers: [{ key: "k", state: "ready" as const, served, idleSince: 0 }] }]]);
    expect(schedule({ queue: queue(job("j", true)), pools: pools(), warm: warm(0) }, 2)).toEqual([{ job: "j", mode: "warm", family: "f", key: "k" }]);
    // A worker that served jobs is retired for a fresh one when the pool is full…
    expect(schedule({ queue: queue(job("j", true)), pools: pools(), warm: warm(3) }, 2)).toEqual([{ mode: "spawnWarm", family: "f", key: "k", retire: 0 }]);
    // …and kept beside it when there is room; a job that is not a retry takes it.
    expect(schedule({ queue: queue(job("j", true)), pools: pools(), warm: warm(3, 2) }, 2)).toEqual([{ mode: "spawnWarm", family: "f", key: "k" }]);
    expect(schedule({ queue: queue(job("j", false)), pools: pools(), warm: warm(3) }, 2)).toEqual([{ job: "j", mode: "warm", family: "f", key: "k" }]);
  });

  test("warm_head_is_the_oldest_due_job_of_its_key", () => {
    const q = queue({ ...job("b", false), createdAt: 5 }, { ...job("a", false), createdAt: 3 }, { ...job("c", false), createdAt: 1, nextRunAt: 100 });
    expect(q.warmHead("f", "k", 2)?.id).toBe("a");
    expect(q.warmHead("f", "k", 200)?.id).toBe("c");
    expect(q.warmHead("f", "other", 200)).toBeUndefined();
  });
});
