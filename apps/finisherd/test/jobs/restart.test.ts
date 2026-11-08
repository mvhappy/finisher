// Persistence, start-up recovery and close (5C-1 plan Task 9b, Decisions 26, 28–30, 40).
// `crash()` stops an engine as a dead `finisherd` would: nothing more is written.
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { openControlStore } from "../../src/control/db.ts";
import type { SucceededCtx } from "../../src/jobs/kinds.ts";
import { JobStore } from "../../src/jobs/store.ts";
import { closeTestEngines, run, type TestEngine, testClock, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

const scratch: string[] = [];
afterEach(() => {
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});
function serverData(): string {
  const d = mkdtempSync(join(tmpdir(), "fin-restart-"));
  scratch.push(d);
  return d;
}

const items = (t: TestEngine, job: string): string[] =>
  t.events.filter((e) => e.type === "item" && e.job.id === job).map((e) => (e as { item: string }).item);

describe("persistence", () => {
  test("transitions_written_at_once", async () => {
    const t = await testEngine({ cores: 2 });
    const a = await t.submit("test.sleep", { ms: 60_000 });
    expect(t.jobs.get(a.id)).toMatchObject({ state: "running", attempts: 1 });
    const b = await t.submit("test.echo", { value: 1 });
    expect(t.jobs.get(b.id)?.state).toBe("queued");
    await run(t.engine.pause({ job: b.id }));
    expect(t.jobs.get(b.id)?.state).toBe("paused");
    await run(t.engine.cancel(a.id));
    expect(t.jobs.get(a.id)).toMatchObject({ state: "cancelled", error: { code: "JOB_CANCELLED" } });
    expect(t.jobs.get(a.id)?.finishedAt).toBeDefined();
  });

  test("checkpoint_throttled_to_10s", async () => {
    const tc = await testClock();
    const t = await testEngine({ clock: tc.clock });
    const v = await t.submit("test.checkpoint", { steps: 100_000, stepMs: 2 });
    // The first checkpoint is written at once, then at most one per 10 s of the engine's clock.
    await until(() => (t.jobs.row(v.id)?.checkpoint as { step?: number } | undefined)?.step === 1);
    await until(() => (t.engine.list({}).jobs[0]?.progress?.done ?? 0) > 20);
    expect(t.jobs.row(v.id)?.checkpoint).toEqual({ step: 1 });
    await tc.adjust(10_000);
    await until(() => ((t.jobs.row(v.id)?.checkpoint as { step: number }).step ?? 0) > 20);
    await run(t.engine.cancel(v.id));
    await t.close();
    await tc.close();
  });
});

describe("restart", () => {
  test("restart_requeues_resumable_from_checkpoint", async () => {
    const data = serverData();
    const first = await testEngine({ serverData: data });
    const v = await first.submit("test.checkpoint", { steps: 60, stepMs: 20 });
    await until(() => (first.jobs.row(v.id)?.checkpoint as { step?: number } | undefined)?.step !== undefined);
    await new Promise((r) => setTimeout(r, 100));
    await first.crash();
    const second = await testEngine({ serverData: data });
    const step = (second.jobs.row(v.id)?.checkpoint as { step: number }).step;
    expect(step).toBeGreaterThanOrEqual(1);
    const done = await second.wait(v.id);
    expect(done).toMatchObject({ state: "succeeded", attempts: 2 });
    expect((done.result as { resumedFrom: number }).resumedFrom).toBe(step);
    expect(second.sink.done).toEqual([expect.objectContaining({ outcome: "succeeded", attempts: 2 })]);
  });

  test("restart_interrupts_non_resumable", async () => {
    const data = serverData();
    const opts = { serverData: data, testKinds: { overrides: { "test.sleep": { dedupe: () => "one" } } } };
    const first = await testEngine(opts);
    const v = await first.submit("test.sleep", { ms: 60_000 });
    await first.crash();
    const second = await testEngine(opts);
    expect(second.jobs.get(v.id)).toMatchObject({ state: "interrupted", error: { code: "JOB_INTERRUPTED" } });
    // No `running` row is left behind to hold its dedupe key (9a review M2).
    expect(second.jobs.list({ state: ["running"] }).jobs).toEqual([]);
    const again = await second.submit("test.sleep", { ms: 1 });
    expect(again.id).not.toBe(v.id);
    expect(second.sink.done).toEqual([expect.objectContaining({ outcome: "interrupted", code: "JOB_INTERRUPTED" })]);
    expect(existsSync(join(data, "jobs", v.id))).toBe(false);
  });

  test("restart_keeps_paused", async () => {
    const data = serverData();
    const first = await testEngine({ serverData: data, cores: 2 });
    await first.submit("test.sleep", { ms: 60_000 });
    const p = await first.submit("test.echo", { value: 1 });
    await run(first.engine.pause({ job: p.id }));
    await first.crash();
    const second = await testEngine({ serverData: data });
    await new Promise((r) => setTimeout(r, 100));
    expect(second.jobs.get(p.id)?.state).toBe("paused");
    await run(second.engine.resume({ job: p.id }));
    expect((await second.wait(p.id)).state).toBe("succeeded");
  });

  test("restart_reruns_on_succeeded", async () => {
    const data = serverData();
    const calls: Array<{ rerun: boolean; dir: boolean; marker: boolean }> = [];
    let first: TestEngine | undefined;
    let crashed: Promise<void> | undefined;
    const onSucceeded = (ctx: SucceededCtx<never, never>): Effect.Effect<void, FinError> =>
      Effect.suspend(() => {
        calls.push({ rerun: ctx.rerun, dir: existsSync(ctx.dir), marker: existsSync(ctx.path("hook-was-here")) });
        if (calls.length > 1) return Effect.void;
        // Half-way through: it leaves a trace in the directory, then the engine dies.
        return Effect.promise(async () => {
          const { writeFile } = await import("node:fs/promises");
          await writeFile(ctx.path("hook-was-here"), "1");
          crashed = first?.crash();
        }).pipe(Effect.andThen(Effect.never));
      });
    const opts = { serverData: data, testKinds: { overrides: { "test.echo": { onSucceeded: onSucceeded as never } } } };
    first = await testEngine(opts);
    const v = await first.submit("test.echo", { value: 1 });
    await until(() => crashed);
    await crashed;
    expect(first.sink.done).toEqual([]);
    const second = await testEngine(opts);
    const done = await second.wait(v.id);
    expect(done).toMatchObject({ state: "succeeded", result: expect.objectContaining({ value: 1 }) });
    // The directory was intact for the re-run, and the re-run knew it was one.
    expect(calls).toEqual([
      { rerun: false, dir: true, marker: false },
      { rerun: true, dir: true, marker: true },
    ]);
    expect(second.sink.done).toHaveLength(1);
  });

  test("items_reported_in_the_last_second_may_rerun", async () => {
    // Decision 26: items are written once a second. An item acknowledged just before the
    // crash is lost and runs again; one written before it never does.
    const data = serverData();
    const first = await testEngine({ serverData: data });
    const v = await first.submit("test.crash", { items: ["a", "b", "c", "d", "e", "f"], how: "exit", itemMs: 400 });
    await until(() => items(first, v.id).includes("b"));
    await until(() => first.jobs.items(v.id).has("a")); // `a` was written (the first flush)
    const written = [...first.jobs.items(v.id).keys()];
    const acknowledged = items(first, v.id);
    await first.crash();
    const second = await testEngine({ serverData: data });
    expect((await second.wait(v.id, 30_000)).state).toBe("succeeded");
    const rerun = items(second, v.id);
    for (const item of written) expect(rerun).not.toContain(item);
    // What was acknowledged and not written yet ran again (at least once, never zero times).
    for (const item of acknowledged.filter((i) => !written.includes(i))) expect(rerun).toContain(item);
    expect(new Set([...written, ...rerun])).toEqual(new Set(["a", "b", "c", "d", "e", "f"]));
  }, 30_000);
});

describe("close and retention", () => {
  test("close_leaves_no_running_rows", async () => {
    const data = serverData();
    const t = await testEngine({
      serverData: data,
      cores: 4,
      testKinds: { overrides: { "test.echo": { onSucceeded: () => Effect.sleep("300 millis") } } },
    });
    const resumable = await t.submit("test.checkpoint", { steps: 400, stepMs: 20 });
    const plain = await t.submit("test.sleep", { ms: 60_000 });
    const hooked = await t.submit("test.echo", { value: 1 });
    await until(() => t.jobs.row(hooked.id)?.phase === "hooks");
    await until(() => t.jobs.row(resumable.id)?.checkpoint !== undefined);
    await t.close();
    const control = openControlStore(data, { serverId: "sid-test", checkpoint: false });
    const after = new JobStore(control);
    expect(after.list({ state: ["running"] }).jobs).toEqual([]);
    expect(after.get(resumable.id)?.state).toBe("queued");
    expect(after.get(plain.id)).toMatchObject({ state: "interrupted", error: { code: "JOB_INTERRUPTED" } });
    expect(after.get(hooked.id)?.state).toBe("succeeded");
    expect(t.sink.done.map((d) => d.outcome).sort()).toEqual(["interrupted", "succeeded"]);
    await control.close();
  });

  test("retention_runs_hourly", async () => {
    const tc = await testClock();
    const t = await testEngine({ clock: tc.clock });
    const v = await t.runJob("test.echo", { value: 1 });
    expect(t.jobs.get(v.id)?.state).toBe("succeeded");
    await tc.adjust(6 * 24 * 3_600_000);
    expect(t.jobs.get(v.id)).toBeDefined(); // younger than 7 days
    await tc.adjust(2 * 24 * 3_600_000);
    await until(() => t.jobs.get(v.id) === undefined);
    await t.close();
    await tc.close();
  });
});
