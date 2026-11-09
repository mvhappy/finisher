// The library budget script at a small size (plan 5D Task 15): every measure runs and the main
// thread runs no catalog statement and parses no batch. The figures themselves are the
// script's to judge at full size (`apps/finisherd/scripts/library-budget.ts`), never asserted
// here: this machine's load moves them.
import { expect, test } from "vitest";
import { measureLibraryBudget, withinBudget } from "../../scripts/library-budget.ts";

test("library_budget_runs_end_to_end_at_a_small_size", { timeout: 180_000 }, async () => {
  const r = await measureLibraryBudget({ shape: { top: 3, sub: 4, per: 5 }, libraries: 2, searchSamples: 12 });
  // 12 leaf directories of 5 files; directories 0 and 10 (by index) add two twins each, which a
  // filesystem folding case or normalisation merges into the first.
  expect(r.files).toBeGreaterThanOrEqual(60);
  expect(r.files).toBeLessThanOrEqual(64);
  expect(r.mainThread).toEqual({ statements: 0, batchParses: 0 });
  const names = r.lines.map((l) => l.name.replace(/\(\d+ files\)/, "(n files)").replace(/\d+ (directories|libraries)/, "N $1"));
  expect(names).toEqual([
    "full scan, first (n files), warm",
    "probe throughput",
    "full scan, repeat, warm",
    "unchanged poll of N directories",
    "event-loop lag p99 during scans",
    "worker RSS, peak",
    "catalog size per file (with FTS)",
    "indexer apply (median of fresh catalogs)",
    "search p95, one library, first page",
    "search p95, one library, later pages",
    "search p95, N libraries, first page",
    "search p95, N libraries, later pages",
  ]);
  for (const l of r.lines) if (l.name !== "worker RSS, peak" && !l.name.includes("later pages")) expect(l.value, l.name).toBeGreaterThan(0);
  if (process.platform !== "win32") expect(r.lines.find((l) => l.name === "worker RSS, peak")!.value).toBeGreaterThan(0);
});

test("within_budget_reads_max_min_and_unmeasured_lines", () => {
  expect(withinBudget({ name: "a", value: 3, unit: "s", budget: { limit: 20, kind: "max" } })).toBe(true);
  expect(withinBudget({ name: "a", value: 21, unit: "s", budget: { limit: 20, kind: "max" } })).toBe(false);
  expect(withinBudget({ name: "b", value: 24_000, unit: "/s", budget: { limit: 25_000, kind: "min" } })).toBe(false);
  expect(withinBudget({ name: "c", value: null, unit: "MiB", budget: { limit: 256, kind: "max" } })).toBe(false);
  expect(withinBudget({ name: "c", value: null, unit: "MiB", budget: { limit: 256, kind: "max" }, note: "not sampled" })).toBe(true);
  expect(withinBudget({ name: "d", value: null, unit: "s" })).toBe(true);
});
