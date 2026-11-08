// A cold job's helpers (5C-1 plan Task 9b, Decision 39): spawned and attached before the worker,
// only their own files, a helper failure fails the job, they die with it and shut down after it.
import { readFileSync } from "node:fs";
import type { HostMsg, JobSpawnRequest } from "@finisher/core-types";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { CallCtx } from "../../src/jobs/kinds.ts";
import { recordingHost } from "../support/fake-host.ts";
import { closeTestEngines, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

function gone(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return false;
  } catch (e) {
    return (e as { code?: string }).code === "ESRCH";
  }
}

describe("helpers", () => {
  test("helpers_start_before_the_worker", async () => {
    const reqs: JobSpawnRequest[] = [];
    const sent: Array<{ proc: number; msg: HostMsg }> = [];
    const t = await testEngine({ jobHost: recordingHost(reqs, sent) });
    const v = await t.runJob("test.helper", { helper: "ok" });
    expect(v.state).toBe("succeeded");
    expect(v.result).toEqual({ helpers: ["test"] });
    expect(reqs.map((r) => r.role)).toEqual(["helper", "worker"]);
    // The helper was attached before the worker was even spawned, and the worker's start names it.
    const kinds = sent.map((s) => s.msg.t);
    expect(kinds.indexOf("attach")).toBeLessThan(kinds.indexOf("start"));
    const start = sent.find((s) => s.msg.t === "start")?.msg;
    expect(start?.t === "start" ? start.helpers : undefined).toEqual([{ id: "test", kind: "codec", files: ["helper.bin"] }]);
    expect(t.sink.done[0]).toMatchObject({ helpers: 1 });
  });

  test("helpers_only_get_their_files", async () => {
    const reqs: JobSpawnRequest[] = [];
    const sent: Array<{ proc: number; msg: HostMsg }> = [];
    const t = await testEngine({
      jobHost: recordingHost(reqs, sent),
      testKinds: {
        overrides: {
          "test.helper": {
            files: [
              { name: "helper.bin", bytes: 4096, mode: "readWrite" },
              { name: "worker-only.bin", bytes: 16, mode: "write" },
            ],
          },
        },
      },
    });
    expect((await t.runJob("test.helper", { helper: "ok" })).state).toBe("succeeded");
    const attach = sent.find((s) => s.msg.t === "attach")?.msg;
    expect(attach?.t === "attach" ? attach.files?.map((f) => f.name) : undefined).toEqual(["helper.bin"]);
    const start = sent.find((s) => s.msg.t === "start")?.msg;
    expect(start?.t === "start" ? start.files?.map((f) => f.name) : undefined).toEqual(["helper.bin", "worker-only.bin"]);
    // Its grants: the job directory read-only, its own temp directory, its own file.
    const helper = reqs.find((r) => r.role === "helper");
    expect(helper?.grants?.map((g) => g.mode)).toEqual(["read", "readWrite", "readWrite"]);
    expect(helper?.cwd).toMatch(/helpers[\\/]test$/);
  });

  test("helper_exit_early_is_helper_failed", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.helper", { helper: "exitEarly", delayMs: 10_000 }, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "HELPER_FAILED", details: { helper: "test" } });
    expect(t.engine.workers().processes).toEqual([]);
    expect(t.sink.exits.find((e) => e.role === "helper")).toMatchObject({ helperKind: "codec", reason: "crashed" });
  });

  test.skipIf(process.platform === "win32")("helpers_die_with_the_job", async () => {
    let pids: { worker: number; mid: number; grandchild: number } | undefined;
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.helper": {
            onSucceeded: (ctx) =>
              Effect.sync(() => {
                pids = JSON.parse(readFileSync(ctx.path("helper.bin"), "utf8").replace(/\0+$/, "")) as typeof pids;
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.helper", { helper: "spawn", delayMs: 300 });
    expect(v.state).toBe("succeeded");
    expect(pids?.grandchild).toBeGreaterThan(0);
    // The hook ran after the whole group was gone: the helper and its grandchild too.
    expect(gone(pids!.worker)).toBe(true);
    await until(() => gone(pids!.grandchild), 5000);
  });

  test("helpers_shut_down_after_result", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.helper", { helper: "ok" });
    expect(v.state).toBe("succeeded");
    const helper = t.sink.exits.find((e) => e.role === "helper");
    // `shutdown` → exit 0, reported `done`, after it served the job.
    expect(helper).toMatchObject({ reason: "done", exitCode: 0, jobsServed: 1 });
  });

  test("helper_calls_reach_its_on_call", async () => {
    const calls: Array<{ ctx: CallCtx; method: string; params: unknown }> = [];
    const t = await testEngine({
      testKinds: {
        helper: {
          onCall: (ctx, method, params) =>
            Effect.sync(() => {
              calls.push({ ctx, method, params });
              return { ok: true };
            }),
        },
      },
    });
    const v = await t.runJob("test.helper", { helper: "call", delayMs: 300 });
    expect(v.state).toBe("succeeded");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ method: "test.echo", params: { from: "helper" }, ctx: { role: "helper", helper: { kind: "codec", id: "test" } } });
    expect(calls[0]?.ctx.job.id).toBe(v.id);
  });

  test("a_helper_hang_is_reported_and_the_job_fails", async () => {
    const t = await testEngine({ timing: { hungMs: 300 } });
    const v = await t.runJob("test.helper", { helper: "hang", delayMs: 10_000 }, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "HELPER_FAILED", details: { helper: "test", code: "WORKER_HUNG" } });
  });
});
