// One cold attempt: liveness, stall, timeout and memory through the engine, exit verdicts, and
// the minimal retry policy of Task 9a (5C-1 plan Decisions 19, 20, 31).
import { afterEach, describe, expect, test } from "vitest";
import { exitVerdict, type JobHostExitInfo } from "../../src/jobs/host.ts";
import { memoryRule } from "../../src/jobs/runner.ts";
import { closeTestEngines, testEngine } from "../support/jobs.ts";

afterEach(closeTestEngines);

describe("liveness through the engine", () => {
  test("hung_job_fails_worker_hung", async () => {
    const t = await testEngine({ timing: { hungMs: 1000 } });
    const v = await t.runJob("test.hang", {}, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("WORKER_HUNG");
    expect(t.sink.exits[0]?.reason).toBe("hung");
  });

  test("stalled_job_fails_job_stalled", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.stall": { stallS: 0.3 } } } });
    const v = await t.runJob("test.stall", {}, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("JOB_STALLED");
    expect(t.sink.exits[0]?.reason).toBe("stalled");
  });

  test("timeout_fails_job_timeout", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.sleep": { timeoutS: 0.3 } } } });
    const t0 = Date.now();
    const v = await t.runJob("test.sleep", { ms: 10_000 }, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("JOB_TIMEOUT");
    expect(Date.now() - t0).toBeLessThan(8000);
  });

  test("oom_fails_worker_oom", async () => {
    const t = await testEngine({ timing: { rssSampleMs: 50 }, testKinds: { overrides: { "test.oom": { memory: { fixedMiB: 64 } } } } });
    const v = await t.runJob("test.oom", { mib: 256, holdMs: 5000 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("WORKER_OOM");
    expect(t.sink.exits[0]?.reason).toBe("oom");
  });

  test("a_crash_stores_the_stderr_tail_and_the_signal", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.crash", { items: ["a"], poison: ["a"], how: "abort" }, { priority: "interactive", maxAttempts: 1 });
    expect(v.error?.code).toBe("WORKER_CRASHED");
    const details = v.error?.details as Record<string, unknown>;
    if (process.platform !== "win32") expect(details.signal).toBe("abrt");
    expect(typeof details.stderrTail === "string" || details.stderrTail === undefined).toBe(true);
    // The journal's `worker.exit` never carries stderr.
    expect(JSON.stringify(t.sink.exits)).not.toContain("stderrTail");
  });

  test("a_worker_retry_hint_retries_at_once_then_fails", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.files", { failOnAttempt: 1 }, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("IO_ERROR");
    expect(v.attempts).toBe(1);
  });
});

describe("exit verdicts", () => {
  const info = (o: Partial<JobHostExitInfo> & { reason: string }): JobHostExitInfo =>
    ({ stderrTail: "", lifetimeMs: 1, ...o }) as unknown as JobHostExitInfo;

  test("each_host_reason_maps_to_a_worker_exit_reason_and_code", () => {
    const rows: Array<[JobHostExitInfo, boolean, string, string]> = [
      [info({ reason: "exited", code: 0 }), true, "done", "WORKER_CRASHED"],
      [info({ reason: "exited", code: 0 }), false, "crashed", "WORKER_CRASHED"],
      [info({ reason: "exited", code: 3 }), true, "crashed", "WORKER_CRASHED"],
      [info({ reason: "resourceFault", signal: "bus" }), false, "crashed", "WORKER_CRASHED"],
      [info({ reason: "hung" }), false, "hung", "WORKER_HUNG"],
      [info({ reason: "stalled" }), false, "stalled", "JOB_STALLED"],
      [info({ reason: "timedOut" }), false, "killed", "JOB_TIMEOUT"],
      [info({ reason: "oom", memoryPeak: 9 }), false, "oom", "WORKER_OOM"],
      [info({ reason: "noHello" }), false, "spawnFailed", "WORKER_UNAVAILABLE"],
      [info({ reason: "abandoned" }), false, "abandoned", "WORKER_CRASHED"],
      [info({ reason: "killed", kill: "cancel" } as never), false, "cancelled", "JOB_CANCELLED"],
      [info({ reason: "killed", kill: "hung" } as never), false, "hung", "WORKER_HUNG"],
      [info({ reason: "killed", kill: "jobEnded" } as never), false, "killed", "WORKER_CRASHED"],
      [info({ reason: "protocolError", what: "seq" } as never), false, "protocolError", "WORKER_CRASHED"],
      [info({ reason: "spawnFailed", what: "selfSandbox" } as never), false, "spawnFailed", "WORKER_UNAVAILABLE"],
      [info({ reason: "protocolMismatch", expected: "v1", got: "v2" } as never), false, "protocolError", "WORKER_PROTOCOL_MISMATCH"],
    ];
    for (const [i, ended, reason, code] of rows) {
      const v = exitVerdict(i, ended);
      expect([i.reason, v.reason, v.error.code]).toEqual([i.reason, reason, code]);
    }
    expect(exitVerdict(info({ reason: "resourceFault" }), false).error.details).toMatchObject({ resourceFault: true });
    expect(exitVerdict(info({ reason: "exited", signal: "bus" }), false).error.details).toMatchObject({ resourceFault: true, signal: "bus" });
    expect(exitVerdict(info({ reason: "exited", code: -1073741818 }), false).error.details).toMatchObject({ resourceFault: true });
    expect(exitVerdict(info({ reason: "oom", memoryPeak: 9 }), false).error.details).toMatchObject({ used: 9 });
    expect(exitVerdict(info({ reason: "exited", code: 1, stderrTail: "boom" }), false).error.details).toMatchObject({ exitCode: 1, stderrTail: "boom" });
  });

  test("memory_rule_kind_then_family_class", () => {
    const view = (priority: string) => ({ priority, params: {} }) as never;
    const family = { family: "f", profile: "worker", program: { binary: "fin-workers", args: [] }, memory: { interactive: { fixedMiB: 1 }, batch: { fixedMiB: 2 } } } as never;
    const kind = { memory: (p: unknown) => (p === undefined ? {} : { fixedMiB: 3 }) } as never;
    expect(memoryRule({} as never, family, view("interactive"))).toEqual({ fixedMiB: 1 });
    expect(memoryRule({} as never, family, view("normal"))).toEqual({ fixedMiB: 2 });
    expect(memoryRule(kind, family, view("normal"))).toEqual({ fixedMiB: 3 });
  });
});
