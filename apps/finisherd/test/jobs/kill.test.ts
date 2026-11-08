// `finisherd` death (5C-1 plan Task 12; Decisions 15, 17, 28, 29): a real engine in a child
// process (`support/engine-child.ts`) runs workers; the test kills the child (`SIGKILL`; on
// Windows `process.kill` is `TerminateProcess`) and checks that nothing it started survives,
// that the next start interrupts and sweeps, and that a graceful close leaves no `running` row.
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, test } from "vitest";
import { until } from "../support/jobs.ts";

const CHILD = fileURLToPath(new URL("../support/engine-child.ts", import.meta.url));
const WIN = process.platform === "win32";

interface Fact {
  readonly type: string;
  readonly [k: string]: unknown;
}
interface Row {
  readonly id: string;
  readonly kind: string;
  readonly state: string;
  readonly code?: string;
}

interface EngineChild {
  readonly child: ChildProcess;
  readonly facts: Fact[];
  readonly stderr: () => string;
  next(type: string, after?: number): Promise<Fact>;
  command(line: string): void;
}

const children: ChildProcess[] = [];
const dirs: string[] = [];
afterEach(() => {
  for (const c of children.splice(0)) if (c.exitCode === null && c.signalCode === null) c.kill("SIGKILL");
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

function serverData(): string {
  const d = mkdtempSync(join(tmpdir(), "fin-kill-"));
  dirs.push(d);
  return d;
}

function engineChild(data: string, ...jobs: string[]): EngineChild {
  const child = spawn(process.execPath, [CHILD, data, ...jobs], { stdio: ["pipe", "pipe", "pipe"], env: process.env });
  children.push(child);
  const facts: Fact[] = [];
  let out = "";
  let err = "";
  child.stdout?.setEncoding("utf8").on("data", (d: string) => {
    out += d;
    for (let nl = out.indexOf("\n"); nl >= 0; nl = out.indexOf("\n")) {
      const line = out.slice(0, nl);
      out = out.slice(nl + 1);
      if (line.startsWith('{"type"')) facts.push(JSON.parse(line) as Fact);
    }
  });
  child.stderr?.setEncoding("utf8").on("data", (d: string) => (err += d));
  return {
    child,
    facts,
    stderr: () => err,
    async next(type, after = 0) {
      try {
        return await until(() => facts.slice(after).find((f) => f.type === type), 30_000);
      } catch {
        throw new Error(`no ${type} from the engine child (exit ${child.exitCode}): ${err}`);
      }
    },
    command: (line) => void child.stdin?.write(`${line}\n`),
  };
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as { code?: string }).code === "EPERM";
  }
}

/** The watchdog of host process `host` (`fin-workers watchdog <host pid>`), Unix only. */
function watchdogOf(host: number): number | undefined {
  const ps = spawnSync("ps", ["-A", "-o", "pid=,args="], { encoding: "utf8" });
  for (const line of ps.stdout.split("\n")) {
    const m = /^\s*(\d+)\s+(.*)$/.exec(line);
    if (m !== null && new RegExp(`fin-workers\\S*\\s+watchdog\\s+${host}(\\s|$)`).test(m[2] as string)) return Number(m[1]);
  }
  return undefined;
}

function killChild(c: EngineChild): Promise<void> {
  return new Promise((resolve) => {
    c.child.once("exit", () => resolve());
    c.child.kill("SIGKILL");
  });
}

async function rowsOf(c: EngineChild): Promise<{ rows: Row[]; jobsDir: string[] }> {
  const at = c.facts.length;
  c.command("rows");
  return (await c.next("rows", at)) as unknown as { rows: Row[]; jobsDir: string[] };
}

describe("finisherd death", () => {
  test("finisherd_kill_leaves_no_process", { timeout: 60_000 }, async () => {
    const data = serverData();
    const c = engineChild(data, 'test.spawn={"detach":"none","out":"pids.json"}', "test.hang={}");
    const ready = await c.next("ready");
    await until(() => c.facts.filter((f) => f.type === "job").length === 2, 30_000);
    const jobs = c.facts.filter((f) => f.type === "job");
    const spawned = jobs.find((j) => j.kind === "test.spawn")?.pids as { worker: number; mid: number; grandchild: number };
    const hung = jobs.find((j) => j.kind === "test.hang")?.worker as number;
    const pids = [spawned.worker, spawned.mid, spawned.grandchild, hung].filter((p) => p > 0);
    const watchdog = WIN ? undefined : await until(() => watchdogOf(ready.pid as number), 5000);
    expect(alive(spawned.worker) && alive(hung)).toBe(true);
    await killChild(c);
    const t0 = Date.now();
    await until(() => pids.every((p) => !alive(p)) && (watchdog === undefined || !alive(watchdog)), 5000).catch(() => undefined);
    const left = [...pids, ...(watchdog === undefined ? [] : [watchdog])].filter(alive);
    expect(left, `still alive ${Date.now() - t0} ms after the kill`).toEqual([]);
  });

  test("restart_after_kill_interrupts_and_sweeps", { timeout: 60_000 }, async () => {
    const data = serverData();
    const first = engineChild(data, 'test.spawn={"detach":"none","out":"pids.json"}', "test.hang={}");
    await first.next("ready");
    await until(() => first.facts.filter((f) => f.type === "job").length === 2, 30_000);
    const before = await rowsOf(first);
    expect(before.rows.map((r) => r.state)).toEqual(["running", "running"]);
    expect(before.jobsDir.length).toBeGreaterThanOrEqual(2);
    await killChild(first);
    // Decision 28: rows found `running` are interrupted (neither kind is resumable); every directory is swept.
    const second = engineChild(data);
    await second.next("ready");
    const after = await rowsOf(second);
    expect(after.rows.map((r) => [r.kind, r.state, r.code]).sort()).toEqual([
      ["test.hang", "interrupted", "JOB_INTERRUPTED"],
      ["test.spawn", "interrupted", "JOB_INTERRUPTED"],
    ]);
    expect(after.jobsDir).toEqual([]);
    second.command("close");
    await second.next("closed");
  });

  test("graceful_close_leaves_no_running_rows", { timeout: 60_000 }, async () => {
    const data = serverData();
    const c = engineChild(data, "test.hang={}", 'test.checkpoint={"steps":100000,"stepMs":5}');
    await c.next("ready");
    await until(() => c.facts.filter((f) => f.type === "job").length === 2, 30_000);
    const workers = c.facts.filter((f) => f.type === "job").map((j) => j.worker as number);
    const at = c.facts.length;
    c.command("close");
    const { rows, jobsDir } = (await c.next("rows", at)) as unknown as { rows: Row[]; jobsDir: string[] };
    await c.next("closed", at);
    expect(rows.filter((r) => r.state === "running")).toEqual([]);
    // Decision 29: the resumable job goes back to the queue, the other is interrupted.
    expect(rows.find((r) => r.kind === "test.checkpoint")?.state).toBe("queued");
    expect(rows.find((r) => r.kind === "test.hang")).toMatchObject({ state: "interrupted", code: "JOB_INTERRUPTED" });
    expect(jobsDir.filter((d) => d.startsWith("warm-"))).toEqual([]);
    await until(() => workers.every((p) => !alive(p)), 5000);
  });
});
