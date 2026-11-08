// The job engine's budgets (5C-1 plan Task 12; Task 14 records the table per OS). Optimised
// builds: the `build:test` binary (release, with the `test` family) unless FIN_NODE_BINARY_DIR
// names another. Prints a table, or JSON with `--json`.
//   1. store statements with 10,000 jobs and 40,000 items: p50/p99 per statement (≤ 1 ms p99);
//   2. the warm hand-off under Task 11's load (10,000 queued cold jobs, cold spawns cycling):
//      submit → `start` handed to the host's `send`, p50/p99 (≤ 5 ms p99);
//   3. spawn → `hello` for `test.echo`, p50/p95 (6A-2's budget is ≤ 150 ms p95), and again with
//      1 GiB more resident in this process (Linux forks with `pre_exec`: page tables scale with
//      the parent; past 150 ms p95 a spawn helper process is a TASKS follow-up);
//   4. cancel → the job's tree (worker and grandchild) gone, p50/p95 (Unix).
// Usage: node apps/finisherd/scripts/jobs-budget.ts [--json] [--quick]
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { JobHostEvent } from "@finisher/core-types";

// The engine logs through `console.log`: logs go to stderr, the table (or JSON) alone to stdout.
console.log = (...args: unknown[]) => console.error(...args);
process.env.FIN_NODE_BINARY_DIR ??= fileURLToPath(new URL("../../../packages/fin-node/test/native/", import.meta.url));
const quick = process.argv.includes("--quick");
const asJson = process.argv.includes("--json");

// After the binary is chosen: these load fin-node.
const { openControlStore } = await import("../src/control/db.ts");
const { JobStore } = await import("../src/jobs/store.ts");
const { testEngine, until, closeTestEngines, run } = await import("../test/support/jobs.ts");
type JobHostFactory = import("../src/jobs/host.ts").JobHostFactory;

interface Stat {
  readonly name: string;
  readonly n: number;
  readonly p50: number;
  readonly tail: number;
  readonly tailAt: "p95" | "p99";
  readonly budget?: number;
}
const results: Stat[] = [];

function pct(samples: number[], p: number): number {
  const s = [...samples].sort((a, b) => a - b);
  return Number((s[Math.min(s.length - 1, Math.ceil(s.length * p) - 1)] ?? NaN).toFixed(3));
}
function stat(name: string, samples: number[], tailAt: "p95" | "p99", budget?: number): void {
  results.push({ name, n: samples.length, p50: pct(samples, 0.5), tail: pct(samples, tailAt === "p95" ? 0.95 : 0.99), tailAt, ...(budget === undefined ? {} : { budget }) });
}
function time(n: number, f: (i: number) => void): number[] {
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    f(i);
    out.push(performance.now() - t);
  }
  return out;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** fin-node's host, recording spawn calls, `hello`s and `start` sends. */
function timedHost(): { factory: JobHostFactory; spawnAt: Map<number, number>; helloAt: Map<number, number>; startAt: Map<string, number> } {
  const spawnAt = new Map<number, number>();
  const helloAt = new Map<number, number>();
  const startAt = new Map<string, number>();
  const factory: JobHostFactory = async (opts, onEvent) => {
    const { JobHost } = await import("@finisher/fin-node");
    const h = await JobHost.start(opts, (e: JobHostEvent) => {
      if (e.type === "hello") helloAt.set(e.proc, performance.now());
      onEvent(e);
    });
    return {
      caps: () => h.caps(),
      openGroup: (s) => h.openGroup(s),
      spawn: async (g, r) => {
        const t = performance.now();
        const proc = await h.spawn(g, r);
        spawnAt.set(proc, t);
        return proc;
      },
      send: (p, m) => {
        if (m.t === "start") startAt.set(m.job.id, performance.now());
        return h.send(p, m);
      },
      watch: (p, w) => h.watch(p, w),
      closeStdin: (p) => h.closeStdin(p),
      killGroup: (g, r) => h.killGroup(g, r),
      closeGroup: (g) => h.closeGroup(g),
      shutdown: (ms) => h.shutdown(ms),
    };
  };
  return { factory, spawnAt, helloAt, startAt };
}

/** 1. The store's per-request statements over 10,000 jobs and 40,000 items. */
async function storeBudget(): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), "fin-jobs-budget-"));
  const control = openControlStore(dir, { serverId: "budget", checkpoint: false });
  try {
    let now = Date.now();
    const store = new JobStore(control, () => now++);
    const owner = { kind: "local", id: "local" } as const;
    const ids: string[] = [];
    control.tx(() => {
      for (let i = 0; i < 10_000; i++) {
        const v = store.insert({ kind: "test.echo", kindVersion: 1, family: "test", params: { value: i }, owner, priority: "normal", maxAttempts: 3, dedupeKey: `d${i}` } as never);
        ids.push(v.id);
        store.writeItems(v.id, [0, 1, 2, 3].map((k) => [`item-${k}`, { value: k }] as const));
        if (i % 5 !== 0) {
          store.transition(v.id, "running", { attempts: 1 });
          store.transition(v.id, "succeeded", { result: { ok: true } });
        }
      }
    });
    const n = quick ? 500 : 2000;
    const pick = (i: number) => ids[(i * 7919) % ids.length] as string;
    const fresh: string[] = [];
    stat("store: insert", time(n, (i) => fresh.push(store.insert({ kind: "test.echo", kindVersion: 1, family: "test", params: { i }, owner, priority: "normal", maxAttempts: 3 } as never).id)), "p99", 1);
    stat("store: get", time(n, (i) => store.get(pick(i))), "p99", 1);
    stat("store: transition", time(n, (i) => store.transition(fresh[i] as string, "running", { attempts: 1 })), "p99", 1);
    stat("store: writeItems (1)", time(n, (i) => store.writeItems(fresh[i] as string, [[`x${i}`, { value: i }]])), "p99", 1);
    stat("store: items", time(n, (i) => store.items(pick(i))), "p99", 1);
    stat("store: checkpoint", time(n, (i) => store.checkpoint(fresh[i] as string, { step: i })), "p99", 1);
    stat("store: activeByDedupe", time(n, (i) => store.activeByDedupe("test.echo", `d${i % 10_000}`)), "p99", 1);
    stat("store: list (50)", time(n, () => store.list({ limit: 50 })), "p99", 1);
    stat("store: unfinishedCount", time(n, () => store.unfinishedCount()), "p99", 1);
  } finally {
    await control.close();
    rmSync(dir, { recursive: true, force: true });
  }
}

/** 2. The warm hand-off with 10,000 cold jobs queued and cold spawns cycling. */
async function handOffBudget(): Promise<void> {
  const h = timedHost();
  const t = await testEngine({ jobHost: h.factory, cores: 5, cgroups: "auto" });
  for (let i = 0; i < 10_000; i++) await t.submit("test.sleep", { ms: 20 });
  await t.runJob("test.warm", { value: "warm-up" });
  const lat: number[] = [];
  for (let i = 0; i < (quick ? 200 : 1000); i++) {
    const t0 = performance.now();
    const v = await t.submit("test.warm", { value: i });
    await t.wait(v.id);
    lat.push((h.startAt.get(v.id) ?? NaN) - t0);
  }
  stat("warm hand-off: submit → start (10k queued)", lat, "p99", 5);
  await closeTestEngines();
}

/** 3. spawn → hello for cold `test.echo` jobs. */
async function spawnBudget(label: string): Promise<void> {
  const h = timedHost();
  const t = await testEngine({ jobHost: h.factory, cgroups: "auto" });
  for (let i = 0; i < (quick ? 30 : 200); i++) await t.runJob("test.echo", { value: i });
  const lat = [...h.spawnAt].map(([proc, at]) => (h.helloAt.get(proc) ?? NaN) - at).filter(Number.isFinite);
  stat(`spawn → hello, test.echo${label}`, lat, "p95", 150);
  await closeTestEngines();
}

/** 4. cancel → the job's worker and grandchild gone. */
async function cancelBudget(): Promise<void> {
  if (process.platform === "win32") return;
  const t = await testEngine({ cgroups: "auto" });
  const gone = (pid: number) => {
    try {
      process.kill(pid, 0);
      return false;
    } catch (e) {
      return (e as { code?: string }).code === "ESRCH";
    }
  };
  const lat: number[] = [];
  for (let i = 0; i < (quick ? 10 : 50); i++) {
    const v = await t.submit("test.spawn", { detach: "none", out: "pids.json" });
    const file = join(t.serverData, "jobs", v.id, "pids.json");
    const pids = await until(() => {
      try {
        return JSON.parse(readFileSync(file, "utf8").replace(/\0+$/, "")) as { worker: number; grandchild: number };
      } catch {
        return undefined;
      }
    });
    const t0 = performance.now();
    await run(t.engine.cancel(v.id));
    while (!gone(pids.worker) || !gone(pids.grandchild)) await sleep(1);
    lat.push(performance.now() - t0);
  }
  stat("cancel → tree gone (test.spawn)", lat, "p95");
  await closeTestEngines();
}

await storeBudget();
await handOffBudget();
await spawnBudget("");
// 1 GiB more resident: touched pages the kernel must map in every fork's copy of the page tables.
const ballast: Buffer[] = [];
for (let i = 0; i < 16; i++) ballast.push(Buffer.alloc(64 * 1024 * 1024, i + 1));
await spawnBudget(" (+1 GiB resident)");
ballast.length = 0;
await cancelBudget();

if (asJson) process.stdout.write(`${JSON.stringify({ platform: process.platform, arch: process.arch, results }, null, 2)}\n`);
else {
  const rows = results.map((r) => [r.name, String(r.n), r.p50.toFixed(3), `${r.tailAt} ${r.tail.toFixed(3)}`, r.budget === undefined ? "recorded" : r.tail <= r.budget ? `≤ ${r.budget} ok` : `> ${r.budget} OVER`]);
  const head = ["measure", "n", "p50 ms", "tail ms", "budget"];
  const w = head.map((h, i) => Math.max(h.length, ...rows.map((r) => (r[i] as string).length)));
  const line = (r: string[]) => r.map((c, i) => c.padEnd(w[i] as number)).join("  ");
  process.stdout.write(`job engine budgets on ${process.platform}/${process.arch}\n${line(head)}\n${line(w.map((n) => "-".repeat(n)))}\n${rows.map(line).join("\n")}\n`);
}
process.exit(0);
