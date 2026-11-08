// The napi `JobHost` (5C-1 plan, Task 6) over the test-hooks `fin-workers` in test/native/.
import { spawn as spawnChild } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { monitorEventLoopDelay } from "node:perf_hooks";
import { afterEach, describe, expect, test } from "vitest";
import type { HostMsg } from "@finisher/core-types";
import {
  JobHost,
  type JobHostEvent,
  type JobHostOptions,
  type JobSpawnRequest,
  jobKindVersions,
  resolveMemoryRule,
  validateJobParams,
  workersBinaryPath,
} from "../index.js";
import { workersBinaryPath as pathIn } from "../loader.js";

const BIN_DIR = dirname(workersBinaryPath());
const WAIT_MS = 15_000;
const win = process.platform === "win32";

/** `at`: when the JS thread got it; `handed`: when Rust handed it over (test builds stamp it). */
type Seen = { at: number; handed: number; e: JobHostEvent };

interface StartOpts {
  /** Sees every event first, as it arrives. */
  onEach?: (e: JobHostEvent) => void;
  /** Keeps only the first 32 characters of each stderr line (floods). */
  slim?: boolean;
}

interface TestHost {
  host: JobHost;
  seen: Seen[];
  dir: string;
  waitFor(pred: (e: JobHostEvent) => boolean, ms?: number): Promise<JobHostEvent>;
  of(proc: number): JobHostEvent[];
}

const hosts: TestHost[] = [];

afterEach(async () => {
  for (const h of hosts.splice(0)) {
    await h.host.shutdown(0);
    rmSync(h.dir, { recursive: true, force: true });
  }
});

/** Blocks the JS thread for `ms` (no timer, no event, no microtask runs). */
function block(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/** `FIN_STRICT_TIMING=1`: the absolute timing bounds too (a quiet machine). */
const strict = process.env.FIN_STRICT_TIMING === "1";

async function start(opts: Partial<JobHostOptions> = {}, so: StartOpts = {}): Promise<TestHost> {
  const dir = mkdtempSync(join(tmpdir(), "fin-jobhost-"));
  mkdirSync(join(dir, "tmp"));
  const seen: Seen[] = [];
  const waiters = new Set<(e: JobHostEvent) => void>();
  const host = await JobHost.start(
    // cgroups off: the delegation notice would repeat for every host (the caps test keeps `auto`).
    { binaryDir: BIN_DIR, timing: { killWaitMs: 3000 }, cgroups: "off", ...opts },
    (e) => {
      const at = performance.now();
      const stamped = e as JobHostEvent & { handedAtMsForTest?: number };
      const handed = stamped.handedAtMsForTest ?? Number.NaN;
      delete stamped.handedAtMsForTest;
      if (so.slim && e.type === "stderr") e.line = e.line.slice(0, 32);
      so.onEach?.(e);
      seen.push({ at, handed, e });
      for (const w of waiters) w(e);
    },
  );
  return track({
    host,
    seen,
    dir,
    waitFor(pred, ms = WAIT_MS) {
      return new Promise((resolve, reject) => {
        const hit = seen.find((s) => pred(s.e));
        if (hit !== undefined) return resolve(hit.e);
        const check = (e: JobHostEvent) => {
          if (!pred(e)) return;
          waiters.delete(check);
          clearTimeout(timer);
          resolve(e);
        };
        const timer = setTimeout(() => {
          waiters.delete(check);
          reject(new Error(`no matching event within ${ms} ms; seen ${JSON.stringify(seen.slice(-50).map((s) => s.e)).slice(0, 2000)}`));
        }, ms);
        waiters.add(check);
      });
    },
    of: (proc) => seen.map((s) => s.e).filter((e) => "proc" in e && e.proc === proc),
  });
}

function track(t: TestHost): TestHost {
  hosts.push(t);
  return t;
}

function env(dir: string): Record<string, string> {
  const e: Record<string, string> = { TMPDIR: join(dir, "tmp"), TEMP: join(dir, "tmp"), TMP: join(dir, "tmp") };
  if (win && process.env.SystemRoot) e.SystemRoot = process.env.SystemRoot;
  return e;
}

function worker(t: TestHost, args: string[] = ["test"]): JobSpawnRequest {
  return { role: "worker", family: "test", program: { bundled: "fin-workers" }, args, env: env(t.dir), cwd: t.dir, label: "test" };
}

function startMsg(t: TestHost, job: string, kind: string, params: unknown): HostMsg {
  return {
    t: "start",
    job: { id: job, kind, kindVersion: 1, params, attempt: 1 },
    limits: { aliveMs: 100 },
    files: [],
    helpers: [],
    dirs: { job: t.dir, tmp: join(t.dir, "tmp") },
    diagnostics: false,
  };
}

/** A `test` worker in a new group, once its `hello` arrived. */
async function ready(t: TestHost): Promise<{ group: number; proc: number }> {
  const group = t.host.openGroup({ label: "test", qos: "normal" });
  const proc = await t.host.spawn(group, worker(t));
  await t.waitFor((e) => e.type === "hello" && e.proc === proc);
  return { group, proc };
}

const isResult = (proc: number) => (e: JobHostEvent) => e.type === "message" && e.proc === proc && e.msg.t === "result";
const isExit = (proc: number) => (e: JobHostEvent) => e.type === "exit" && e.proc === proc;

function exitOf(t: TestHost, proc: number) {
  const e = t.of(proc).find((x) => x.type === "exit");
  if (e?.type !== "exit") throw new Error(`no exit for ${proc}`);
  return e.info;
}

function progressOf(t: TestHost, proc: number): Seen[] {
  return t.seen.filter((s) => s.e.type === "message" && s.e.proc === proc && s.e.msg.t === "progress");
}

describe("JobHost", () => {
  test("starts_and_reports_caps", async () => {
    const t = await start({ cgroups: "auto" });
    const caps = t.host.caps();
    expect(caps).toMatchObject({ sandbox: "none", sandboxed: false });
    if (win) expect(caps).toMatchObject({ killTree: "jobObject", leafLimits: true, cgroupMemory: true, watchdog: false });
    else expect(caps.watchdog).toBe(true);
    if (process.platform === "darwin") expect(caps.killTree).toBe("processGroup");
    if (process.platform === "linux") expect(["cgroup", "processGroup"]).toContain(caps.killTree);
  });

  test("spawns_test_echo_through_napi", { timeout: 30_000 }, async () => {
    const t = await start();
    const { group, proc } = await ready(t);
    const [spawned, hello] = t.of(proc);
    expect(spawned).toEqual({ type: "spawned", proc, group });
    expect(hello).toMatchObject({ type: "hello", hello: { v: 1, role: "worker", family: "test" } });
    expect(t.host.send(proc, startMsg(t, "j1", "test.echo", { value: { a: 1 } }))).toBe("ok");
    const result = await t.waitFor(isResult(proc));
    expect(result).toMatchObject({ msg: { t: "result", job: "j1", value: { value: { a: 1 } } } });
    await t.waitFor((e) => e.type === "groupEmpty" && e.group === group);
    expect(exitOf(t, proc)).toMatchObject({ reason: "exited", code: 0 });
    // Gone: nothing to enqueue to (an unknown id too).
    expect(t.host.send(proc, { t: "shutdown" })).toBe("gone");
    expect(t.host.send(proc + 1000, { t: "shutdown" })).toBe("gone");
  });

  test("send_answers_full_for_a_process_not_reading", { timeout: 30_000 }, async () => {
    const t = await start({ timing: { killWaitMs: 3000, hungMs: 60_000 } });
    const { proc } = await ready(t);
    t.host.send(proc, startMsg(t, "j1", "test.hang", {}));
    // Let it stop reading its stdin first.
    await new Promise((r) => setTimeout(r, 300));
    const big: HostMsg = { t: "reply", id: 1, value: "x".repeat(256 * 1024) };
    const answers: string[] = [];
    for (let i = 0; i < 200 && answers.at(-1) !== "full"; i++) answers.push(t.host.send(proc, big));
    expect(answers.at(-1)).toBe("full");
    expect(answers.slice(0, -1).every((a) => a === "ok")).toBe(true);
    // The cause comes with the exit: the process is killed as hung.
    await t.waitFor(isExit(proc));
    expect(exitOf(t, proc)).toMatchObject({ reason: "hung" });
  });

  test("events_arrive_in_order_per_process", { timeout: 30_000 }, async () => {
    const t = await start();
    const a = await ready(t);
    const b = await ready(t);
    for (const p of [a, b]) t.host.send(p.proc, startMsg(t, `j${p.proc}`, "test.sleep", { ms: 400, progressEveryMs: 10 }));
    for (const p of [a, b]) await t.waitFor((e) => e.type === "groupEmpty" && e.group === p.group);
    for (const p of [a, b]) {
      const types = t.of(p.proc).filter((e) => e.type !== "stderr").map((e) => (e.type === "message" ? e.msg.t : e.type));
      expect(types.slice(0, 2)).toEqual(["spawned", "hello"]);
      expect(types.at(-1)).toBe("exit");
      expect(types.at(-2)).toBe("result");
      expect(types.slice(2, -2).every((x) => x === "progress")).toBe(true);
      const done = progressOf(t, p.proc).map((s) => (s.e.type === "message" && s.e.msg.t === "progress" ? s.e.msg.done : -1));
      expect(done).toEqual([...done].sort((x, y) => x - y));
      const empty = t.seen.findIndex((s) => s.e.type === "groupEmpty" && s.e.group === p.group);
      expect(t.seen.findIndex((s) => s.e.type === "exit" && s.e.proc === p.proc)).toBeLessThan(empty);
    }
  });

  test("no_event_precedes_its_spawn", { timeout: 60_000 }, async () => {
    // Workers that crash at once (an unknown argument), each spawned while the JS thread is
    // busy, so the answer, `spawned`, `stderr` and `exit` all wait in the queue together.
    const known = new Set<number>();
    const early: JobHostEvent[] = [];
    const t = await start({}, { onEach: (e) => ("proc" in e && !known.has(e.proc) ? early.push(e) : 0) });
    const procs: number[] = [];
    for (let i = 0; i < 20; i++) {
      const g = t.host.openGroup({ label: "crash", qos: "normal" });
      const spawning = t.host.spawn(g, worker(t, ["--bogus"]));
      block(100);
      const p = await spawning;
      known.add(p);
      procs.push(p);
    }
    for (const p of procs) await t.waitFor(isExit(p));
    expect(early).toEqual([]);
    for (const p of procs) {
      expect(t.of(p)[0]).toMatchObject({ type: "spawned", proc: p });
      expect(exitOf(t, p)).toMatchObject({ reason: "exited" });
      expect(exitOf(t, p).code ?? 0).not.toBe(0);
    }
  });

  test("spawn_and_kill_are_async", { timeout: 60_000 }, async () => {
    const t = await start();
    const lag = monitorEventLoopDelay({ resolution: 1 });
    lag.enable();
    const groups = Array.from({ length: 20 }, () => t.host.openGroup({ label: "lag", qos: "normal" }));
    const procs = await Promise.all(groups.map((g) => t.host.spawn(g, worker(t))));
    for (const p of procs) await t.waitFor((e) => e.type === "hello" && e.proc === p);
    for (const p of procs) t.host.send(p, startMsg(t, `j${p}`, "test.sleep", { ms: 60_000 }));
    await Promise.all(groups.map((g) => t.host.killGroup(g, "cancel")));
    for (const p of procs) await t.waitFor(isExit(p));
    lag.disable();
    for (const p of procs) expect(exitOf(t, p)).toMatchObject({ reason: "killed", kill: "cancel" });
    // The event loop kept turning: its p99 delay (1 ms resolution included) stays small.
    expect(lag.count).toBeGreaterThan(50);
    expect(lag.percentile(99) / 1e6).toBeLessThan(strict ? 6 : 25);
  });

  for (const [name, opts, lineBytes] of [
    ["count", { eventQueue: 8 }, 1000],
    ["bytes", { eventQueueBytes: 32 * 1024 }, 4000],
  ] as const) {
    test(`blocked_js_backpressures_the_worker_by_${name}`, { timeout: 120_000 }, async () => {
      const t = await start(opts, { slim: true });
      const { proc } = await ready(t);
      const n = 100_000;
      t.host.send(proc, startMsg(t, "flood", "test.progressFlood", { n, stderrBytes: lineBytes }));
      await t.waitFor((e) => e.type === "stderr" && e.proc === proc);
      const rss0 = process.memoryUsage().rss;
      // What JS took before it blocked was handed over before.
      const taken = t.seen.length;
      const from = Date.now();
      block(2000);
      const to = Date.now();
      // Before the JS thread takes anything: what waits for it is bounded.
      const grown = process.memoryUsage().rss - rss0;
      await t.waitFor(isResult(proc), 100_000);
      const lines = t.seen.filter((s) => s.e.type === "stderr" && s.e.proc === proc);
      const counter = (s: Seen) => (s.e.type === "stderr" ? Number(s.e.line.split(" ")[1]) : Number.NaN);
      // Nothing is taken while JS is blocked, so at most the bound is handed over meanwhile
      // (2 ms off each end: `Date.now` has whole milliseconds).
      const during = lines.filter((s) => s.handed > from + 2 && s.handed < to - 2).length;
      const bound = "eventQueue" in opts ? opts.eventQueue : Math.ceil(opts.eventQueueBytes / lineBytes) + 1;
      expect(during).toBeLessThanOrEqual(bound);
      // The worker's own counter: it was blocked on its pipe, not running through the flood.
      const before = Math.max(...t.seen.slice(0, taken).filter((s) => s.e.type === "stderr").map(counter));
      const after = lines.find((s) => s.handed > to);
      expect(after).toBeDefined();
      expect(counter(after!) - before).toBeLessThan(n / 10);
      expect(lines.map(counter)).toEqual(Array.from({ length: n }, (_, i) => i + 1));
      expect(grown).toBeLessThan(64 * 1024 * 1024);
    });
  }

  test("slow_js_backpressures_the_worker", { timeout: 60_000 }, async () => {
    const t = await start();
    const n = 400_000;
    const free = await ready(t);
    const t0 = performance.now();
    t.host.send(free.proc, startMsg(t, "free", "test.progressFlood", { n }));
    await t.waitFor(isResult(free.proc), 30_000);
    const freeMs = performance.now() - t0;
    // Unpaused, the flood runs well inside the 2 s pause: on a quiet machine under 1.5 s; the
    // counter check below needs only that 2 s would have covered a fifth of it.
    if (strict) expect(freeMs).toBeLessThan(1500);
    expect(freeMs).toBeLessThan(10_000);

    const { proc } = await ready(t);
    const rss0 = process.memoryUsage().rss;
    t.host.pauseEventsForTest!(2000);
    const t1 = performance.now();
    t.host.send(proc, startMsg(t, "slow", "test.progressFlood", { n }));
    await t.waitFor(isResult(proc), 30_000);
    expect(performance.now() - t1).toBeGreaterThan(1900);
    const ps = progressOf(t, proc).map((s) => (s.e.type === "message" && s.e.msg.t === "progress" ? s.e.msg.done : 0));
    // The worker's own counter: what it had sent when the pause ended (the first value handed
    // over after it) is a pipe's worth, not the flood: its writes blocked.
    expect(ps.length).toBeGreaterThanOrEqual(2);
    expect(ps[1]!).toBeLessThan(n / 20);
    expect(ps.at(-1)).toBe(n);
    expect(process.memoryUsage().rss - rss0).toBeLessThan(64 * 1024 * 1024);
  });

  test("slow_js_is_not_hung", { timeout: 30_000 }, async () => {
    const t = await start({ timing: { hungMs: 300, killWaitMs: 3000 } });
    const { proc } = await ready(t);
    t.host.pauseEventsForTest!(2000);
    t.host.send(proc, startMsg(t, "j1", "test.sleep", { ms: 1000, progressEveryMs: 20 }));
    await t.waitFor(isExit(proc));
    expect(t.of(proc).some((e) => e.type === "message" && e.msg.t === "result")).toBe(true);
    expect(exitOf(t, proc)).toMatchObject({ reason: "exited", code: 0 });
  });

  test("progress_is_coalesced", { timeout: 30_000 }, async () => {
    const t = await start();
    const { proc } = await ready(t);
    // Spread the flood over ~1.5 s of 2 ms sleeps' worth of progress too.
    t.host.send(proc, startMsg(t, "j1", "test.sleep", { ms: 1500, progressEveryMs: 2 }));
    await t.waitFor(isResult(proc));
    const sleepers = progressOf(t, proc);
    const n = 100_000;
    const flood = await ready(t);
    t.host.send(flood.proc, startMsg(t, "j2", "test.progressFlood", { n }));
    await t.waitFor(isResult(flood.proc));
    for (const [ps, last] of [
      [sleepers, undefined],
      [progressOf(t, flood.proc), n],
    ] as const) {
      // Timed when Rust handed each over, so the JS thread's own jitter does not count.
      const spanS = (ps.at(-1)!.handed - ps[0]!.handed) / 1000;
      expect(ps.length).toBeLessThanOrEqual(4 * Math.ceil(spanS) + 2);
      // Every gap but the last (the pending value flushed before `result`) is ≥ 250 ms.
      for (let i = 1; i < ps.length - 1; i++) expect(ps[i]!.handed - ps[i - 1]!.handed).toBeGreaterThanOrEqual(249);
      if (last !== undefined) {
        const e = ps.at(-1)!.e;
        expect(e.type === "message" && e.msg.t === "progress" && e.msg.done).toBe(last);
      }
    }
    const order = t.of(flood.proc).map((e) => (e.type === "message" ? e.msg.t : e.type));
    expect(order.lastIndexOf("progress")).toBeLessThan(order.indexOf("result"));
  });

  test("kill_group_from_js", { timeout: 30_000 }, async () => {
    const t = await start();
    const { group, proc } = await ready(t);
    t.host.send(proc, startMsg(t, "j1", "test.sleep", { ms: 60_000 }));
    await t.host.killGroup(group, "cancel");
    await t.waitFor(isExit(proc));
    expect(exitOf(t, proc)).toMatchObject(win ? { reason: "killed", kill: "cancel" } : { reason: "killed", kill: "cancel", signal: "kill" });
    // A closed or unknown group is the caller's mistake, not a retryable one.
    t.host.closeGroup(group);
    await expect(t.host.spawn(group, worker(t))).rejects.toMatchObject({ code: "JOBHOST_STATE", details: { reason: "groupClosed" } });
    await expect(t.host.spawn(group + 1000, worker(t))).rejects.toMatchObject({ code: "JOBHOST_STATE", details: { reason: "unknownGroup" } });
  });

  test("test_hooks_kill_one_and_abandon", { timeout: 30_000 }, async () => {
    const t = await start({ timing: { killWaitMs: 300 } });
    const a = await ready(t);
    t.host.killProcForTest!(a.proc);
    await t.waitFor(isExit(a.proc));
    expect(exitOf(t, a.proc)).toMatchObject(win ? { reason: "exited", code: 137 } : { reason: "exited", signal: "kill" });
    const b = await ready(t);
    t.host.abandonForTest!(b.proc, 1500);
    await t.host.killGroup(b.group, "cancel");
    await t.waitFor(isExit(b.proc));
    expect(exitOf(t, b.proc)).toMatchObject({ reason: "abandoned" });
  });

  test("lost_answers_reject_internal", { timeout: 30_000 }, async () => {
    const base = { binaryDir: BIN_DIR, cgroups: "off" } as const;
    const unanswered = { code: "INTERNAL", details: { reason: "unanswered" } };
    // The start thread does not start; `Host::start` panics.
    for (const failForTest of ["startThread", "hostStart"]) {
      await expect(JobHost.start({ ...base, failForTest } as never, () => {})).rejects.toMatchObject(unanswered);
    }
    // Each spawn command panics before it answers; the spawn thread lives on.
    const t = await start({ failForTest: "spawn" } as never);
    const g = t.host.openGroup({ label: "lost", qos: "normal" });
    await expect(t.host.spawn(g, worker(t))).rejects.toMatchObject(unanswered);
    await expect(t.host.spawn(g, worker(t))).rejects.toMatchObject(unanswered);
    await t.host.killGroup(g, "cancel");
  });

  test("lost_answers_do_not_keep_node_alive", { timeout: 60_000 }, async () => {
    const index = pathToFileURL(join(import.meta.dirname, "..", "index.js")).href;
    const script = `
      import { JobHost } from ${JSON.stringify(index)};
      const base = { binaryDir: ${JSON.stringify(BIN_DIR)}, cgroups: "off" };
      for (const failForTest of ["startThread", "hostStart"]) await JobHost.start({ ...base, failForTest }, () => {}).catch((e) => console.log(e.code));
      const h = await JobHost.start({ ...base, failForTest: "spawn" }, () => {});
      const req = { role: "worker", family: "test", program: { bundled: "fin-workers" }, args: ["test"], cwd: ${JSON.stringify(tmpdir())} };
      await h.spawn(h.openGroup({ label: "x", qos: "normal" }), req).catch((e) => console.log(e.code));
    `;
    const child = spawnChild(process.execPath, ["--input-type=module", "-e", script], { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", () => {});
    const code = await new Promise<number | null>((resolve) => {
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
        resolve(null);
      }, 30_000);
      child.on("exit", (c) => {
        clearTimeout(timer);
        resolve(c);
      });
    });
    expect(out.trim().split(/\s+/)).toEqual(["INTERNAL", "INTERNAL", "INTERNAL"]);
    // Exits by itself: no settled-never promise holds the event loop.
    expect(code).toBe(0);
  });

  test("bad_arguments_are_invalid_request", async () => {
    const t = await start();
    const g = t.host.openGroup({ label: "bad", qos: "normal" });
    await expect(t.host.spawn(g, { ...worker(t), role: "helper" })).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "req.helper" } });
    await expect(t.host.spawn(g, { ...worker(t), nope: 1 } as never)).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "req" } });
    expect(() => t.host.send(1, { t: "bogus" } as never)).toThrow(expect.objectContaining({ code: "INVALID_REQUEST" }));
    expect(() => t.host.openGroup({ label: "x", qos: "fast" } as never)).toThrow(expect.objectContaining({ code: "INVALID_REQUEST" }));
    await expect(t.host.killGroup(g, "nope" as never)).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "reason" } });
    await expect(t.host.spawn(-1, worker(t))).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "group" } });
    await expect(t.host.spawn(g, { ...worker(t), program: { bundled: "sh" } })).rejects.toMatchObject({ code: "WORKER_UNAVAILABLE", details: { reason: "program" } });
    // Like `spawn`, a bad `shutdown` argument rejects rather than throws.
    await expect(t.host.shutdown(-1)).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "graceMs" } });
    await expect(t.host.shutdown(1.5)).rejects.toMatchObject({ code: "INVALID_REQUEST", details: { field: "graceMs" } });
    await expect(t.host.shutdown("x" as never)).rejects.toThrow();
    await expect(t.host.killGroup("x" as never, "cancel")).rejects.toThrow();
  });

  test("node_child_process_exits_are_not_stolen", { timeout: 60_000 }, async () => {
    const t = await start();
    const jobs = await Promise.all(Array.from({ length: 20 }, () => ready(t)));
    for (const j of jobs) t.host.send(j.proc, startMsg(t, `j${j.proc}`, "test.sleep", { ms: 300 }));
    const children = Array.from(
      { length: 100 },
      (_, i) =>
        new Promise<{ want: number; got: number | null }>((resolve, reject) => {
          const want = i % 50;
          const c = win ? spawnChild("cmd.exe", ["/d", "/c", `exit ${want}`]) : spawnChild("/bin/sh", ["-c", `exit ${want}`]);
          c.on("error", reject);
          c.on("exit", (got) => resolve({ want, got }));
        }),
    );
    for (const r of await Promise.all(children)) expect(r.got).toBe(r.want);
    for (const j of jobs) await t.waitFor(isExit(j.proc));
    for (const j of jobs) {
      expect(t.of(j.proc).some((e) => e.type === "message" && e.msg.t === "result")).toBe(true);
      expect(exitOf(t, j.proc)).toMatchObject({ reason: "exited", code: 0 });
    }
  });

  test("shutdown_resolves_after_all_exits", { timeout: 30_000 }, async () => {
    const t = await start();
    const ps = await Promise.all([ready(t), ready(t), ready(t)]);
    for (const p of ps.slice(0, 2)) t.host.send(p.proc, startMsg(t, `j${p.proc}`, "test.sleep", { ms: 60_000 }));
    await t.host.shutdown(200);
    // The exits were handed over before the promise settled; they travel on another queue.
    for (const p of ps) await t.waitFor(isExit(p.proc), 1000);
    for (const p of ps) expect(exitOf(t, p.proc)).toMatchObject({ reason: "killed", kill: "shutdown" });
    await expect(t.host.spawn(t.host.openGroup({ label: "late", qos: "normal" }), worker(t))).rejects.toMatchObject({
      code: "JOBHOST_STATE",
      details: { reason: "shutDown" },
    });
  });
});

describe("params, rules and paths", () => {
  test("validate_params_accepts_and_rejects", () => {
    expect(validateJobParams("test.sleep", 1, { ms: 5 })).toEqual({ ok: true, params: { ms: 5 } });
    const wrongType = validateJobParams("test.sleep", 1, { ms: "x" });
    expect(wrongType.ok).toBe(false);
    expect(!wrongType.ok && wrongType.errors.join(" ")).toMatch(/ms/);
    const unknownField = validateJobParams("test.sleep", 1, { ms: 1, bogus: 1 });
    expect(!unknownField.ok && unknownField.errors.join(" ")).toMatch(/bogus/);
    const missing = validateJobParams("test.sleep", 1, {});
    expect(!missing.ok && missing.errors.join(" ")).toMatch(/ms/);
    expect(validateJobParams("test.nope", 1, {})).toMatchObject({ ok: false });
    expect(validateJobParams("test.sleep", 2, { ms: 5 })).toMatchObject({ ok: false });
    expect(jobKindVersions()).toContainEqual({ kind: "test.sleep", version: 1 });
  });

  test("validate_params_refuses_bad_versions_and_huge_params", () => {
    for (const version of [1.5, -1, Number.NaN, 2 ** 32]) {
      expect(() => validateJobParams("test.sleep", version, { ms: 5 }), String(version)).toThrow(
        expect.objectContaining({ code: "INVALID_REQUEST", details: expect.objectContaining({ field: "version" }) }),
      );
    }
    // Under the cap it passes; over it the job could never be started (a 1 MiB line).
    expect(validateJobParams("test.echo", 1, { value: "x".repeat(500 * 1024) })).toMatchObject({ ok: true });
    const huge = validateJobParams("test.echo", 1, { value: "x".repeat(600 * 1024) });
    expect(huge.ok).toBe(false);
    expect(!huge.ok && huge.errors.join(" ")).toMatch(/524288/);
  });

  test("resolve_memory_rule_matches_rust", () => {
    const gib = 1024 ** 3;
    expect(resolveMemoryRule({ fixedMiB: 512 }, 8 * gib)).toBe(512 * 1024 ** 2);
    expect(resolveMemoryRule({ ramFraction: 0.25, capMiB: 1024 }, 8 * gib)).toBe(1024 * 1024 ** 2);
    expect(resolveMemoryRule({ ramFraction: 0.25 }, 2 * gib)).toBe(gib / 2);
    expect(resolveMemoryRule({}, 8 * gib)).toBeUndefined();
  });

  test("workers_binary_path_finds_the_test_binary", () => {
    const p = workersBinaryPath();
    expect(existsSync(p)).toBe(true);
    expect(p.replaceAll("\\", "/")).toContain("test/native/fin-workers");
    expect(pathIn("/x/", "linux")).toBe("/x/fin-workers");
    expect(pathIn("C:\\x\\", "win32")).toBe("C:\\x\\fin-workers.exe");
    expect(workersBinaryPath("/y")).toBe(win ? "/y\\fin-workers.exe" : "/y/fin-workers");
  });
});
