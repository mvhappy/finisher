// The kind hooks (5C-1 plan Decision 40): `prepare` before any process, `onSucceeded` after the
// group is gone and before the directory is deleted, persisted first, at-least-once.
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { FinError } from "@finisher/contracts";
import { Deferred, Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { JobEngine } from "../../src/jobs/engine.ts";
import type { JobRow } from "../../src/jobs/store.ts";
import { closeTestEngines, run, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as { code?: string }).code === "EPERM";
  }
}

describe("kind hooks", () => {
  test("prepare_writes_before_any_process", async () => {
    const holder: { engine?: JobEngine; processes?: number | undefined } = {};
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.files": {
            prepare: (ctx) =>
              Effect.promise(async () => {
                holder.processes = holder.engine?.workers().processes.length;
                await mkdir(ctx.path("in"), { recursive: true });
                await writeFile(ctx.path("in/data.txt"), "mine\n");
                return { extra: { from: "prepare", attempt: ctx.attempt } };
              }),
          },
        },
      },
    });
    holder.engine = t.engine;
    const v = await t.runJob("test.files", { failOnAttempt: 1 });
    expect(v.state).toBe("succeeded");
    expect(holder.processes).toBe(0);
    // Once per job directory: attempt 2 reused it, so `extra` is still attempt 1's.
    expect((t.jobs.row(v.id) as JobRow).extra).toEqual({ from: "prepare", attempt: 1 });
    expect((v.result as { files: Array<{ name: string; bytes: number }> }).files[0]).toMatchObject({ name: "in/data.txt", bytes: 5 });
  });

  test("prepare_failure_fails_the_job_without_a_process", async () => {
    const t = await testEngine({
      testKinds: { overrides: { "test.files": { prepare: () => Effect.fail(new FinError({ code: "PREP_BAD", message: "no", details: {} })) } } },
    });
    const v = await t.runJob("test.files", {});
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "PREP_BAD" } });
    expect(t.sink.exits).toHaveLength(0);
  });

  test("on_succeeded_runs_after_group_exit_before_delete", async () => {
    const seen: { pidAlive?: boolean; dir?: boolean; dirPath?: string } = {};
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.echo": {
            onSucceeded: (ctx) =>
              Effect.sync(() => {
                seen.pidAlive = alive((ctx.result as { pid: number }).pid);
                seen.dir = existsSync(ctx.dir);
                seen.dirPath = ctx.dir;
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.echo", { value: 1, report: ["pid"] });
    expect(v.state).toBe("succeeded");
    expect(seen).toMatchObject({ pidAlive: false, dir: true });
    expect(existsSync(seen.dirPath!)).toBe(false);
  });

  test("result_and_phase_persisted_before_the_hook", async () => {
    const holder: { row?: JobRow | undefined; read?: (id: string) => JobRow | undefined } = {};
    const t = await testEngine({
      testKinds: {
        overrides: { "test.echo": { onSucceeded: (ctx) => Effect.sync(() => void (holder.row = holder.read?.(ctx.job.id))) } },
      },
    });
    holder.read = (id) => t.jobs.row(id);
    const v = await t.runJob("test.echo", { value: "r" });
    expect(holder.row).toMatchObject({ state: "running", phase: "hooks", result: { value: "r" } });
    expect(v.state).toBe("succeeded");
  });

  test("on_succeeded_can_replace_the_result", async () => {
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.echo": {
            onSucceeded: (ctx) => Effect.succeed({ replaced: (ctx.result as { value: unknown }).value, rerun: ctx.rerun, items: ctx.items.size }),
          },
        },
      },
    });
    const v = await t.runJob("test.echo", { value: 7 });
    expect(v.result).toEqual({ replaced: 7, rerun: false, items: 0 });
  });

  test("on_succeeded_failure_fails_the_job", async () => {
    const t = await testEngine({
      testKinds: { overrides: { "test.echo": { onSucceeded: () => Effect.fail(new FinError({ code: "HOOK_BAD", message: "no", details: { why: 1 } })) } } },
    });
    const v = await t.runJob("test.echo", { value: 1 });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "HOOK_BAD", details: { why: 1 } } });
    expect(t.sink.done[0]).toMatchObject({ outcome: "failed", code: "HOOK_BAD" });
  });

  test("hook_timeout_is_job_timeout", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.echo": { hookTimeoutS: 0.2, onSucceeded: () => Effect.never } } } });
    const v = await t.runJob("test.echo", { value: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "JOB_TIMEOUT", details: { hook: "onSucceeded" } } });
  });

  test("cancel_during_hook_ends_succeeded", async () => {
    const gate = Effect.runSync(Deferred.make<void>());
    let entered = false;
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.echo": {
            onSucceeded: () =>
              Effect.sync(() => {
                entered = true;
              }).pipe(Effect.andThen(Deferred.await(gate))),
          },
        },
      },
    });
    const v = await t.submit("test.echo", { value: 1 });
    await until(() => entered);
    const during = await run(t.engine.cancel(v.id));
    expect(during.state).toBe("running");
    Effect.runSync(Deferred.succeed(gate, undefined));
    const end = await t.wait(v.id);
    expect(end.state).toBe("succeeded");
    expect(t.sink.done).toHaveLength(1);
  });

  test("cancel_queued_and_running_before_9b", async () => {
    const t = await testEngine({ cores: 2 });
    const busy = await t.submit("test.sleep", { ms: 2000 });
    const queued = await t.submit("test.sleep", { ms: 1 });
    const c = await run(t.engine.cancel(queued.id));
    expect(c).toMatchObject({ state: "cancelled", error: { code: "JOB_CANCELLED" } });
    const e = await Effect.runPromiseExit(t.engine.cancel(busy.id));
    expect(e._tag).toBe("Failure");
  });
});
