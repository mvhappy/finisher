// Retries, backoff, poison handling and isolation with real worker processes (5C-1 plan Task 10,
// Decisions 31, 32). `recordingHost` keeps every `start`, so each run's items are known: the
// worker begins exactly the items its params name.
import { rmSync } from "node:fs";
import type { HostMsg, JobSpawnRequest } from "@finisher/core-types";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { FakeHost, recordingHost } from "../support/fake-host.ts";
import { closeTestEngines, run, testClock, testEngine, until, waitThroughBackoff } from "../support/jobs.ts";

afterEach(closeTestEngines);

const win = process.platform === "win32";
const letters = (s: string): string[] => s.split("");

/** An engine whose `start` messages are kept: `runs()` lists each run's items. */
async function recorded(opts: Parameters<typeof testEngine>[0] = {}) {
  const reqs: JobSpawnRequest[] = [];
  const sent: Array<{ proc: number; msg: HostMsg }> = [];
  const t = await testEngine({ jobHost: recordingHost(reqs, sent), ...opts });
  const runs = (): string[][] => sent.flatMap((s) => (s.msg.t === "start" ? [(s.msg.job.params as { items: string[] }).items] : []));
  return { t, runs, reqs };
}

/** A batch override for a kind that is not one (its params are passed through). */
const asBatch = { batch: { items: () => ["a", "b"], restrict: (p: unknown) => p } };

describe("isolation", () => {
  test("poison_item_isolated_in_two_runs", async () => {
    const { t, runs } = await recorded();
    const v = await t.runJob("test.crash", { items: letters("abcd"), poison: ["b"], how: "abort" });
    expect(v).toMatchObject({ state: "succeeded", attempts: 1 });
    expect(runs()).toEqual([letters("abcd"), ["b"], ["c", "d"]]);
    expect(v.itemErrors).toEqual([{ item: "b", error: expect.objectContaining({ code: "POISON_ITEM", details: expect.objectContaining({ item: "b", cause: "WORKER_CRASHED" }) }) }]);
    expect(v.result).toEqual({
      items: { a: { item: "a" }, c: { item: "c" }, d: { item: "d" } },
      itemErrors: [{ item: "b", error: expect.objectContaining({ code: "POISON_ITEM" }) }],
    });
    expect(t.sink.done[0]).toMatchObject({ outcome: "succeeded", isolated: true, items: 4, itemsFailed: 1, attempts: 1 });
  });

  test("isolation_runs_without_backoff", async () => {
    const clock = await testClock();
    const { t, runs } = await recorded({ clock: clock.clock });
    // The clock never moves: every isolation run starts at once.
    const v = await t.runJob("test.crash", { items: letters("abcdef"), poison: ["b", "e"], how: "segv" });
    expect(v.state).toBe("succeeded");
    expect(runs()).toEqual([letters("abcdef"), ["b"], letters("cdef"), ["e"], ["f"]]);
    expect(v.itemErrors?.map((e) => e.item)).toEqual(["b", "e"]);
    await t.close();
    await clock.close();
  });

  test("crash_between_items_bisects", async () => {
    const { t, runs } = await recorded();
    const v = await t.runJob("test.crash", { items: letters("abcdefgh"), poison: ["f"], how: "abort", between: true });
    expect(v.state).toBe("succeeded");
    // Nothing in flight: the remainder f, g, h is halved, then f, g again, then f alone poisons it.
    expect(runs()).toEqual([letters("abcdefgh"), ["f", "g"], ["f"], ["g"], ["h"]]);
    expect(v.itemErrors?.map((e) => e.item)).toEqual(["f"]);
  });

  test("reported_items_are_not_rerun", async () => {
    const { t, runs } = await recorded();
    const items = Array.from({ length: 10 }, (_, i) => `i${i}`);
    const v = await t.runJob("test.crash", { items, poison: ["i3", "i7"], how: "exit" });
    expect(v.state).toBe("succeeded");
    const all = runs();
    // An item a run acknowledged is never in a later run (the worker begins exactly its params' items).
    const poison = new Set(["i3", "i7"]);
    const acked = new Set<string>();
    for (const r of all) {
      for (const i of r) expect(acked.has(i)).toBe(false);
      const stop = r.findIndex((i) => poison.has(i)); // the worker dies inside a poison item
      for (const i of stop === -1 ? r : r.slice(0, stop)) acked.add(i);
    }
    expect(all).toEqual([items, ["i3"], items.slice(4), ["i7"], items.slice(8)]);
  });

  test("isolation_budget_exhausted_by_unexplained_crashes", async () => {
    // Every run of two or more dies in its second item, which alone succeeds. 32 items: the
    // budget is 2⌈log₂ 32⌉ = 10 crashing runs, and 16 would be needed (8 items fit in 6).
    const { t } = await recorded();
    const items = Array.from({ length: 32 }, (_, i) => `i${i}`);
    const v = await t.runJob("test.crash", { items, how: "exit", crashOnSecond: true });
    expect(v).toMatchObject({ state: "failed", error: { code: "WORKER_CRASHED", details: { isolation: "budget", crashRuns: 11, poisoned: 0 } } });
    expect(v.itemErrors).toBeUndefined();
    expect(t.sink.done[0]).toMatchObject({ isolated: true, itemsFailed: 0 });
  }, 60_000);

  test("eight_unexplained_items_fit_the_budget", async () => {
    const { t } = await recorded();
    const v = await t.runJob("test.crash", { items: letters("abcdefgh"), how: "exit", crashOnSecond: true });
    expect(v.state).toBe("succeeded");
    expect(Object.keys((v.result as { items: object }).items)).toHaveLength(8);
  }, 30_000);

  test.skipIf(win)("oom_with_item_in_flight_isolates", async () => {
    const t = await testEngine({ timing: { rssSampleMs: 50 }, testKinds: { overrides: { "test.crash": { memory: { fixedMiB: 64 } } } } });
    const v = await t.runJob("test.crash", { items: letters("abc"), poison: ["b"], how: "oom" });
    expect(v.state).toBe("succeeded");
    expect(v.itemErrors).toEqual([{ item: "b", error: expect.objectContaining({ code: "POISON_ITEM", details: expect.objectContaining({ cause: "WORKER_OOM" }) }) }]);
  }, 30_000);

  test("oom_without_item_fails", async () => {
    const t = await testEngine({ timing: { rssSampleMs: 50 }, testKinds: { overrides: { "test.oom": { memory: { fixedMiB: 64 } } } } });
    const v = await t.runJob("test.oom", { mib: 256, holdMs: 5000 });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "WORKER_OOM" } });
  });

  test("interactive_retries_once_at_once_and_is_never_isolated", async () => {
    const clock = await testClock();
    const { t, runs } = await recorded({ clock: clock.clock });
    const v = await t.runJob("test.crash", { items: ["a", "b"], poison: ["a"], how: "abort" }, { priority: "interactive" });
    expect(v).toMatchObject({ state: "failed", attempts: 2, maxAttempts: 2, error: { code: "WORKER_CRASHED" } });
    expect(runs()).toEqual([["a", "b"], ["a", "b"]]);
    expect(v.itemErrors).toBeUndefined();
    expect(t.sink.done[0]).toMatchObject({ isolated: false });
    await t.close();
    await clock.close();
  });
});

describe("never bisected", () => {
  test("sigbus_is_a_resource_fault", async () => {
    const clock = await testClock();
    const { t, runs } = await recorded({ clock: clock.clock });
    const s = await t.submit("test.crash", { items: ["a", "b"], poison: ["a"], how: "bus" }, { maxAttempts: 2 });
    const delays: number[] = [];
    const v = await waitThroughBackoff(t, clock, s.id, delays);
    expect(delays).toEqual([10_000]); // a backoff retry, not an isolation run
    expect(v).toMatchObject({ state: "failed", attempts: 2, error: { code: "WORKER_CRASHED", details: { resourceFault: true } } });
    expect(runs()).toEqual([["a", "b"], ["a", "b"]]);
    expect(v.itemErrors).toBeUndefined();
    expect(t.sink.done[0]).toMatchObject({ isolated: false });
    await t.close();
    await clock.close();
  });

  test("resource_unhealthy_never_poisons", async () => {
    let asked = 0;
    const resourceHealthy = () => Effect.sync(() => (asked++, false));
    const { t, runs } = await recorded({ testKinds: { overrides: { "test.crash": { resourceHealthy } } } });
    const v = await t.runJob("test.crash", { items: ["a", "b"], poison: ["b"], how: "abort" }, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "WORKER_CRASHED", details: { resourceHealthy: false } } });
    expect(asked).toBe(1);
    expect(runs()).toEqual([["a", "b"]]);
    expect(v.itemErrors).toBeUndefined();
    expect(t.jobs.items(v.id).has("a")).toBe(true); // what it acknowledged stays
  });

  test("hang_is_never_bisected", async () => {
    const t = await testEngine({ timing: { hungMs: 1000 }, testKinds: { overrides: { "test.hang": asBatch } } });
    const v = await t.runJob("test.hang", {}, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "WORKER_HUNG" } });
    expect(t.sink.exits).toHaveLength(1);
    expect(t.sink.done[0]).toMatchObject({ isolated: false });
  });

  test("stall_is_job_stalled_never_bisected", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.stall": { stallS: 0.3, ...asBatch } } } });
    const v = await t.runJob("test.stall", {}, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "JOB_STALLED" } });
    expect(t.sink.exits).toHaveLength(1);
    expect(t.sink.done[0]).toMatchObject({ isolated: false });
  });

  test("helper_failure_is_never_bisected", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.helper": asBatch } } });
    const v = await t.runJob("test.helper", { helper: "exitEarly", delayMs: 10_000 }, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "HELPER_FAILED" } });
    expect(t.sink.exits.filter((e) => e.role === "helper")).toHaveLength(1);
    expect(t.sink.done[0]).toMatchObject({ isolated: false });
  });
});

describe("retries", () => {
  test("retry_backoff_persists_next_run_at", async () => {
    const first = await testClock();
    const a = await testEngine({ clock: first.clock, keepData: true });
    const s = await a.submit("test.fail", { code: "IO_ERROR", retry: "backoff" });
    await until(() => a.jobs.get(s.id)?.nextRunAt);
    const nextRunAt = a.jobs.get(s.id)?.nextRunAt as number;
    expect(nextRunAt - first.now()).toBe(10_000);
    await a.close();
    await first.close();
    // A restart keeps `nextRunAt`: nothing runs before it.
    const second = await testClock();
    const b = await testEngine({ clock: second.clock, serverData: a.serverData });
    expect(b.jobs.get(s.id)).toMatchObject({ state: "queued", attempts: 1, nextRunAt });
    await new Promise((r) => setTimeout(r, 100));
    expect(b.jobs.get(s.id)?.state).toBe("queued");
    const delays: number[] = [];
    const left = nextRunAt - second.now();
    const v = await waitThroughBackoff(b, second, s.id, delays);
    expect(delays).toEqual([left, 20_000]);
    expect(v).toMatchObject({ state: "failed", attempts: 3, error: { code: "IO_ERROR" } });
    await b.close();
    await second.close();
    rmSync(a.serverData, { recursive: true, force: true });
  });

  test("max_attempts_then_failed", async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    const s = await t.submit("test.fail", { code: "IO_ERROR", retry: "backoff", failUntil: 9 }, { maxAttempts: 4 });
    const delays: number[] = [];
    const v = await waitThroughBackoff(t, clock, s.id, delays);
    expect(delays).toEqual([10_000, 20_000, 40_000]);
    expect(v).toMatchObject({ state: "failed", attempts: 4, error: { code: "IO_ERROR" } });
    const ok = await waitThroughBackoff(t, clock, (await t.submit("test.fail", { code: "IO_ERROR", retry: "backoff", failUntil: 1 })).id);
    expect(ok).toMatchObject({ state: "succeeded", attempts: 2, result: { attempt: 2 } });
    expect(ok.error).toBeUndefined();
    await t.close();
    await clock.close();
  });

  test("fresh_retry_once_at_once", async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    // The clock never moves: the retry is at once, and only once.
    const v = await t.runJob("test.fail", { code: "GPU_DEVICE_LOST", retry: "fresh" }, { maxAttempts: 3 });
    expect(v).toMatchObject({ state: "failed", attempts: 2, error: { code: "GPU_DEVICE_LOST" } });
    expect(t.sink.exits).toHaveLength(2);
    await t.close();
    await clock.close();
  });

  test("deterministic_error_not_retried", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.fail", { code: "BAD_INPUT" }, { maxAttempts: 3 });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "BAD_INPUT" } });
  });

  test("mismatch_not_retried", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.badHello", {}, { maxAttempts: 3 });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "WORKER_PROTOCOL_MISMATCH" } });
  });

  test("a_pause_during_a_retried_run_keeps_the_job_paused", async () => {
    // The worker answers the pause's cancel with a retryable error: the retry is queued, and paused.
    const fake = new FakeHost((host, proc) =>
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.checkpoint", version: 1 }], warm: false, pid: 1 } }),
    );
    fake.onSend = (host, proc, msg) => {
      if (msg.t !== "cancel") return;
      host.emit({ type: "message", proc, msg: { t: "error", job: msg.job, error: { code: "IO_ERROR", message: "disk", retry: "backoff" } } });
      host.exit(proc, { reason: "exited", code: 0 });
    };
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.submit("test.checkpoint", { steps: 10 });
    await until(() => fake.sent.some((s) => s.msg.t === "start"));
    await run(t.engine.pause({ job: v.id }));
    const paused = t.jobs.get(v.id);
    expect(paused).toMatchObject({ state: "paused", attempts: 1, error: { code: "IO_ERROR" } });
    expect(paused?.nextRunAt).toBeGreaterThan(0);
    await run(t.engine.resume({ job: v.id }));
    expect(t.jobs.get(v.id)).toMatchObject({ state: "queued", nextRunAt: paused?.nextRunAt });
  });

  test("prepare_reruns_after_an_interrupted_prepare", async () => {
    // 9b review M3's path: a pause interrupts `prepare`, which left no mark; the resumed run prepares again.
    let calls = 0;
    const prepare = () => (++calls === 1 ? Effect.never : Effect.succeed({ extra: { call: calls } }));
    const t = await testEngine({ testKinds: { overrides: { "test.checkpoint": { prepare } } } });
    const v = await t.submit("test.checkpoint", { steps: 2 });
    await until(() => calls === 1);
    await run(t.engine.pause({ job: v.id }));
    expect(t.jobs.get(v.id)).toMatchObject({ state: "paused", attempts: 0 });
    await run(t.engine.resume({ job: v.id }));
    expect(await t.wait(v.id)).toMatchObject({ state: "succeeded", attempts: 1 });
    expect(calls).toBe(2);
  });
});
