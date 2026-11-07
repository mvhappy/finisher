// The indexer and reader threads keep the main thread free (plan 5D Task 7a, Decision 2).
import fs, { readFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { type Scratch, b64, done, entry, newLib, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";

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
  const out = await run(c.indexer.applyBatchFile(lib, path, 1));
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
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [{ seq: 1, entries, dirsDone: [done([], 300)] }]), 1));
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
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, batches), 1));
  // Warm the reader's statements.
  for (let i = 0; i < 3; i++) await run(c.reader.search(lib, { query: "interv", limit: 50 }));
  let spinning = true;
  const spin = run(c.indexer.exec(lib, "notes", "spin", { ms: 800 })).finally(() => {
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
  expect(Math.max(...times)).toBeLessThan(20);
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
  const long = run(c.indexer.applyBatchFile(a, path, 1)).then((r) => {
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
