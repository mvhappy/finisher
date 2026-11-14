// The fixes of the Task 9b review (B1, H1, H2, M1–M5, L1–L6); each test names its finding.
import { execFileSync } from "node:child_process";
import { existsSync, linkSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Hello, HostMsg, JobSpawnRequest } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { openControlStore } from "../../src/control/db.ts";
import { Abandonment } from "../../src/jobs/control.ts";
import { ABANDONED_HOLD_MS, Engine } from "../../src/jobs/engine.ts";
import { createFiles, jobDirs, markPrepared, PREPARED_MARKER } from "../../src/jobs/files.ts";
import { checkTree, openMapped } from "../../src/jobs/fsafe.ts";
import { defaultPools } from "../../src/jobs/pools.ts";
import { RETENTION_EVERY_MS } from "../../src/jobs/restart.ts";
import { JobStore } from "../../src/jobs/store.ts";
import { FakeHost } from "../support/fake-host.ts";
import { recordingHost } from "../support/fake-host.ts";
import { closeTestEngines, failure, OWNER, run, testClock, testEngine, until } from "../support/jobs.ts";


const win = process.platform === "win32";
const scratch: string[] = [];
// Engines close before their data goes (Vitest runs `afterEach` hooks last-registered first,
// so two hooks would remove a directory an open engine still writes into).
afterEach(async () => {
  await closeTestEngines();
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});
function tmp(): string {
  const d = mkdtempSync(join(tmpdir(), "fin-9b-"));
  scratch.push(d);
  return d;
}

/** A row read from the server data after its engine closed. */
function rowAfter(serverData: string, id: string) {
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  try {
    return new JobStore(control).row(id);
  } finally {
    void control.close();
  }
}

const hello = (kind: string, family = "test"): Hello => ({ v: 1, minor: 0, role: "worker", family, kinds: [{ kind, version: 1 }], warm: false, pid: 1 });
const jobOf = (msg: HostMsg): string => (msg.t === "start" ? msg.job.id : "");

/** A worker that says hello and does what `onStart` says once it got `start`. */
function scripted(kind: string, onStart: (host: FakeHost, proc: number, msg: HostMsg & { t: "start" }, req: JobSpawnRequest) => void): FakeHost {
  const reqs = new Map<number, JobSpawnRequest>();
  const fake = new FakeHost((host, proc, req) => {
    reqs.set(proc, req);
    host.emit({ type: "hello", proc, hello: hello(kind) });
  });
  fake.onSend = (host, proc, msg) => {
    if (msg.t === "start") onStart(host, proc, msg, reqs.get(proc) as JobSpawnRequest);
  };
  return fake;
}

describe("B1: nothing a worker planted is followed by a hook", () => {
  test.skipIf(win)("a_link_in_out_before_the_hook_fails_the_job", async () => {
    const secret = join(tmp(), "id_ed25519");
    writeFileSync(secret, "secret key");
    let hookRan = false;
    const fake = scripted("test.files", (host, proc, msg, req) => {
      symlinkSync(secret, join(req.cwd as string, "out", "result"));
      host.emit({ type: "message", proc, msg: { t: "result", job: jobOf(msg), value: {} } });
      host.exit(proc, { reason: "exited", code: 0 });
    });
    const t = await testEngine({
      jobHost: fake.factory,
      testKinds: {
        overrides: {
          "test.files": {
            onSucceeded: (ctx) =>
              Effect.promise(async () => {
                hookRan = true;
                await (await ctx.open("out/result")).close();
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.files", {});
    expect(v).toMatchObject({ state: "failed", error: { code: "JOB_FAILED", details: { file: "out/result", problem: "link" } } });
    expect(hookRan).toBe(false);
    expect(readFileSync(secret, "utf8")).toBe("secret key");
  });

  test.skipIf(win)("the_restart_rerun_checks_the_directory_too", async () => {
    const serverData = tmp();
    const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    const store = new JobStore(control);
    const j = store.insert({ kind: "test.files", kindVersion: 1, family: "test", params: {}, owner: OWNER, priority: "normal", maxAttempts: 3 });
    store.transition(j.id, "running", { attempts: 1 });
    store.enterHooks(j.id, {});
    await control.close();
    const dir = join(serverData, "jobs", j.id);
    mkdirSync(join(dir, "out"), { recursive: true });
    symlinkSync(tmp(), join(dir, "out", "planted"));
    let hookRan = false;
    const t = await testEngine({ serverData, testKinds: { overrides: { "test.files": { onSucceeded: () => Effect.sync(() => void (hookRan = true)) } } } });
    expect(await t.wait(j.id)).toMatchObject({ state: "failed", error: { code: "JOB_FAILED", details: { problem: "link" } } });
    expect(hookRan).toBe(false);
  });

  test.skipIf(win)("check_tree_refuses_links_fifos_and_hard_links", async () => {
    const files = [{ name: "out/", mode: "write" as const }, { name: "out.bin", bytes: 4, mode: "write" as const }];
    const fresh = async (): Promise<string> => {
      const d = tmp();
      await createFiles(d, files);
      return d;
    };
    const ok = await fresh();
    mkdirSync(join(ok, "out", "deep"));
    writeFileSync(join(ok, "out", "deep", "f"), "x");
    await checkTree(ok, files);
    const link = await fresh();
    symlinkSync("/etc/passwd", join(link, "out", "deep"));
    await expect(checkTree(link, files)).rejects.toMatchObject({ code: "JOB_FAILED", details: { problem: "link", file: "out/deep" } });
    const fifo = await fresh();
    execFileSync("mkfifo", [join(fifo, "out", "pipe")]);
    await expect(checkTree(fifo, files)).rejects.toMatchObject({ details: { problem: "special" } });
    const hard = await fresh();
    writeFileSync(join(hard, "elsewhere"), "x");
    linkSync(join(hard, "elsewhere"), join(hard, "out", "linked"));
    await expect(checkTree(hard, files)).rejects.toMatchObject({ details: { problem: "hardLink" } });
    const replaced = await fresh();
    rmSync(join(replaced, "out.bin"));
    symlinkSync("/etc/passwd", join(replaced, "out.bin"));
    await expect(checkTree(replaced, files)).rejects.toMatchObject({ details: { problem: "link", file: "out.bin" } });
  });

  test.skipIf(win)("open_follows_no_link_and_opens_no_fifo", async () => {
    const files = [{ name: "out/", mode: "write" as const }, { name: "in.txt", mode: "read" as const }];
    const d = tmp();
    await createFiles(d, files);
    writeFileSync(join(d, "in.txt"), "input");
    const fh = await openMapped(d, files, "in.txt");
    expect(await fh.readFile("utf8")).toBe("input");
    await fh.close();
    const w = await openMapped(d, files, "out/new/file.txt", "w");
    await w.writeFile("made");
    await w.close();
    expect(readFileSync(join(d, "out", "new", "file.txt"), "utf8")).toBe("made");
    await expect(openMapped(d, files, "elsewhere.txt")).rejects.toMatchObject({ code: "INVALID_REQUEST" });
    await expect(openMapped(d, files, "out/../in.txt")).rejects.toMatchObject({ code: "INVALID_REQUEST" });
    const target = tmp();
    symlinkSync(target, join(d, "out", "via"));
    await expect(openMapped(d, files, "out/via/x", "w")).rejects.toMatchObject({ code: "JOB_FAILED", details: { problem: "link" } });
    expect(readdirSync(target)).toEqual([]);
    symlinkSync(join(d, "in.txt"), join(d, "out", "l"));
    await expect(openMapped(d, files, "out/l")).rejects.toMatchObject({ details: { problem: "link" } });
    execFileSync("mkfifo", [join(d, "out", "fifo")]);
    // Returns at once (no reader blocks a libuv thread) and is refused.
    await expect(openMapped(d, files, "out/fifo")).rejects.toMatchObject({ details: { problem: "special" } });
    linkSync(join(d, "in.txt"), join(d, "out", "hard"));
    await expect(openMapped(d, files, "out/hard", "w")).rejects.toMatchObject({ details: { problem: "hardLink" } });
    expect(readFileSync(join(d, "in.txt"), "utf8")).toBe("input"); // never truncated
  });
});

describe("H1: a job directory is never reused under a live process", () => {
  /** Attempt 1 answers `error {retry: fresh}` and is abandoned; attempt 2 succeeds. */
  function abandonedThenOk(): FakeHost {
    let n = 0;
    return scripted("test.echo", (host, proc, msg) => {
      if (++n === 1) {
        host.emit({ type: "message", proc, msg: { t: "error", job: jobOf(msg), error: { code: "GPU_DEVICE_LOST", message: "lost", retry: "fresh" } } });
        host.exit(proc, { reason: "abandoned" });
      } else {
        host.emit({ type: "message", proc, msg: { t: "result", job: jobOf(msg), value: { ok: true } } });
        host.exit(proc, { reason: "exited", code: 0 });
      }
    });
  }

  test("an_abandoned_process_holds_the_retry", async () => {
    const fake = abandonedThenOk();
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.submit("test.echo", { value: 1 });
    await until(() => t.jobs.get(v.id)?.state === "queued");
    await new Promise((r) => setTimeout(r, 200));
    expect(fake.spawns).toHaveLength(1); // held: no second attempt while the first process lives
    expect(t.jobs.get(v.id)?.state).toBe("queued");
    fake.emit({ type: "abandonedExit", proc: 1 });
    expect(await t.wait(v.id)).toMatchObject({ state: "succeeded", attempts: 2 });
    expect(fake.spawns).toHaveLength(2);
  });

  test("a_held_job_fails_resource_stuck_after_the_bound", async () => {
    const clock = await testClock();
    const fake = abandonedThenOk();
    const t = await testEngine({ jobHost: fake.factory, clock: clock.clock });
    const v = await t.submit("test.echo", { value: 1 });
    await until(() => t.jobs.get(v.id)?.state === "queued");
    await clock.adjust(ABANDONED_HOLD_MS);
    expect(await t.wait(v.id)).toMatchObject({ state: "failed", error: { code: "RESOURCE_STUCK", details: { reason: "abandoned" } } });
    expect(fake.spawns).toHaveLength(1);
    await t.close();
    await clock.close();
  });

  test.skipIf(win)("empty_dir_never_follows_a_planted_parent", async () => {
    const d = tmp();
    const elsewhere = tmp();
    symlinkSync(elsewhere, join(d, "a"));
    await expect(createFiles(d, [{ name: "a/out/", mode: "write" }])).rejects.toMatchObject({ code: "JOB_FAILED", details: { problem: "link" } });
    await expect(createFiles(d, [{ name: "a/f.bin", bytes: 4, mode: "write" }])).rejects.toMatchObject({ code: "JOB_FAILED" });
    expect(readdirSync(elsewhere)).toEqual([]);
  });

  test.skipIf(win)("mark_prepared_replaces_a_planted_link", async () => {
    const d = tmp();
    const target = join(tmp(), "victim");
    writeFileSync(target, "keep");
    const dirs = jobDirs(d, "01JOB");
    mkdirSync(dirs.job, { recursive: true });
    symlinkSync(target, join(dirs.job, PREPARED_MARKER));
    await markPrepared(dirs);
    expect(readFileSync(target, "utf8")).toBe("keep");
  });
});

describe("H2, L1: abandonment bookkeeping", () => {
  test("abandoned_exit_before_exit_is_remembered", async () => {
    // The host's race: the real exit is reported before the abandonment.
    const fake = new FakeHost((host, proc) => {
      host.emit({ type: "hello", proc, hello: hello("test.hang") });
      setImmediate(() => {
        host.emit({ type: "abandonedExit", proc });
        host.exit(proc, { reason: "abandoned" });
      });
    });
    const t = await testEngine({ jobHost: fake.factory, testKinds: { overrides: { "test.hang": { resources: ["io:lib"] } } } });
    t.engine.definePool("io:lib", 2);
    const a = await t.runJob("test.hang", {}, { maxAttempts: 1 });
    const b = await t.runJob("test.hang", {}, { maxAttempts: 1 });
    expect([a.state, b.state]).toEqual(["failed", "failed"]);
    expect(t.engine.workers().stuck).toEqual([]);
    expect((t.engine as Engine).abandonment.holds(a.id)).toBe(false);
  });

  test("abandoned_processes_are_counted_not_jobs", () => {
    const pools = defaultPools(2);
    pools.define("io:lib", 2);
    const released: string[] = [];
    const ab = new Abandonment(pools, () => {}, () => {}, (job) => released.push(job));
    ab.abandoned(1, "j", ["io:lib"]); // the worker
    ab.abandoned(2, "j", ["io:lib"]); // its helper
    expect(pools.snapshot().find((p) => p.name === "io:lib")?.stuck).toBe(true);
    expect(ab.holds("j")).toBe(true);
    ab.exited(1);
    expect(pools.snapshot().find((p) => p.name === "io:lib")?.stuck).toBe(false);
    expect(released).toEqual([]);
    ab.exited(2);
    expect(released).toEqual(["j"]);
    // A tombstone: the exit of 3 came first, so its abandonment is not counted.
    ab.exited(3);
    ab.abandoned(3, "k", ["io:lib"]);
    expect(ab.holds("k")).toBe(false);
  });
});

describe("M1, M2, L6: what a stop takes over", () => {
  /** `test.checkpoint` (resumable) whose worker answers a `cancel` with `answer`, or ignores it. */
  function onCancel(answer: { code: string } | "ignore"): FakeHost {
    const fake = new FakeHost((host, proc) => host.emit({ type: "hello", proc, hello: hello("test.checkpoint") }));
    fake.onSend = (host, proc, msg) => {
      if (msg.t !== "cancel" || answer === "ignore") return;
      host.emit({ type: "message", proc, msg: { t: "error", job: msg.job, error: { code: answer.code, message: "the answer" } } });
      host.exit(proc, { reason: "exited", code: 0 });
    };
    return fake;
  }

  test("pause_racing_an_error_answer_keeps_the_error", async () => {
    const fake = onCancel({ code: "IO_ERROR" });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.submit("test.checkpoint", { steps: 10 });
    await until(() => fake.sent.some((s) => s.msg.t === "start"));
    await run(t.engine.pause({ job: v.id }));
    expect(t.jobs.get(v.id)).toMatchObject({ state: "failed", error: { code: "IO_ERROR" } });
  });

  test("pause_answered_with_cancelled_pauses", async () => {
    const fake = onCancel({ code: "JOB_CANCELLED" });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.submit("test.checkpoint", { steps: 10 });
    await until(() => fake.sent.some((s) => s.msg.t === "start"));
    await run(t.engine.pause({ job: v.id }));
    expect(t.jobs.get(v.id)).toMatchObject({ state: "paused", attempts: 0 });
  });

  test("pause_then_close_stays_paused", async () => {
    const fake = onCancel("ignore");
    const serverData = tmp();
    const t = await testEngine({ jobHost: fake.factory, serverData });
    const v = await t.submit("test.checkpoint", { steps: 10 });
    await until(() => fake.sent.some((s) => s.msg.t === "start"));
    const paused = run(t.engine.pause({ job: v.id }));
    await until(() => (t.engine as Engine).runs.get(v.id)?.pauseRequested);
    await t.close();
    await paused;
    expect(rowAfter(serverData, v.id)).toMatchObject({ state: "paused", attempts: 0 });
  }, 20_000);

  test("close_spends_no_attempt_of_a_resumable_job", async () => {
    const serverData = tmp();
    const t = await testEngine({ serverData });
    const v = await t.submit("test.checkpoint", { steps: 1000, stepMs: 20 });
    await until(() => t.engine.workers().processes.some((p) => p.job === v.id && p.pid !== undefined));
    await new Promise((r) => setTimeout(r, 100));
    await t.close();
    expect(rowAfter(serverData, v.id)).toMatchObject({ state: "queued", attempts: 0 });
  }, 20_000);
});

describe("M3: prepare is bounded and stoppable", () => {
  test("prepare_times_out", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.files": { prepare: () => Effect.never, hookTimeoutS: 0.2 } } } });
    const v = await t.runJob("test.files", {});
    expect(v).toMatchObject({ state: "failed", error: { code: "JOB_TIMEOUT", details: { hook: "prepare" } } });
  });

  test("cancel_interrupts_prepare", async () => {
    let interrupted = false;
    const prepare = () => Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => void (interrupted = true))));
    const t = await testEngine({ testKinds: { overrides: { "test.files": { prepare } } } });
    const v = await t.submit("test.files", {});
    const t0 = Date.now();
    expect((await run(t.engine.cancel(v.id))).state).toBe("cancelled");
    expect(Date.now() - t0).toBeLessThan(5000);
    expect(interrupted).toBe(true);
  });
});

describe("M4, M5: job.done at least once, and to every listener", () => {
  test("a_throwing_sink_leaves_the_row_to_announce_again", async () => {
    const serverData = tmp();
    let throws = true;
    const first = await testEngine({
      serverData,
      wrapSink: (s) => ({
        ...s,
        jobDone: (e, job) => {
          if (throws) throw new Error("the journal is down");
          s.jobDone(e, job);
        },
      }),
    });
    const v = await first.runJob("test.echo", { value: 1 });
    expect(v.state).toBe("succeeded");
    expect(first.jobs.row(v.id)?.phase).toBe("announce");
    throws = false;
    await first.close();
    const second = await testEngine({ serverData });
    expect(second.sink.done).toEqual([expect.objectContaining({ kind: "test.echo", outcome: "succeeded" })]);
    expect(second.sink.doneJobs).toEqual([v.id]);
    expect(second.jobs.row(v.id)?.phase).toBe("done");
  });

  test("the_reannounce_reaches_listeners_with_its_stats", async () => {
    const serverData = tmp();
    const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    const store = new JobStore(control);
    const j = store.insert({ kind: "test.echo", kindVersion: 1, family: "test", params: { value: 1 }, owner: OWNER, priority: "normal", maxAttempts: 3 });
    store.transition(j.id, "running", { attempts: 1 });
    store.transition(j.id, "succeeded", { result: { value: 1 }, phase: "announce", stats: { runMicros: 1234, helpers: 2, isolated: false } });
    await control.close();
    const t = await testEngine({ serverData });
    const told: JobView[] = [];
    t.engine.onDone((job) => told.push(job));
    await until(() => told.length > 0);
    expect(told.map((x) => x.id)).toEqual([j.id]);
    expect(t.events.some((e) => e.type === "state" && e.job.id === j.id && e.job.state === "succeeded")).toBe(true);
    expect(t.sink.done).toEqual([expect.objectContaining({ outcome: "succeeded", runMicros: 1234, helpers: 2 })]);
    expect(t.jobs.row(j.id)?.phase).toBe("done");
  });
});

describe("L2–L5", () => {
  test("hourly_sweep_deletes_leftovers_only", async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    const live = await t.submit("test.sleep", { ms: 60_000 });
    await until(() => existsSync(join(t.serverData, "jobs", live.id)));
    mkdirSync(join(t.serverData, "jobs", "01LEFTOVER", "tmp"), { recursive: true });
    await clock.adjust(RETENTION_EVERY_MS);
    await until(() => !existsSync(join(t.serverData, "jobs", "01LEFTOVER")));
    expect(existsSync(join(t.serverData, "jobs", live.id))).toBe(true);
    await run(t.engine.cancel(live.id));
    await t.close();
    await clock.close();
  }, 20_000);

  test("a_helper_failure_reports_its_own_error", async () => {
    const fake = new FakeHost((host, proc, req) => {
      if (req.role === "helper") host.emit({ type: "message", proc, msg: { t: "error", job: "", error: { code: "CODEC_BROKEN", message: "no codec" } } });
    });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.helper", { helper: "ok" }, { maxAttempts: 1 });
    expect(v.error).toMatchObject({ code: "HELPER_FAILED", details: { helper: "test", code: "CODEC_BROKEN" } });
  });

  test("a_helper_hello_naming_a_family_is_refused", async () => {
    const fake = new FakeHost((host, proc, req) => {
      if (req.role !== "helper") return;
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "helper", family: "test", kind: "codec", id: "test", pid: 1 } as Hello });
    });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.helper", { helper: "ok" }, { maxAttempts: 1 });
    expect(v.error).toMatchObject({ code: "HELPER_FAILED", details: { code: "WORKER_PROTOCOL_MISMATCH" } });
  });

  test("start_helpers_lists_only_mapped_files", async () => {
    const reqs: JobSpawnRequest[] = [];
    const sent: Array<{ proc: number; msg: HostMsg }> = [];
    const helpers = [{ kind: "codec" as const, id: "test", profile: "worker" as const, files: ["helper.bin", "not-mapped.bin"] }];
    const t = await testEngine({ jobHost: recordingHost(reqs, sent), testKinds: { overrides: { "test.helper": { helpers } } } });
    expect((await t.runJob("test.helper", { helper: "ok" })).state).toBe("succeeded");
    const start = sent.find((s) => s.msg.t === "start")?.msg;
    expect(start?.t === "start" ? start.helpers : undefined).toEqual([{ id: "test", kind: "codec", files: ["helper.bin"] }]);
    const worker = reqs.find((r) => r.role === "worker");
    expect(worker?.grants?.some((g) => g.path === worker.cwd)).toBe(false);
  });

  test("a_second_close_waits_for_the_first", async () => {
    const t = await testEngine();
    const v = await t.submit("test.sleep", { ms: 60_000 });
    await until(() => t.engine.workers().processes.some((p) => p.job === v.id && p.pid !== undefined));
    const first = run(t.engine.close());
    await run(t.engine.close());
    expect(t.engine.workers().processes).toEqual([]);
    await first;
    expect((await failure(t.engine.submit(OWNER, { kind: "test.echo", params: { value: 1 } }))).code).toBe("WORKER_UNAVAILABLE");
  }, 20_000);
});
