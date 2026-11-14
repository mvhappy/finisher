// The indexer and reader threads keep the main thread free (plan 5D Task 7a, Decision 2).
import fs, { readFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { type Scratch, b64, done, entry, newLib, run, scratch, stopAll, threads, writeBatch, applied } from "./support/catalog.ts";

let s: Scratch;
beforeEach(() => {
  s = scratch();
});
afterEach(async () => {
  vi.restoreAllMocks();
  syncBuiltinESMExports();
  await stopAll();
  s.cleanup();
});

const MARKER = "zz-main-thread-marker";

test("main_thread_parses_no_batch", async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  const entries = Array.from({ length: 500 }, (_, i) => entry([], `${MARKER}-${i}.mov`));
  const path = writeBatch(s.outDir, [{ seq: 1, entries, dirsDone: [done([], 500)] }]);
  const markerB64 = b64(`${MARKER}-0.mov`);
  const touched: string[] = [];
  // Record every file read and JSON parse on this thread while the batch is applied.
  for (const name of ["readFileSync", "openSync", "readSync", "lstatSync", "statSync"] as const) {
    const real = fs[name] as (...a: unknown[]) => unknown;
    vi.spyOn(fs, name).mockImplementation(((...a: unknown[]) => {
      touched.push(`${name}(${String(a[0])})`);
      return real.apply(fs, a);
    }) as never);
  }
  const parseSpy = vi.spyOn(JSON, "parse");
  syncBuiltinESMExports();
  const out = await run(c.indexer.applyBatchFile(lib, path, applied(1)));
  expect(out.added).toBe(500);
  expect(touched.filter((t) => t.includes(path) || t.includes(s.outDir))).toEqual([]);
  expect(parseSpy.mock.calls.filter(([text]) => typeof text === "string" && text.includes(markerB64))).toEqual([]);
  // The spies do see a read and a parse on this thread.
  JSON.parse(readFileSync(path, "utf8").split("\n")[0]!);
  expect(touched.some((t) => t.includes(path))).toBe(true);
  expect(parseSpy.mock.calls.some(([text]) => typeof text === "string" && text.includes(markerB64))).toBe(true);
});

test("main_thread_runs_no_catalog_statement", async () => {
  const prepare = vi.spyOn(DatabaseSync.prototype, "prepare");
  const exec = vi.spyOn(DatabaseSync.prototype, "exec");
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  const entries = Array.from({ length: 300 }, (_, i) => entry([], `clip-${i}.mov`));
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [{ seq: 1, entries, dirsDone: [done([], 300)] }]), applied(1)));
  await run(c.indexer.exec(lib, "notes", "add", { text: "x" }));
  await run(c.reader.search(lib, { query: "clip", limit: 10 }));
  await run(c.reader.files(lib, { limit: 10 }));
  await run(c.reader.counts(lib));
  await run(c.indexer.close(lib));
  expect(prepare).not.toHaveBeenCalled();
  expect(exec).not.toHaveBeenCalled();
});

test("reader_never_waits_for_writer", async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  const entries = Array.from({ length: 2000 }, (_, i) => entry(["Shoot"], `interview-${i}.mov`));
  const batches = [
    { seq: 1, entries: [entry([], "Shoot", { type: "dir", size: 0 })], dirsDone: [done([], 1)] },
    { seq: 1, entries, dirsDone: [done(["Shoot"], 2000)] },
  ];
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, batches), applied(1)));
  // Warm the reader's statements.
  for (let i = 0; i < 3; i++) await run(c.reader.search(lib, { query: "interv", limit: 50 }));
  let spinning = true;
  const spin = run(c.indexer.exec(lib, "notes", "spin", { ms: 3000 })).finally(() => {
    spinning = false;
  });
  await new Promise((r) => setTimeout(r, 100));
  const times: number[] = [];
  for (let i = 0; i < 10; i++) {
    const t0 = performance.now();
    const hits = await run(c.reader.search(lib, { query: "interv", limit: 50 }));
    times.push(performance.now() - t0);
    expect(hits.hits).toHaveLength(50);
  }
  expect(spinning).toBe(true);
  // A reader that waited for the writer would take the rest of the 3 s spin (2.9 s), so a
  // read over 1 s says it did; below that the figures measure the machine (at load average 35
  // three of ten reads took over 20 ms), so the median and the tail are generous. Every read
  // under 20 ms is asked with `FIN_STRICT_TIMING=1`.
  const sorted = [...times].sort((a, b) => a - b);
  if (process.env.FIN_STRICT_TIMING === "1") expect(sorted.at(-1)).toBeLessThan(20);
  expect(sorted[Math.floor(sorted.length / 2)]).toBeLessThan(100);
  expect(sorted.at(-1)).toBeLessThan(1000);
  expect((await spin) as { n: number }).toHaveProperty("n");
});

test("libraries interleave: a long apply of one does not block another's op", async () => {
  const a = newLib();
  const b = newLib();
  const c = await threads(s, { txRows: 100 });
  await run(c.indexer.open(a));
  await run(c.indexer.open(b));
  const entries = Array.from({ length: 4000 }, (_, i) => entry([], `long-${i}.mov`));
  const path = writeBatch(s.outDir, [{ seq: 1, entries, dirsDone: [done([], 4000)] }]);
  let longDone = false;
  const long = run(c.indexer.applyBatchFile(a, path, applied(1))).then((r) => {
    longDone = true;
    return r;
  });
  await new Promise((r) => setTimeout(r, 5));
  await run(c.indexer.exec(b, "notes", "add", { text: "quick" }));
  const quickFirst = !longDone;
  expect((await long).transactions).toBe(41);
  expect(quickFirst).toBe(true);
  expect(readFileSync(path).length).toBeGreaterThan(0);
});

test("a dead indexer thread is a fatal event and a typed error, never restarted silently", async () => {
  const lib = newLib();
  const fatal: Array<{ code: string }> = [];
  const c = await threads(s, { onFatal: (e) => fatal.push(e) });
  await run(c.indexer.open(lib));
  expect(c.health()).toEqual({ ok: true });
  const code = (e: unknown) => (e as { code: string }).code;
  expect(await run(c.indexer.exec(lib, "notes", "crash", {})).catch(code)).toBe("CATALOG_THREAD_EXITED");
  expect(await run(c.indexer.stats()).catch(code)).toBe("CATALOG_THREAD_EXITED");
  expect(fatal.map((e) => e.code)).toEqual(["CATALOG_THREAD_EXITED"]);
  const h = c.health();
  expect(h.ok).toBe(false);
  expect(h.ok ? undefined : h.error.code).toBe("CATALOG_THREAD_EXITED");
  // The reader still answers; a clean shutdown reports nothing more.
  await c.shutdown();
  expect(fatal).toHaveLength(1);
});

test("a subtree change runs in steps of ≤ txRows rows and readers see it whole", async () => {
  const lib = newLib();
  const c = await threads(s, { txRows: 100 });
  await run(c.indexer.open(lib));
  const DIRS = 40;
  const PER = 150;
  const dirs = Array.from({ length: DIRS }, (_, i) => `d${i}`);
  const entries = [entry([], "Big", { type: "dir", size: 0 }), ...dirs.map((d) => entry(["Big"], d, { type: "dir", size: 0 }))];
  const files = dirs.flatMap((d) => Array.from({ length: PER }, (_, i) => entry(["Big", d], `f${i}.mov`)));
  const lines = [
    { seq: 1, entries, dirsDone: [done([], 1), done(["Big"], DIRS)] },
    ...dirs.map((d, k) => ({ seq: 1, entries: files.slice(k * PER, (k + 1) * PER), dirsDone: [done(["Big", d], PER)] })),
  ];
  for (let i = 0; i < lines.length; i += 10) await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, lines.slice(i, i + 10)), applied(1)));
  expect((await run(c.reader.counts(lib))).files).toEqual({ present: DIRS * PER });
  const before = await run(c.indexer.stats());
  // `Big` becomes excluded: 6,000 rows go missing in one transaction of many steps.
  const path = writeBatch(s.outDir, [{ seq: 1, entries: [entry([], "Big", { type: "dir", size: 0, excluded: "GLOB" })], dirsDone: [done([], 1)] }]);
  let finished = false;
  const applying = run(c.indexer.applyBatchFile(lib, path, applied(1))).finally(() => {
    finished = true;
  });
  const seen = new Set<string>();
  while (!finished) seen.add(JSON.stringify((await run(c.reader.counts(lib))).files));
  expect(await applying).toMatchObject({ missing: DIRS * PER, excluded: DIRS * PER, transactions: 1 });
  seen.add(JSON.stringify((await run(c.reader.counts(lib))).files));
  expect([...seen].every((v) => v === JSON.stringify({ present: DIRS * PER }) || v === JSON.stringify({ missing: DIRS * PER }))).toBe(true);
  const after = await run(c.indexer.stats());
  expect(after.transactions - before.transactions).toBe(1);
  expect(after.maxStepRows).toBeLessThanOrEqual(100);
});
