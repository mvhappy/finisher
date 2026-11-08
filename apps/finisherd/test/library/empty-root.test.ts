// The owner's accept of a root that is empty for good (`acceptEmptyRoot`, plan 5D Decision 18 as
// extended by the Task 14 owner decision): the catalog stops refusing an empty root listing
// (`ROOT_EMPTY_SUSPECT`), the walker is not told the root's last count, and the root's files go
// missing only after the usual `DIR_EMPTY_SUSPECT` confirmation (a second empty listing by
// another job 60 s later). A listing that finds the root non-empty ends the accept; the accept
// survives a restart.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { ScanBatch } from "@finisher/core-types";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { Catalogs } from "../../src/library/catalog/client.ts";
import { catalogPath } from "../../src/library/catalog/open.ts";
import { type Scratch, dirEntry, done, entry, failCode, newLib, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";

let s: Scratch;
let c: Catalogs;
let now: number;
let lib: string;
const starts = new Map<string, number>();

beforeEach(async () => {
  starts.clear();
  s = scratch();
  now = 1_800_000_000_000;
  c = await threads(s, { clock: () => now });
  lib = newLib();
  await run(c.indexer.open(lib));
});
afterEach(async () => {
  await stopAll();
  s.cleanup();
});

async function file(job: string, entries: ScanBatch["entries"], dirsDone: ScanBatch["dirsDone"]) {
  now += 1000;
  const out = join(s.jobsRoot, job, "out");
  mkdirSync(out, { recursive: true });
  let t = starts.get(job);
  if (t === undefined) {
    t = await run(c.indexer.attemptStart(lib));
    starts.set(job, t);
  }
  return { path: writeBatch(out, [{ seq: 1, entries, dirsDone }]), startedAt: t };
}
const apply = async (job: string, entries: ScanBatch["entries"], dirsDone: ScanBatch["dirsDone"], gen = 1) => {
  const f = await file(job, entries, dirsDone);
  return run(c.indexer.applyBatchFile(lib, f.path, { gen, startedAt: f.startedAt }));
};
const failure = async (job: string, entries: ScanBatch["entries"], dirsDone: ScanBatch["dirsDone"], gen = 1) => {
  const f = await file(job, entries, dirsDone);
  return failCode(c.indexer.applyBatchFile(lib, f.path, { gen, startedAt: f.startedAt }));
};
const states = (): string[] => {
  const db = new DatabaseSync(catalogPath(s.serverData, lib), { readOnly: true });
  try {
    return (db.prepare("SELECT state FROM files ORDER BY id").all() as Array<{ state: string }>).map((r) => r.state);
  } finally {
    db.close();
  }
};
const rootLine = async (): Promise<Record<string, unknown>> => {
  const { text } = await run(c.indexer.knownDirs(lib, 1));
  return text.split("\n").filter((l) => l !== "").map((l) => JSON.parse(l) as { dir: { components: string[] } }).find((l) => l.dir.components.length === 0)!;
};
const seed = () => apply("s1", [dirEntry([], "A"), entry(["A"], "a.mov"), entry(["A"], "b.mov")], [done([], 1), done(["A"], 2)]);

describe("accepting an empty root", () => {
  test("an_empty_root_listing_is_refused_until_the_owner_accepts_it", async () => {
    await seed();
    expect(await failure("s2", [], [done([], 0)], 2)).toBe("ROOT_EMPTY_SUSPECT");
    expect(states()).toEqual(["present", "present"]);
    expect((await rootLine()).entries).toBe(1);
  });

  test("accepted_empty_root_marks_missing_only_after_confirmation", async () => {
    await seed();
    await run(c.indexer.setEmptyRootOk(lib, true));
    // The walker is no longer told the root had entries (its own empty-root refusal is inert).
    expect(await rootLine()).not.toHaveProperty("entries");
    const t0 = now + 1000;
    // The listing lands, and the root is a suspect directory like any other: nothing is marked.
    expect(await apply("p1", [], [done([], 0), done(["A"], 0)])).toMatchObject({ missing: 0, suspect: 2 });
    expect(states()).toEqual(["present", "present"]);
    now = t0 + 30_000; // another job, too soon
    expect(await apply("p2", [], [done([], 0), done(["A"], 0)])).toMatchObject({ missing: 0 });
    now = t0 + 70_000; // another job, 60 s after the first empty listing
    expect(await apply("p3", [], [done([], 0), done(["A"], 0)])).toMatchObject({ missing: 2 });
    expect(states()).toEqual(["missing", "missing"]);
  });

  test("a_non_empty_root_listing_ends_the_accept_and_it_survives_a_restart", async () => {
    await seed();
    await run(c.indexer.setEmptyRootOk(lib, true));
    await c.shutdown();
    c = await threads(s, { clock: () => now });
    await run(c.indexer.open(lib));
    expect(await rootLine()).not.toHaveProperty("entries");
    await apply("s2", [dirEntry([], "A")], [done([], 1)], 2);
    expect(await rootLine()).toHaveProperty("entries", 1);
    expect(await failure("s3", [], [done([], 0)], 3)).toBe("ROOT_EMPTY_SUSPECT");
  });
});
