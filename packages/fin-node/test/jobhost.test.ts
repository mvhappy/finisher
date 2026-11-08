// The napi `JobHost` (5C-1 plan, Task 6) over the test-hooks `fin-workers` in test/native/.
import { spawn as spawnChild } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
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

type Seen = { at: number; e: JobHostEvent };

interface TestHost {
  host: JobHost;
  seen: Seen[];
  dir: string;
  /** The longest our event callback took (a JS turn this binding causes). */
  longestCallbackMs: number;
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

async function start(opts: Partial<JobHostOptions> = {}): Promise<TestHost> {
  const dir = mkdtempSync(join(tmpdir(), "fin-jobhost-"));
  mkdirSync(join(dir, "tmp"));
  const seen: Seen[] = [];
  const t: Partial<TestHost> = { seen, dir, longestCallbackMs: 0 };
  const waiters = new Set<() => void>();
  const host = await JobHost.start(
    // cgroups off: the delegation notice would repeat for every host (the caps test keeps `auto`).
    { binaryDir: BIN_DIR, timing: { killWaitMs: 3000 }, cgroups: "off", ...opts },
    (e) => {
      const t0 = performance.now();
      seen.push({ at: t0, e });
      for (const w of waiters) w();
      t.longestCallbackMs = Math.max(t.longestCallbackMs!, performance.now() - t0);
    },
  );
  const th: TestHost = {
    ...(t as TestHost),
    host,
    waitFor(pred, ms = WAIT_MS) {
      return new Promise((resolve, reject) => {
        const check = () => {
          const hit = seen.find((s) => pred(s.e));
          if (hit === undefined) return false;
          waiters.delete(check);
          clearTimeout(timer);
          resolve(hit.e);
          return true;
        };
        const timer = setTimeout(() => {
          waiters.delete(check);
          reject(new Error(`no matching event within ${ms} ms; seen ${JSON.stringify(seen.map((s) => s.e)).slice(0, 2000)}`));
        }, ms);
        if (!check()) waiters.add(check);
      });
    },
    of: (proc) => seen.map((s) => s.e).filter((e) => "proc" in e && e.proc === proc),
  };
  Object.defineProperty(th, "longestCallbackMs", { get: () => t.longestCallbackMs! });
  hosts.push(th);
  return th;
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
    const hello = t.of(proc)[0];
    expect(hello).toMatchObject({ type: "hello", hello: { v: 1, role: "worker", family: "test" } });
    expect(t.host.send(proc, startMsg(t, "j1", "test.echo", { value: { a: 1 } }))).toBe(true);
    const result = await t.waitFor(isResult(proc));
    expect(result).toMatchObject({ msg: { t: "result", job: "j1", value: { value: { a: 1 } } } });
    await t.waitFor((e) => e.type === "groupEmpty" && e.group === group);
    expect(exitOf(t, proc)).toMatchObject({ reason: "exited", code: 0 });
    // Gone: nothing to enqueue to.
    expect(t.host.send(proc, { t: "shutdown" })).toBe(false);
  });

  test("events_arrive_in_order_per_process", { timeout: 30_000 }, async () => {
    const t = await start();
    const a = await ready(t);
    const b = await ready(t);
    for (const p of [a, b]) t.host.send(p.proc, startMsg(t, `j${p.proc}`, "test.sleep", { ms: 400, progressEveryMs: 10 }));
    for (const p of [a, b]) await t.waitFor((e) => e.type === "groupEmpty" && e.group === p.group);
    for (const p of [a, b]) {
      const types = t.of(p.proc).filter((e) => e.type !== "stderr").map((e) => (e.type === "message" ? e.msg.t : e.type));
      expect(types[0]).toBe("hello");
      expect(types.at(-1)).toBe("exit");
      expect(types.at(-2)).toBe("result");
      expect(types.slice(1, -2).every((x) => x === "progress")).toBe(true);
      const done = progressOf(t, p.proc).map((s) => (s.e.type === "message" && s.e.msg.t === "progress" ? s.e.msg.done : -1));
      expect(done).toEqual([...done].sort((x, y) => x - y));
      const empty = t.seen.findIndex((s) => s.e.type === "groupEmpty" && s.e.group === p.group);
      expect(t.seen.findIndex((s) => s.e.type === "exit" && s.e.proc === p.proc)).toBeLessThan(empty);
    }
  });

  test("spawn_and_kill_are_async", { timeout: 60_000 }, async () => {
    const t = await start();
    let longestCall = 0;
    const timed = <T>(f: () => T): T => {
      const t0 = performance.now();
      const r = f();
      longestCall = Math.max(longestCall, performance.now() - t0);
      return r;
    };
    const groups = Array.from({ length: 20 }, () => timed(() => t.host.openGroup({ label: "lag", qos: "normal" })));
    const procs = await Promise.all(groups.map((g) => timed(() => t.host.spawn(g, worker(t)))));
    for (const p of procs) await t.waitFor((e) => e.type === "hello" && e.proc === p);
    for (const p of procs) timed(() => t.host.send(p, startMsg(t, `j${p}`, "test.sleep", { ms: 60_000 })));
    await Promise.all(groups.map((g) => timed(() => t.host.killGroup(g, "cancel"))));
    for (const p of procs) await t.waitFor(isExit(p));
    for (const p of procs) expect(exitOf(t, p)).toMatchObject({ reason: "killed", kill: "cancel" });
    // No synchronous call and no event callback holds the JS thread for 5 ms.
    expect(longestCall).toBeLessThan(5);
    expect(t.longestCallbackMs).toBeLessThan(5);
  });

  test("slow_js_backpressures_the_worker", { timeout: 60_000 }, async () => {
    const t = await start();
    const n = 400_000;
    // Unpaused, the flood takes well under the 2 s pause.
    const free = await ready(t);
    const t0 = performance.now();
    t.host.send(free.proc, startMsg(t, "free", "test.progressFlood", { n }));
    await t.waitFor(isResult(free.proc), 30_000);
    const freeMs = performance.now() - t0;
    expect(freeMs).toBeLessThan(1500);

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
      const spanS = (ps.at(-1)!.at - ps[0]!.at) / 1000;
      expect(ps.length).toBeLessThanOrEqual(4 * Math.ceil(spanS) + 2);
      // Every gap but the last (the pending value flushed before `result`) is ≥ 250 ms, less jitter.
      for (let i = 1; i < ps.length - 1; i++) expect(ps[i]!.at - ps[i - 1]!.at).toBeGreaterThan(200);
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
    // A closed group refuses spawns.
    t.host.closeGroup(group);
    await expect(t.host.spawn(group, worker(t))).rejects.toMatchObject({ code: "WORKER_UNAVAILABLE", details: { reason: "groupClosed" } });
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
      code: "WORKER_UNAVAILABLE",
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
