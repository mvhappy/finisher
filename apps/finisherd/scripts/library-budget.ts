// The library indexer's budgets (plan 5D Task 15; Task 17 records them on each OS). Release
// builds: fin-node's `build` binary and its `fin-workers` unless FIN_NODE_BINARY_DIR names
// another. A real engine, catalog threads and library service, as `finisherd` wires them, over a
// generated tree (`library-budget-tree.ts`):
//   - full scan of 100k files in 5k directories, end to end (submit → the service noted its end):
//     the first (empty catalog) and a repeat, both warm (the tree's metadata in the OS cache),
//     ≤ 20 s; `--cold` drops the caches before a third (Linux: root or `sudo -n`; macOS: `purge`), recorded;
//   - an unchanged poll of the 5k directories ≤ 1.5 s;
//   - the indexer's apply ≥ 25k entries/s (the tree's batches into fresh catalogs);
//   - search p95 ≤ 50 ms on the 100k library and ≤ 200 ms over five, first and later pages;
//   - event-loop lag p99 ≤ 20 ms during the scans, and no catalog statement or batch parse on
//     the main thread;
//   - catalog ≤ 1 KiB per file (after the probe pass), worker RSS ≤ 256 MiB, probe throughput;
//   - with FIN_TEST_SHARE (a mounted share): a 10k-file scan, first, repeat and (`--cold`) cold, and
//     an unchanged poll of its 1k directories, recorded.
// Usage: node apps/finisherd/scripts/library-budget.ts [--json] [--cold] [--quick]
// Prints a table (or JSON); exits 1 when a budget is over.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { monitorEventLoopDelay } from "node:perf_hooks";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Effect } from "effect";
import { FULL, QUERIES, type TreeShape, treeBatches, writeTree } from "./library-budget-tree.ts";

export interface BudgetLine {
  readonly name: string;
  readonly value: number | null;
  readonly unit: string;
  /** `max`: value ≤ limit; `min`: value ≥ limit. */
  readonly budget?: { readonly limit: number; readonly kind: "max" | "min" };
  readonly note?: string;
}

export interface BudgetOptions {
  readonly shape?: TreeShape;
  /** Libraries searched together (the first is the scanned one, the others are applied). */
  readonly libraries?: number;
  readonly searchSamples?: number;
  readonly cold?: boolean;
  /** A mounted share to measure on (FIN_TEST_SHARE). */
  readonly share?: string;
  readonly shareShape?: TreeShape;
  readonly log?: (line: string) => void;
}

export interface BudgetResult {
  readonly platform: string;
  readonly files: number;
  readonly lines: BudgetLine[];
  /** Catalog statements and batch parses seen on the main thread (must be 0). */
  readonly mainThread: { readonly statements: number; readonly batchParses: number };
}

const ms = (t0: number): number => performance.now() - t0;
const round = (n: number, d = 1): number => Number(n.toFixed(d));
const sleep = (n: number) => new Promise((r) => setTimeout(r, n));

function pct(samples: number[], p: number): number {
  const s = [...samples].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.max(0, Math.ceil(s.length * p) - 1))] ?? Number.NaN;
}

/** Counts catalog statements prepared or run on this thread, and batch lines parsed here. */
function mainThreadSpy(): { readonly counts: { statements: number; batchParses: number }; restore(): void } {
  const counts = { statements: 0, batchParses: 0 };
  const catalog = `${sep}libraries${sep}`;
  const proto = DatabaseSync.prototype as unknown as Record<"prepare" | "exec", (this: DatabaseSync, sql: string) => unknown>;
  const real = { prepare: proto.prepare, exec: proto.exec };
  for (const m of ["prepare", "exec"] as const) {
    proto[m] = function (this: DatabaseSync, sql: string) {
      if ((this.location() ?? "").includes(catalog)) counts.statements++;
      return real[m].call(this, sql);
    };
  }
  const parse = JSON.parse;
  JSON.parse = ((text: string, reviver?: Parameters<typeof JSON.parse>[1]) => {
    if (typeof text === "string" && text.includes('"nameNfc"')) counts.batchParses++;
    return parse(text, reviver);
  }) as typeof JSON.parse;
  return {
    counts,
    restore() {
      proto.prepare = real.prepare;
      proto.exec = real.exec;
      JSON.parse = parse;
    },
  };
}

/** The largest resident size of this process's `fin-workers` children, sampled until stopped (Linux and macOS). */
function workerRss(): { stop(): number | null } {
  let peak: number | null = null;
  const sample = () => {
    try {
      if (process.platform === "linux") {
        for (const pid of readdirSync("/proc")) {
          if (!/^\d+$/.test(pid)) continue;
          let status: string;
          try {
            status = readFileSync(`/proc/${pid}/status`, "utf8");
          } catch {
            continue;
          }
          if (!/^Name:\s+fin-workers$/m.test(status) || Number(/^PPid:\s+(\d+)/m.exec(status)?.[1]) !== process.pid) continue;
          const kb = Number(/^VmHWM:\s+(\d+)/m.exec(status)?.[1] ?? /^VmRSS:\s+(\d+)/m.exec(status)?.[1]);
          if (Number.isFinite(kb)) peak = Math.max(peak ?? 0, kb / 1024);
        }
      } else if (process.platform === "darwin") {
        const out = execFileSync("ps", ["-A", "-o", "ppid=,rss=,comm="], { encoding: "utf8" });
        for (const line of out.split("\n")) {
          const [ppid, rss, ...comm] = line.trim().split(/\s+/);
          if (Number(ppid) === process.pid && comm.join(" ").endsWith("fin-workers")) peak = Math.max(peak ?? 0, Number(rss) / 1024);
        }
      }
    } catch {
      // A sample lost is a sample lost.
    }
  };
  const timer = setInterval(sample, 200);
  return {
    stop() {
      clearInterval(timer);
      sample();
      return peak === null ? null : round(peak);
    },
  };
}

/** Drops the OS caches (Linux as root, macOS `purge`); answers why not when it cannot. */
function dropCaches(): string | undefined {
  try {
    if (process.platform === "linux") {
      execFileSync("sync");
      try {
        writeFileSync("/proc/sys/vm/drop_caches", "3");
      } catch {
        // Not root: `sudo -n` never asks for a password, it fails instead.
        execFileSync("sudo", ["-n", "sh", "-c", "echo 3 > /proc/sys/vm/drop_caches"], { stdio: "ignore" });
      }
      return undefined;
    }
    if (process.platform === "darwin") {
      execFileSync("purge");
      return undefined;
    }
    return "Windows: reboot, or detach and re-attach a VHD, then run again";
  } catch (e) {
    return `cannot drop caches here (${(e as NodeJS.ErrnoException).code ?? "failed"}: needs root)`;
  }
}

export async function measureLibraryBudget(opts: BudgetOptions = {}): Promise<BudgetResult> {
  const shape = opts.shape ?? FULL;
  const say = opts.log ?? (() => {});
  const lines: BudgetLine[] = [];
  const line = (l: BudgetLine) => {
    lines.push(l);
    say(`${l.name}: ${l.value ?? "-"} ${l.unit}${l.note === undefined ? "" : ` (${l.note})`}`);
  };
  const spy = mainThreadSpy();
  // After the spy: these open the control store and start the catalog threads.
  const { openControlStore } = await import("../src/control/db.ts");
  const { startCatalogThreads } = await import("../src/library/catalog/client.ts");
  const { catalogPath } = await import("../src/library/catalog/open.ts");
  const { libraryWiring } = await import("../src/library/layer.ts");
  const { addLibrary } = await import("../src/library/admin.ts");
  const { searchLibraries } = await import("../src/library/search.ts");
  const { ulid } = await import("../src/control/ulid.ts");
  const { testEngine } = await import("../test/support/jobs.ts");

  // Everything started is stopped (and every folder made removed) in reverse, whatever fails.
  const undo: Array<() => unknown> = [];
  const run = <A, E>(e: Effect.Effect<A, E>) => Effect.runPromise(e);
  try {
    const base = realpathSync(mkdtempSync(join(tmpdir(), "fin-libbudget-")));
    undo.push(() => rmSync(base, { recursive: true, force: true }));
    const share = opts.share === undefined ? undefined : realpathSync(mkdtempSync(join(opts.share, "fin-libbudget-")));
    if (share !== undefined) undo.push(() => rmSync(share, { recursive: true, force: true }));
    const serverData = join(base, "s");
    const jobsRoot = join(serverData, "jobs");
    mkdirSync(jobsRoot, { recursive: true, mode: 0o700 });
    const control = openControlStore(serverData, { serverId: "budget", checkpoint: false });
    undo.push(() => control.close());
    const catalogs = await startCatalogThreads({ serverData, jobsRoot });
    undo.push(() => catalogs.shutdown());
    const wiring = libraryWiring({ serverData, jobsRoot, control, catalogs, log: () => {} });
    const t = await testEngine({ serverData, keepData: true, testKinds: false, register: (k) => wiring.register(k) });
    undo.push(() => t.close());
    const svc = await wiring.start(t.engine);
    undo.push(() => svc.close());
    const actor = { kind: "console", id: "console" };
    /** Submits a scan and waits until the service has noted its end; answers the milliseconds. */
    const scan = async (id: string, mode: "full" | "poll"): Promise<number> => {
      const t0 = performance.now();
      const before = svc.view(svc.registry.get(id)!).lastScan?.at ?? 0;
      const job = await svc.submitScan(id, { mode, trigger: "schedule" });
      if (job === undefined) throw new Error(`the ${mode} scan of ${id} was not submitted`);
      const [done] = await run(t.engine.wait([job.id], { timeoutMs: 600_000 }));
      if (done?.state !== "succeeded") throw new Error(`the ${mode} scan ended ${JSON.stringify(done)}`);
      for (;;) {
        const last = svc.view(svc.registry.get(id)!).lastScan;
        if (last !== undefined && last.at > before && last.mode === mode) return ms(t0);
        await sleep(2);
      }
    };
    const lagDuring = async <A>(f: () => Promise<A>): Promise<{ value: A; p99: number }> => {
      const h = monitorEventLoopDelay({ resolution: 10 });
      h.enable();
      try {
        const value = await f();
        return { value, p99: h.percentile(99) / 1e6 };
      } finally {
        h.disable();
      }
    };
    const files = (counts: Record<string, number>) => Object.values(counts).reduce((a, b) => a + b, 0);

    say("writing the tree");
    const root = join(base, "Footage");
    writeTree(root, shape);
    const added = await addLibrary(svc, { root, name: "Footage" }, actor);
    const lib = added.library.id;
    const rss = workerRss();
    const first = await lagDuring(async () => {
      const t0 = performance.now();
      if (added.job !== undefined) await run(t.engine.wait([added.job.id], { timeoutMs: 600_000 }));
      for (;;) {
        if (svc.view(svc.registry.get(lib)!).lastScan !== undefined) return ms(t0);
        await sleep(2);
      }
    });
    const n = files((await run(catalogs.reader.counts(lib))).files);
    line({ name: `full scan, first (${n} files), warm`, value: round(first.value / 1000, 2), unit: "s", budget: { limit: 20, kind: "max" } });

    // The probe pass: every file is probed once (they are not media: each ends `probeFailed`).
    say("waiting for the probe pass");
    const p0 = performance.now();
    for (;;) {
      const counts = (await run(catalogs.reader.counts(lib))).files;
      if ((counts.present ?? 0) === 0) break;
      if (ms(p0) > 900_000) throw new Error(`the probe pass did not end: ${JSON.stringify(counts)}`);
      await sleep(100);
    }
    line({ name: "probe throughput", value: round(n / (ms(p0) / 1000), 0), unit: "files/s", note: "small non-media files: one open, fingerprint and refusal each" });

    const repeat = await lagDuring(() => scan(lib, "full"));
    line({ name: "full scan, repeat, warm", value: round(repeat.value / 1000, 2), unit: "s", budget: { limit: 20, kind: "max" } });
    const poll = await lagDuring(() => scan(lib, "poll"));
    line({ name: `unchanged poll of ${shape.top * shape.sub + shape.top + 1} directories`, value: round(poll.value / 1000, 3), unit: "s", budget: { limit: 1.5, kind: "max" } });
    // `monitorEventLoopDelay` measures timer gaps, its 10 ms resolution included: stricter, not looser.
    line({ name: "event-loop lag p99 during scans", value: round(Math.max(first.p99, repeat.p99, poll.p99), 2), unit: "ms", budget: { limit: 20, kind: "max" }, note: "includes the 10 ms sampling interval" });
    line({ name: "worker RSS, peak", value: rss.stop(), unit: "MiB", budget: { limit: 256, kind: "max" }, ...(process.platform === "win32" ? { note: "not sampled on Windows" } : {}) });

    // The catalog's size, with the probe pass's rows (WAL checkpointed by a close and reopen).
    await run(catalogs.indexer.close(lib));
    const path = catalogPath(serverData, lib);
    const bytes = ["", "-wal"].reduce((a, s) => a + (existsSync(path + s) ? statSync(path + s).size : 0), 0);
    await run(catalogs.indexer.open(lib));
    line({ name: "catalog size per file (with FTS)", value: round(bytes / n, 0), unit: "B", budget: { limit: 1024, kind: "max" } });

    // The indexer's apply rate: the tree's listings into fresh catalogs (also the other libraries searched).
    const { batches, entries } = treeBatches(shape);
    const out = join(jobsRoot, "budget-apply", "out");
    mkdirSync(out, { recursive: true });
    const others: string[] = [];
    const rates: number[] = [];
    for (let i = 1; i < (opts.libraries ?? 5); i++) {
      const id = ulid();
      await run(catalogs.indexer.open(id));
      const paths = batches.map((b, j) => {
        const p = join(out, `l${i}-${j}.ndjson`);
        writeFileSync(p, `${JSON.stringify(b)}\n`);
        return p;
      });
      const t0 = performance.now();
      for (const p of paths) await run(catalogs.indexer.applyBatchFile(id, p, { gen: 1, startedAt: Number.MAX_SAFE_INTEGER }));
      rates.push(entries / (ms(t0) / 1000));
      others.push(id);
    }
    if (rates.length > 0) line({ name: "indexer apply (median of fresh catalogs)", value: round(pct(rates, 0.5), 0), unit: "entries/s", budget: { limit: 25_000, kind: "min" } });

    // Search: first pages and the pages after them, one library and all of them.
    const searchP95 = async (libraries: string[]): Promise<{ first: number; later: number }> => {
      const firstPages: number[] = [];
      const laterPages: number[] = [];
      const samples = opts.searchSamples ?? 200;
      for (let i = 0; i < samples; i++) {
        const query = i % 6 === 5 ? "" : QUERIES[i % QUERIES.length]!;
        const t0 = performance.now();
        let page = await run(searchLibraries(catalogs.reader, { libraries, query, limit: 50 }));
        firstPages.push(ms(t0));
        for (let k = 0; k < 2 && page.next !== undefined; k++) {
          const t1 = performance.now();
          page = await run(searchLibraries(catalogs.reader, { libraries, query, limit: 50, cursor: page.next }));
          laterPages.push(ms(t1));
        }
      }
      return { first: round(pct(firstPages, 0.95), 2), later: round(pct(laterPages, 0.95), 2) };
    };
    const one = await searchP95([lib]);
    line({ name: "search p95, one library, first page", value: one.first, unit: "ms", budget: { limit: 50, kind: "max" } });
    line({ name: "search p95, one library, later pages", value: one.later, unit: "ms", budget: { limit: 50, kind: "max" } });
    if (others.length > 0) {
      const all = await searchP95([lib, ...others]);
      line({ name: `search p95, ${others.length + 1} libraries, first page`, value: all.first, unit: "ms", budget: { limit: 200, kind: "max" } });
      line({ name: `search p95, ${others.length + 1} libraries, later pages`, value: all.later, unit: "ms", budget: { limit: 200, kind: "max" } });
    }

    // Cold last: dropping the caches would make every measure after it a cold one too.
    if (opts.cold === true) {
      const why = dropCaches();
      if (why === undefined) line({ name: "full scan, repeat, cold", value: round((await scan(lib, "full")) / 1000, 2), unit: "s" });
      else line({ name: "full scan, repeat, cold", value: null, unit: "s", note: why });
    }

    if (share !== undefined) {
      const shareShape = opts.shareShape ?? { top: 10, sub: 100, per: 10 };
      say("writing the tree on the share");
      writeTree(join(share, "Footage"), shareShape);
      const s = await addLibrary(svc, { root: join(share, "Footage"), name: "Share", storage: "network" }, actor);
      const t0 = performance.now();
      if (s.job !== undefined) await run(t.engine.wait([s.job.id], { timeoutMs: 600_000 }));
      while (svc.view(svc.registry.get(s.library.id)!).lastScan === undefined) await sleep(2);
      line({ name: "share: full scan, first (tree just written)", value: round(ms(t0) / 1000, 2), unit: "s" });
      line({ name: "share: full scan, repeat, warm", value: round((await scan(s.library.id, "full")) / 1000, 2), unit: "s" });
      if (opts.cold === true) {
        const why = dropCaches();
        line({ name: "share: full scan, repeat, cold", value: why === undefined ? round((await scan(s.library.id, "full")) / 1000, 2) : null, unit: "s", ...(why === undefined ? {} : { note: why }) });
      }
      line({ name: `share: unchanged poll of ${shareShape.top * shareShape.sub} directories`, value: round((await scan(s.library.id, "poll")) / 1000, 3), unit: "s" });
    }
    return { platform: `${process.platform}/${process.arch}`, files: n, lines, mainThread: { ...spy.counts } };
  } finally {
    spy.restore();
    for (const f of undo.reverse()) {
      try {
        await f();
      } catch (e) {
        say(`cleanup: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
  }
}

/** True when `l` is within its budget or has none; a budget not measurable here says why in its note. */
export function withinBudget(l: BudgetLine): boolean {
  if (l.budget === undefined) return true;
  if (l.value === null) return l.note !== undefined;
  return l.budget.kind === "max" ? l.value <= l.budget.limit : l.value >= l.budget.limit;
}

async function main(argv: string[]): Promise<number> {
  // Logs go to stderr; the table (or JSON) alone to stdout.
  console.log = (...args: unknown[]) => console.error(...args);
  const quick = argv.includes("--quick");
  const result = await measureLibraryBudget({
    ...(quick ? { shape: { top: 10, sub: 20, per: 10 }, searchSamples: 30 } : {}),
    cold: argv.includes("--cold"),
    ...(process.env.FIN_TEST_SHARE ? { share: process.env.FIN_TEST_SHARE } : {}),
    log: (l) => process.stderr.write(`library-budget: ${l}\n`),
  });
  const over = result.lines.filter((l) => !withinBudget(l));
  const onMain = result.mainThread.statements + result.mainThread.batchParses;
  if (argv.includes("--json")) process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  else {
    const rows = result.lines.map((l) => [
      l.name,
      l.value === null ? "-" : `${l.value} ${l.unit}`,
      l.budget === undefined ? "recorded" : `${l.budget.kind === "max" ? "≤" : "≥"} ${l.budget.limit} ${withinBudget(l) ? "ok" : "OVER"}`,
      l.note ?? "",
    ]);
    rows.push(["main thread: catalog statements, batch parses", `${result.mainThread.statements}, ${result.mainThread.batchParses}`, `= 0 ${onMain === 0 ? "ok" : "OVER"}`, ""]);
    const head = ["measure", "value", "budget", "note"];
    const w = head.map((h, i) => Math.max(h.length, ...rows.map((r) => r[i]!.length)));
    const fmt = (r: string[]) => r.map((c, i) => c.padEnd(w[i]!)).join("  ").trimEnd();
    process.stdout.write(`library budgets on ${result.platform}${quick ? " (quick: not the budget's size)" : ""}\n${fmt(head)}\n${fmt(w.map((x) => "-".repeat(x)))}\n${rows.map(fmt).join("\n")}\n`);
  }
  return over.length === 0 && onMain === 0 ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  process.env.FIN_NODE_BINARY_DIR ??= fileURLToPath(new URL("../../../packages/fin-node/", import.meta.url));
  process.exit(await main(process.argv.slice(2)));
}
