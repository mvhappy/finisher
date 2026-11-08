// The fixes of the Task 9a review (B1, H1–H3, M1, M3, L1–L6 and the smaller items); each test
// fails without its fix.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { JobSpawnRequest } from "@finisher/core-types";
import type { MappedFile } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { openControlStore } from "../../src/control/db.ts";
import { checkNoLinks, createJobDir, isPrepared, jobDirs, markPrepared } from "../../src/jobs/files.ts";
import { MAX_ITEM_CHARS } from "../../src/jobs/runner.ts";
import { JobStore } from "../../src/jobs/store.ts";
import { FakeHost, recordingHost } from "../support/fake-host.ts";
import { closeTestEngines, failure, hostOf, OWNER, procOf, run, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

const win = process.platform === "win32";
const scratch: string[] = [];
afterEach(() => {
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});
function tmp(): string {
  const d = mkdtempSync(join(tmpdir(), "fin-review-"));
  scratch.push(d);
  return d;
}

const FILES: MappedFile[] = [
  { name: "in/data.txt", mode: "read" },
  { name: "out.bin", bytes: 4096, mode: "write" },
  { name: "out/", mode: "write" },
  { name: "sub/deep.bin", bytes: 64, mode: "write" },
];

describe("9a review", () => {
  // B1: a worker replaces `out/` and `sub/` with links to outside directories, then asks for a
  // retry. Without the check, attempt 2 emptied the first target and truncated a file in the second.
  test.skipIf(win)("planted_links_are_never_followed_on_retry", async () => {
    const victim = tmp();
    const victim2 = tmp();
    writeFileSync(join(victim, "keep.txt"), "precious");
    writeFileSync(join(victim2, "deep.bin"), "precious too");
    let root = "";
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.files": {
            // Called at submit and before each attempt; before attempt 2 it plays the worker of attempt 1.
            files: () => {
              const dirs = root === "" ? [] : readdirSync(root);
              const dir = dirs.length === 1 ? join(root, dirs[0] as string) : undefined;
              if (dir !== undefined && existsSync(join(dir, "out")) && !existsSync(join(dir, "planted"))) {
                rmSync(join(dir, "out"), { recursive: true });
                symlinkSync(victim, join(dir, "out"));
                rmSync(join(dir, "sub"), { recursive: true });
                symlinkSync(victim2, join(dir, "sub"));
                writeFileSync(join(dir, "planted"), "");
              }
              return FILES;
            },
          },
        },
      },
    });
    root = join(t.serverData, "jobs");
    const v = await t.runJob("test.files", { failOnAttempt: 1 }, { maxAttempts: 3 });
    expect(v.state).toBe("failed");
    expect(v.attempts).toBe(2);
    expect(v.error).toMatchObject({ code: "JOB_FAILED", details: { problem: "link" } });
    expect(readFileSync(join(victim, "keep.txt"), "utf8")).toBe("precious");
    expect(readFileSync(join(victim2, "deep.bin"), "utf8")).toBe("precious too");
  });

  test.skipIf(win)("check_no_links_lstats_every_segment", async () => {
    const dir = tmp();
    mkdirSync(join(dir, "a"));
    await checkNoLinks(dir, ["a/b/c.bin", "tmp/"]);
    symlinkSync(tmp(), join(dir, "a", "b"));
    const e = await checkNoLinks(dir, ["a/b/c.bin"]).catch((x: unknown) => x);
    expect(e).toMatchObject({ code: "JOB_FAILED", details: { file: "a/b/c.bin", problem: "link" } });
  });

  // H1: the job directory is read-only to the worker; only `tmp/` and the writable files are not.
  test("grants_follow_each_files_mode", async () => {
    const reqs: JobSpawnRequest[] = [];
    const t = await testEngine({ jobHost: recordingHost(reqs) });
    const v = await t.runJob("test.files", {});
    expect(v.state).toBe("succeeded");
    const dirs = jobDirs(join(t.serverData, "jobs"), v.id);
    expect(reqs[0]?.grants).toEqual([
      { path: dirs.job, mode: "read" },
      { path: dirs.tmp, mode: "readWrite" },
      { path: join(dirs.job, "in", "data.txt"), mode: "read" },
      { path: join(dirs.job, "out.bin"), mode: "write" },
      { path: join(dirs.job, "out"), mode: "write" },
    ]);
  });

  // H2: a terminal row whose job.done did not go out (death between the UPDATE and the sink) is announced at the next start, once.
  test("an_unannounced_end_is_announced_at_the_next_start", async () => {
    const serverData = tmp();
    const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    const store = new JobStore(control);
    const j = store.insert({ kind: "test.echo", kindVersion: 1, family: "test", params: { value: 1 }, owner: OWNER, priority: "normal", maxAttempts: 3 });
    store.transition(j.id, "running", { attempts: 1 });
    store.transition(j.id, "succeeded", { result: { value: 1 }, phase: "announce" });
    await control.close();
    const a = await testEngine({ serverData, keepData: true });
    expect(a.sink.done).toEqual([expect.objectContaining({ kind: "test.echo", outcome: "succeeded" })]);
    expect(a.jobs.row(j.id)?.phase).toBe("done");
    await a.close();
    const b = await testEngine({ serverData, keepData: true });
    expect(b.sink.done).toEqual([]);
    await b.close();
  });

  test("job_done_comes_before_the_directory_is_deleted", async () => {
    const t = await testEngine();
    let dirAtDone: boolean | undefined;
    t.engine.onDone((j) => {
      dirAtDone = existsSync(join(t.serverData, "jobs", j.id));
    });
    const v = await t.runJob("test.echo", { value: 1 });
    expect(t.sink.done).toHaveLength(1);
    expect(dirAtDone).toBe(true);
    await until(() => !existsSync(join(t.serverData, "jobs", v.id)));
  });

  // H3: a throw before the attempt's own try gives the slot back and fails the job INTERNAL.
  test("a_runner_throw_frees_the_slot_and_fails_internal", async () => {
    const t = await testEngine({
      cores: 2,
      testKinds: {
        overrides: {
          "test.sleep": {
            memory: () => {
              throw new Error("a broken memory rule");
            },
          },
        },
      },
    });
    const v = await t.runJob("test.sleep", { ms: 1 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("INTERNAL");
    expect(t.engine.workers().pools.find((p) => p.name === "cpu")?.used).toBe(0);
    expect((await t.runJob("test.echo", { value: 2 })).state).toBe("succeeded");
  });

  test("a_throwing_sink_still_settles_the_attempt", async () => {
    const t = await testEngine();
    t.sink.workerExit = () => {
      throw new Error("sink bug");
    };
    expect((await t.runJob("test.echo", { value: 1 })).state).toBe("succeeded");
  });

  // M1: an open host call has ended before the job's end is decided.
  test("open_calls_end_before_the_outcome", async () => {
    const order: string[] = [];
    const t = await testEngine({
      testKinds: {
        families: {
          test: {
            onCall: () =>
              Effect.never.pipe(
                Effect.onInterrupt(() =>
                  Effect.sync(() => order.push("interrupted")).pipe(
                    Effect.andThen(Effect.sleep("50 millis")),
                    Effect.andThen(Effect.sync(() => order.push("finalized"))),
                  ),
                ),
              ),
          },
        },
      },
    });
    t.engine.onDone(() => order.push("done"));
    const v = await t.submit("test.call", { method: "test.hold" });
    const proc = await procOf(t, v.id);
    await new Promise((r) => setTimeout(r, 200)); // the call is open
    hostOf(t.engine).killProcForTest?.(proc);
    expect((await t.wait(v.id)).state).toBe("failed");
    expect(order).toEqual(["interrupted", "finalized", "done"]);
  });

  // M3: `prepare` runs until it succeeded once for a directory; the directory's existence says nothing.
  test("prepare_marker_not_the_directory", async () => {
    const root = tmp();
    const dirs = jobDirs(root, "01JOB");
    expect(await createJobDir(dirs)).toBe(true);
    expect(await isPrepared(dirs)).toBe(false); // a failed prepare leaves no mark
    expect(await createJobDir(dirs)).toBe(false);
    expect(await isPrepared(dirs)).toBe(false); // so the next attempt prepares again
    await markPrepared(dirs);
    expect(await isPrepared(dirs)).toBe(true);
  });

  test("prepare_runs_once_across_retries", async () => {
    let calls = 0;
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.files": {
            prepare: (ctx) =>
              Effect.promise(async () => {
                calls++;
                const { mkdir, writeFile } = await import("node:fs/promises");
                await mkdir(ctx.path("in"), { recursive: true });
                await writeFile(ctx.path("in/data.txt"), "x", { mode: 0o600 });
                return { extra: { call: calls } };
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.files", { failOnAttempt: 1 }, { maxAttempts: 2 });
    expect(v.state).toBe("succeeded");
    expect(v.attempts).toBe(2);
    expect(calls).toBe(1);
    // L2: the success clears the first attempt's error.
    expect(v.error).toBeUndefined();
  });

  // L1
  test("max_attempts_is_an_integer_from_1_to_10", async () => {
    const t = await testEngine();
    for (const maxAttempts of [0, 11, 1.5, -1]) {
      const e = await failure(t.engine.submit(OWNER, { kind: "test.echo", params: { value: 1 }, maxAttempts }));
      expect(e.code).toBe("JOB_PARAMS_INVALID");
    }
    expect((await t.runJob("test.echo", { value: 1 }, { maxAttempts: 10 })).maxAttempts).toBe(10);
    expect(t.jobs.list().jobs).toHaveLength(1);
  });

  // L3: the unfinished count is kept in memory and follows every transition.
  test("the_queue_limit_follows_ends", async () => {
    const t = await testEngine({ queueLimit: 2 });
    const a = await t.submit("test.sleep", { ms: 300 });
    await t.submit("test.sleep", { ms: 60_000 });
    expect((await failure(t.engine.submit(OWNER, { kind: "test.echo", params: { value: 1 } }))).code).toBe("JOB_QUEUE_FULL");
    await t.wait(a.id);
    expect((await t.submit("test.echo", { value: 1 })).id).toBeDefined();
  });

  // L5: the worker's hello must name the family that was spawned.
  test("a_hello_of_another_family_is_a_mismatch", async () => {
    const fake = new FakeHost((host, proc) => {
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "other", kinds: [{ kind: "test.echo", version: 1 }], warm: false, pid: 1 } });
    });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.echo", { value: 1 });
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "WORKER_PROTOCOL_MISMATCH", details: { expected: { family: "test" }, got: { family: "other" } } });
    expect(fake.sent.some((s) => s.msg.t === "start")).toBe(false);
  });

  // Item names are bounded.
  test("an_overlong_item_is_a_protocol_error", async () => {
    const fake = new FakeHost((host, proc) => {
      host.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.crash", version: 1 }], warm: false, pid: 1 } });
      setImmediate(() => host.emit({ type: "message", proc, msg: { t: "begin", job: "x", item: "i".repeat(MAX_ITEM_CHARS + 1) } }));
    });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.crash", { items: ["a"], how: "exit" });
    expect(v.error).toMatchObject({ code: "WORKER_CRASHED", details: { protocol: "itemTooLong" } });
  });
});
