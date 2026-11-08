// Cancel, pause, resume and abandonment (5C-1 plan Task 9b, Decisions 18, 23, 37).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import { closeTestEngines, failure, hostOf, OWNER, procOf, run, type TestEngine, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

function gone(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return false;
  } catch (e) {
    return (e as { code?: string }).code === "ESRCH";
  }
}

describe("cancel", () => {
  test("cancel_queued", async () => {
    const t = await testEngine({ cores: 2 });
    const busy = await t.submit("test.sleep", { ms: 60_000 });
    const queued = await t.submit("test.sleep", { ms: 1 });
    expect(queued.state).toBe("queued");
    const c = await run(t.engine.cancel(queued.id));
    expect(c).toMatchObject({ state: "cancelled", error: { code: "JOB_CANCELLED" }, attempts: 0 });
    expect(t.sink.done).toEqual([expect.objectContaining({ outcome: "cancelled", code: "JOB_CANCELLED" })]);
    expect((await run(t.engine.cancel(busy.id))).state).toBe("cancelled");
    expect((await run(t.engine.cancel(busy.id))).state).toBe("cancelled"); // a finished job: unchanged
    expect((await failure(t.engine.cancel("nope"))).code).toBe("JOB_NOT_FOUND");
  });

  test.skipIf(process.platform === "win32")("cancel_running_kills_tree", async () => {
    const t = await testEngine();
    const v = await t.submit("test.spawn", { detach: "none", out: "pids.json" });
    const file = join(t.serverData, "jobs", v.id, "pids.json");
    const pids = await until(() => {
      try {
        return JSON.parse(readFileSync(file, "utf8").replace(/\0+$/, "")) as { worker: number; mid: number; grandchild: number };
      } catch {
        return undefined;
      }
    });
    const c = await run(t.engine.cancel(v.id));
    expect(c).toMatchObject({ state: "cancelled", error: { code: "JOB_CANCELLED" } });
    expect(gone(pids.worker)).toBe(true);
    await until(() => gone(pids.grandchild), 5000);
    // The worker answered the cancel itself (`done`) or was killed after the grace (`cancelled`).
    expect(["done", "cancelled"]).toContain(t.sink.exits[0]?.reason);
  });

  test("a_worker_that_answers_the_cancel_ends_before_the_grace", async () => {
    const t = await testEngine();
    const v = await t.submit("test.sleep", { ms: 60_000 });
    await procOf(t, v.id);
    const t0 = Date.now();
    expect((await run(t.engine.cancel(v.id))).state).toBe("cancelled");
    expect(Date.now() - t0).toBeLessThan(1500);
  });
});

describe("pause and resume", () => {
  test("pause_queued_keeps_place", async () => {
    const t = await testEngine({ cores: 2 });
    const busy = await t.submit("test.sleep", { ms: 60_000 });
    const first = await t.submit("test.echo", { value: 1 });
    await new Promise((r) => setTimeout(r, 5)); // a later createdAt (equal ones go by id)
    const second = await t.submit("test.echo", { value: 2 });
    await run(t.engine.pause({ job: first.id }));
    expect(t.jobs.row(first.id)).toMatchObject({ state: "paused", pausedFrom: "queued" });
    await run(t.engine.resume({ job: first.id }));
    const resumed = t.jobs.get(first.id)!;
    expect(resumed).toMatchObject({ state: "queued", createdAt: first.createdAt });
    await run(t.engine.cancel(busy.id));
    const [a, b] = await run(t.engine.wait([first.id, second.id], { timeoutMs: 20_000 }));
    // It kept its place: it ran before the job submitted after it.
    expect(a!.startedAt!).toBeLessThanOrEqual(b!.startedAt!);
  });

  test("pause_running_resumable_checkpoints", async () => {
    const t = await testEngine();
    const v = await t.submit("test.checkpoint", { steps: 400, stepMs: 20 });
    await until(() => (t.engine.list({}).jobs[0]?.progress?.done ?? 0) >= 3);
    await run(t.engine.pause({ job: v.id }));
    const row = t.jobs.row(v.id)!;
    expect(row).toMatchObject({ state: "paused", pausedFrom: "running", attempts: 0 });
    const step = (row.checkpoint as { step: number }).step;
    expect(step).toBeGreaterThanOrEqual(3);
    await run(t.engine.resume({ job: v.id }));
    const done = await t.wait(v.id, 30_000);
    expect(done).toMatchObject({ state: "succeeded", attempts: 1 });
    expect((done.result as { resumedFrom: number }).resumedFrom).toBe(step);
  }, 30_000);

  test("pause_running_non_resumable_finishes", async () => {
    const t = await testEngine();
    const v = await t.submit("test.sleep", { ms: 300 });
    await run(t.engine.pause({ job: v.id }));
    expect((await t.wait(v.id)).state).toBe("succeeded");
  });

  test("pause_all_stops_dispatch", async () => {
    const t = await testEngine({ cores: 3 });
    const resumable = await t.submit("test.checkpoint", { steps: 400, stepMs: 20 });
    const plain = await t.submit("test.sleep", { ms: 300 });
    expect([resumable.state, plain.state]).toEqual(["running", "running"]);
    await run(t.engine.pause({ all: true }));
    expect(t.jobs.get(resumable.id)?.state).toBe("paused");
    // A running job that is not resumable finishes.
    expect((await t.wait(plain.id)).state).toBe("succeeded");
    // Slots are free, but nothing is dispatched while paused.
    const queued = await t.submit("test.echo", { value: 2 });
    expect(queued.state).toBe("queued");
    await new Promise((r) => setTimeout(r, 200));
    expect(t.jobs.get(queued.id)?.state).toBe("queued");
    await run(t.engine.resume({ all: true }));
    expect((await t.wait(queued.id)).state).toBe("succeeded");
    expect((await t.wait(resumable.id, 30_000)).state).toBe("succeeded");
  }, 40_000);

  test("resume_requeues", async () => {
    const t = await testEngine({ cores: 2 });
    const busy = await t.submit("test.sleep", { ms: 60_000 });
    const v = await t.submit("test.echo", { value: 3 });
    await run(t.engine.pause({ job: v.id }));
    await run(t.engine.cancel(busy.id));
    await new Promise((r) => setTimeout(r, 100));
    expect(t.jobs.get(v.id)?.state).toBe("paused"); // a paused job is not dispatched
    await run(t.engine.resume({ job: v.id }));
    expect((await t.wait(v.id)).state).toBe("succeeded");
    expect((await failure(t.engine.resume({ job: "nope" }))).code).toBe("JOB_NOT_FOUND");
  });
});

/** Two `io:lib` jobs whose workers are abandoned when cancelled (their exits held `holds` ms). */
async function abandonTwo(holds: [number, number]): Promise<{ t: TestEngine; ids: [string, string] }> {
  const t = await testEngine({ testKinds: { overrides: { "test.hang": { resources: ["io:lib"] } } } });
  t.engine.definePool("io:lib", 2);
  const a = await t.submit("test.hang", {}, { priority: "interactive" });
  const b = await t.submit("test.hang", {}, { priority: "interactive" });
  const host = hostOf(t.engine);
  host.abandonForTest?.(await procOf(t, a.id), holds[0]);
  host.abandonForTest?.(await procOf(t, b.id), holds[1]);
  await Promise.all([run(t.engine.cancel(a.id)), run(t.engine.cancel(b.id))]);
  return { t, ids: [a.id, b.id] };
}

describe("abandonment", () => {
  test("two_abandoned_mark_io_pool_stuck", async () => {
    const { t, ids } = await abandonTwo([8000, 8000]);
    for (const id of ids) expect(t.jobs.get(id)?.state).toBe("cancelled");
    expect(t.sink.exits.map((e) => e.reason)).toEqual(["abandoned", "abandoned"]);
    expect(t.engine.workers().stuck).toEqual(["io:lib"]);
    expect(t.engine.workers().pools.find((p) => p.name === "io:lib")).toMatchObject({ used: 0, stuck: true });
  }, 30_000);

  test("stuck_pool_refuses_submit_resource_stuck", async () => {
    const { t } = await abandonTwo([8000, 8000]);
    const e = await failure(t.engine.submit(OWNER, { kind: "test.hang", params: {} }));
    expect(e).toMatchObject({ code: "RESOURCE_STUCK", details: { pool: "io:lib" } });
    // Other pools are not affected.
    expect((await t.runJob("test.echo", { value: 1 })).state).toBe("succeeded");
  }, 30_000);

  test("stuck_clears_when_one_exits", async () => {
    // Holds well apart (9b review): the first exits long before the second, on a loaded machine too.
    const { t } = await abandonTwo([6000, 20_000]);
    expect(t.engine.workers().stuck).toEqual(["io:lib"]);
    await until(() => t.engine.workers().stuck.length === 0, 25_000);
    const v = await t.submit("test.hang", {}, { priority: "interactive" });
    expect(v.state).toBe("running");
    await run(t.engine.cancel(v.id));
  }, 60_000);
});
