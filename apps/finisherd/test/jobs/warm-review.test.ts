// The Task 11 review's findings (5C-1 plan Task 11; `fix(jobs): Task 11 review`): B1, H1–H3,
// M1–M3 and L1–L5, each held by a test. Scripted `FakeHost` workers where the condition is
// hard to cause with real processes; real `fin-workers test --warm` elsewhere.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Hello, HostMsg, JobSpawnRequest } from "@finisher/core-types";
import { afterEach, describe, expect, test } from "vitest";
import { Engine } from "../../src/jobs/engine.ts";
import { heldFiles } from "../../src/jobs/held-files.ts";
import { makePools } from "../../src/jobs/pools.ts";
import { QueueIndex, type QueuedJob, schedule } from "../../src/jobs/schedule.ts";
import { FakeHost } from "../support/fake-host.ts";
import { closeTestEngines, run, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);
const scratch: string[] = [];
afterEach(() => {
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});

const warmHello = (proc: number, over: Partial<Hello> = {}): Hello => ({
  v: 1,
  minor: 0,
  role: "worker",
  family: "test",
  kinds: [
    { kind: "test.warm", version: 1 },
    { kind: "test.helper", version: 1 },
  ],
  warm: true,
  pid: 1000 + proc,
  ...over,
});
const isWarm = (req: JobSpawnRequest): boolean => req.args?.includes("--warm") === true;

/** A scripted `test` worker: warm ones answer and send `ready`; cold ones answer and exit. */
function workers(hello: (proc: number, req: JobSpawnRequest) => Hello = (proc, req) => warmHello(proc, { warm: isWarm(req) })): FakeHost {
  const reqs = new Map<number, JobSpawnRequest>();
  const fake = new FakeHost((host, proc, req) => {
    reqs.set(proc, req);
    if (req.role === "worker") host.emit({ type: "hello", proc, hello: hello(proc, req) });
  });
  fake.onSend = (host, proc, msg) => {
    if (msg.t !== "start") return;
    host.emit({ type: "message", proc, msg: { t: "result", job: msg.job.id, value: { value: null, pid: 1000 + proc, served: 1 } } });
    const req = reqs.get(proc);
    if (req !== undefined && isWarm(req)) host.emit({ type: "message", proc, msg: { t: "ready" } });
    else host.exit(proc, { reason: "exited", code: 0 });
  };
  return fake;
}

const warmDirs = (jobsRoot: string): string[] => (existsSync(jobsRoot) ? readdirSync(jobsRoot).filter((n) => n.startsWith("warm-")) : []);

describe("B1: a hello that is not a warm worker of the family", () => {
  for (const [what, over] of [
    ["warm: false", { warm: false }],
    ["another family", { family: "testbad" }],
    ["a helper's role", { role: "helper" as const }],
  ] as const) {
    test(`fails_its_job_within_the_attempt_limits (${what})`, async () => {
      const fake = new FakeHost((host, proc) => host.emit({ type: "hello", proc, hello: warmHello(proc, over as Partial<Hello>) }));
      const t = await testEngine({ jobHost: fake.factory });
      const v = await t.runJob("test.warm", { value: 1 }, { maxAttempts: 5 });
      expect(v).toMatchObject({ state: "failed", error: { code: "WORKER_PROTOCOL_MISMATCH" } });
      const spawned = fake.spawns.length;
      expect(spawned).toBeLessThanOrEqual(5);
      // Nothing keeps respawning once the job ended.
      await new Promise((r) => setTimeout(r, 50));
      expect(fake.spawns).toHaveLength(spawned);
      expect(t.engine.workers().warm).toEqual([]);
    });
  }
});

describe("H1: a warm attempt that throws", () => {
  test("leaves_the_worker_ready_when_no_start_was_sent", async () => {
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.helper": {
            warm: true,
            defaultPriority: "interactive",
            memory: () => {
              throw new Error("the memory rule failed");
            },
          },
        },
      },
    });
    const w = (await t.runJob("test.warm", { value: 0 })).result as { pid: number };
    const v = await t.runJob("test.helper", {}, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "INTERNAL" } });
    expect(t.engine.workers().warm).toMatchObject([{ pid: w.pid, state: "ready", served: 1 }]);
    // The pool is not stalled: the next job runs on the same worker.
    const next = await t.runJob("test.warm", { value: 1 });
    expect(next.result).toMatchObject({ pid: w.pid, served: 2 });
  });
});

describe("H2: temp state never crosses jobs", () => {
  test("tmp_is_emptied_after_each_job_and_names_are_per_run", async () => {
    const t = await testEngine();
    await t.runJob("test.warm", { value: 0 });
    const root = join(t.serverData, "jobs");
    const [dir] = warmDirs(root);
    expect(dir).toMatch(/^warm-[0-9a-f]{8}-1$/);
    writeFileSync(join(root, dir as string, "tmp", "left-by-job-1"), "x");
    mkdirSync(join(root, dir as string, "tmp", "nested"));
    await t.runJob("test.warm", { value: 1 });
    expect(readdirSync(join(root, dir as string, "tmp"))).toEqual([]);
  });

  test("a_stale_warm_dir_is_swept_at_start", async () => {
    const first = await testEngine({ keepData: true });
    const stale = join(first.serverData, "jobs", "warm-1", "tmp");
    await first.close();
    mkdirSync(stale, { recursive: true });
    writeFileSync(join(stale, "junk"), "x");
    const t = await testEngine({ serverData: first.serverData });
    expect(existsSync(join(t.serverData, "jobs", "warm-1"))).toBe(false);
    await t.runJob("test.warm", { value: 0 });
    const [dir] = warmDirs(join(t.serverData, "jobs"));
    expect(dir).not.toBe("warm-1");
    expect(readdirSync(join(t.serverData, "jobs", dir as string, "tmp"))).toEqual([]);
  });

  test("close_waits_for_the_directories", async () => {
    const t = await testEngine({ keepData: true });
    await t.runJob("test.warm", { value: 0 });
    expect(warmDirs(join(t.serverData, "jobs"))).toHaveLength(1);
    await t.close();
    expect(warmDirs(join(t.serverData, "jobs"))).toEqual([]);
  });
});

describe("H3: no warm workers under a sandbox", () => {
  test("caps_sandboxed_runs_warm_kinds_cold", async () => {
    const fake = workers();
    fake.capsOverride = { sandboxed: true, sandbox: "landlock" };
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.warm", { value: 1 });
    expect(v).toMatchObject({ state: "succeeded", warm: false });
    await run(t.engine.warm("test"));
    expect(fake.spawns.filter(isWarm)).toEqual([]);
    expect(t.engine.workers().warm).toEqual([]);
  });

  test("a_self_sandboxed_hello_turns_warm_kinds_cold", async () => {
    const fake = workers((proc, req) => warmHello(proc, { warm: isWarm(req), ...(isWarm(req) ? { selfSandbox: "worker" } : {}) }));
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.warm", { value: 1 });
    expect(v).toMatchObject({ state: "succeeded", warm: false, attempts: 1 });
    expect(fake.spawns.filter(isWarm)).toHaveLength(1);
    const again = await t.runJob("test.warm", { value: 2 });
    expect(again).toMatchObject({ state: "succeeded", warm: false });
    expect(fake.spawns.filter(isWarm)).toHaveLength(1);
    expect(t.engine.workers().warm).toEqual([]);
  });
});

describe("M1: warm keys are encoded, not joined", () => {
  const job = (id: string, family: string, key: string): QueuedJob => ({ id, priority: "interactive", createdAt: 1, resources: ["cpu"], warm: { family, key } });
  test("a_key_with_separators_is_its_own_fifo", () => {
    const q = new QueueIndex();
    q.add(job("pipe", "f", "a|cpu*1|x"));
    q.add(job("colon", "f:a", ""));
    expect(q.warmHead("f", "a", 2)).toBeUndefined();
    expect(q.warmHead("f", "a|cpu*1|x", 2)?.id).toBe("pipe");
    expect(q.warmHead("f", "a:", 2)).toBeUndefined();
    expect(q.warmHead("f:a", "", 2)?.id).toBe("colon");
    expect(q.warmIds().sort()).toEqual(["colon", "pipe"]);
  });
});

describe("M2: fresh retries", () => {
  test("a_worker_error_retries_on_the_same_worker", async () => {
    let failed = false;
    const fake = new FakeHost((host, proc) => host.emit({ type: "hello", proc, hello: warmHello(proc) }));
    fake.onSend = (host, proc, msg: HostMsg) => {
      if (msg.t !== "start") return;
      if (!failed) {
        failed = true;
        host.emit({ type: "message", proc, msg: { t: "error", job: msg.job.id, error: { code: "IO_ERROR", message: "transient", retry: "backoff" } } });
      } else host.emit({ type: "message", proc, msg: { t: "result", job: msg.job.id, value: { value: null, pid: 1000 + proc, served: 2 } } });
      host.emit({ type: "message", proc, msg: { t: "ready" } });
    };
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.submit("test.warm", { value: 1 }, { maxAttempts: 3 });
    // The worker was not lost: its retry needs no fresh one.
    await until(() => t.jobs.get(v.id)?.state === "queued" || t.jobs.get(v.id)?.finishedAt !== undefined);
    const q = (t.engine as Engine).queue.get(v.id);
    expect(q?.warm?.fresh).toBeUndefined();
  });

  test("a_fresh_retry_in_another_bucket_is_not_starved", () => {
    const pools = makePools({ cpu: 4 });
    const q = new QueueIndex();
    q.add({ id: "fresh", priority: "interactive", createdAt: 1, resources: ["cpu"], warm: { family: "f", key: "k", fresh: true } });
    q.add({ id: "other", priority: "interactive", createdAt: 2, resources: ["cpu", "cpu"], warm: { family: "f", key: "k" } });
    const warm = new Map([["f", { slots: 1, workers: [{ key: "k", state: "ready" as const, served: 3, idleSince: 0 }] }]]);
    // The reused worker is retired for the fresh retry instead of going to the younger job.
    expect(schedule({ queue: q, pools, warm }, 5)).toEqual([{ mode: "spawnWarm", family: "f", key: "k", retire: 0 }]);
    // With room in the pool the other job keeps the reused worker meanwhile.
    const roomy = new Map([["f", { slots: 2, workers: [{ key: "k", state: "ready" as const, served: 3, idleSince: 0 }] }]]);
    expect(schedule({ queue: q, pools, warm: roomy }, 5)).toEqual([
      { job: "other", mode: "warm", family: "f", key: "k" },
      { mode: "spawnWarm", family: "f", key: "k" },
    ]);
  });
});

describe("M3: pump never nests", () => {
  test("a_pump_asked_during_a_pump_runs_after_it", async () => {
    const t = await testEngine();
    const e = t.engine as Engine;
    const real = e.schedule;
    let depth = 0;
    let maxDepth = 0;
    let calls = 0;
    e.schedule = (state, now) => {
      depth++;
      calls++;
      maxDepth = Math.max(maxDepth, depth);
      if (calls === 1) e.pump(); // as a synchronous `give()` would
      const out = real(state, now);
      depth--;
      return out;
    };
    e.pump();
    e.schedule = real;
    expect(maxDepth).toBe(1);
    expect(calls).toBe(2);
  });
});

describe("Low", () => {
  test("L1_close_while_spawning_spawns_nothing_more", async () => {
    const fake = new FakeHost(() => {}); // never says hello
    const t = await testEngine({ jobHost: fake.factory });
    await t.submit("test.warm", { value: 1 });
    await until(() => t.engine.workers().warm.find((w) => w.state === "spawning"));
    const spawned = fake.spawns.length;
    await t.close();
    expect(fake.spawns).toHaveLength(spawned);
  });

  test("L2_an_abandoned_idle_worker_holds_like_a_job_process", async () => {
    const fake = workers();
    const t = await testEngine({ jobHost: fake.factory });
    await t.runJob("test.warm", { value: 1 });
    const e = t.engine as Engine;
    fake.exit(1, { reason: "abandoned" });
    expect(e.abandonment.holds("warm test 1")).toBe(true);
    fake.emit({ type: "abandonedExit", proc: 1 });
    expect(e.abandonment.holds("warm test 1")).toBe(false);
  });

  test("L3_the_held_files_check_fails_when_it_cannot_be_made", async () => {
    const dir = mkdtempSync(join(tmpdir(), "fin-held-"));
    scratch.push(dir);
    await expect(heldFiles(2 ** 30, dir, "linux")).rejects.toThrow();
    await expect(heldFiles(undefined, dir)).rejects.toThrow();
    await expect(heldFiles(process.pid, dir, "freebsd")).rejects.toThrow();
    if (process.platform === "linux") expect(await heldFiles(process.pid, dir)).toEqual([]);
  });

  test("L4_a_failed_helper_cancels_the_worker_and_keeps_it", async () => {
    let helper: number | undefined;
    let worker: number | undefined;
    const fake = new FakeHost((host, proc, req) => {
      if (req.role === "helper") helper = proc;
      else worker = proc;
      if (req.role === "helper") host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "helper", kind: "codec", id: "test", pid: 2000 + proc } as Hello });
      else host.emit({ type: "hello", proc, hello: warmHello(proc) });
    });
    fake.onSend = (host, proc, msg) => {
      if (msg.t === "start") {
        // The helper fails once the worker has the job.
        if (helper !== undefined) host.emit({ type: "message", proc: helper, msg: { t: "error", job: msg.job.id, error: { code: "CODEC_BROKEN", message: "no codec" } } });
      } else if (msg.t === "cancel") {
        host.emit({ type: "message", proc, msg: { t: "error", job: msg.job, error: { code: "JOB_CANCELLED", message: "cancelled" } } });
        host.emit({ type: "message", proc, msg: { t: "ready" } });
      }
    };
    const t = await testEngine({ jobHost: fake.factory, testKinds: { overrides: { "test.helper": { warm: true, defaultPriority: "interactive" } } } });
    const v = await t.runJob("test.helper", {}, { maxAttempts: 1 });
    expect(v).toMatchObject({ state: "failed", error: { code: "HELPER_FAILED", details: { code: "CODEC_BROKEN" } } });
    if (worker === undefined) throw new Error("no worker was spawned");
    expect(fake.sent.some((s) => s.proc === worker && s.msg.t === "cancel")).toBe(true);
    expect(fake.alive(worker)).toBe(true);
    expect(t.engine.workers().warm).toMatchObject([{ state: "ready", served: 1 }]);
  });

  test("L5_warm_of_a_keyed_family_starts_no_unkeyed_worker", async () => {
    const t = await testEngine();
    await run(t.engine.warm("testplugin"));
    await new Promise((r) => setTimeout(r, 20));
    expect(t.engine.workers().warm).toEqual([]);
  });
});
