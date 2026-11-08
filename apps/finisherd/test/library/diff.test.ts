// The catalog diff, case by case (plan 5D Task 7b, Decisions 11, 18, 22, 25). The property
// test (`diff.property.test.ts`) covers the combinations; these pin each rule's meaning.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { ScanBatch, ScanEntry } from "@finisher/core-types";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { Catalogs } from "../../src/library/catalog/client.ts";
import { catalogPath } from "../../src/library/catalog/open.ts";
import { type Scratch, dirEntry, done, entry, failCode, newLib, rel, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";

let s: Scratch;
let c: Catalogs;
let now: number;
let lib: string;
beforeEach(async () => {
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

const id = (n: number): string => Buffer.from(n.toString(16).padStart(16, "0"), "hex").toString("base64");
const withDev = (e: ScanEntry): ScanEntry => ({ ...e, dev: "64769" });

/** Applies `batches` as one file of `job` (its own `out/`), `now` advanced first. */
async function apply(job: string, batches: ScanBatch[], gen = 1) {
  now += 1000;
  const out = join(s.jobsRoot, job, "out");
  mkdirSync(out, { recursive: true });
  return run(c.indexer.applyBatchFile(lib, writeBatch(out, batches), gen));
}

const batch = (entries: ScanEntry[], dirsDone: ScanBatch["dirsDone"]): ScanBatch[] => [{ seq: 1, entries, dirsDone }];

/** The catalog's file rows by display path (`dir/name`). */
function rows(): Record<string, Record<string, unknown>> {
  const db = new DatabaseSync(catalogPath(s.serverData, lib), { readOnly: true });
  try {
    const all = db
      .prepare(
        "SELECT f.id, d.relKey AS dir, f.nameNfc, f.state, f.fp, f.collision, f.hardlink, f.renameOf, f.firstSeen, f.missingSince " +
          "FROM files f JOIN dirs d ON d.id = f.dirId ORDER BY f.id",
      )
      .all() as Array<Record<string, unknown>>;
    return Object.fromEntries(all.map((r) => [`${r.dir === "" ? "" : `${r.dir}/`}${r.nameNfc}`, r]));
  } finally {
    db.close();
  }
}

/** The catalog's file rows in id order (twins share a display path). */
function list(): Array<Record<string, unknown>> {
  const db = new DatabaseSync(catalogPath(s.serverData, lib), { readOnly: true });
  try {
    return db.prepare("SELECT id, nameNfc, state, collision FROM files ORDER BY id").all() as Array<Record<string, unknown>>;
  } finally {
    db.close();
  }
}

const dirRow = (relKey: string) => {
  const db = new DatabaseSync(catalogPath(s.serverData, lib), { readOnly: true });
  try {
    return db.prepare("SELECT state, code, emptySince, emptyJob, CAST(mtimeNs AS TEXT) AS mtimeNs, listedGen FROM dirs WHERE relKey = ?").get(relKey) as Record<string, unknown>;
  } finally {
    db.close();
  }
};

const probe = (fileId: number, fp: string, size = 100) =>
  c.indexer.applyProbed(lib, [{ fileId, size, mtimeNs: "1700000000000000000", fp, probeVersion: 1, state: "present" }]);

/** A root with `A/` holding `a.mov` and `b.mov`, listed completely by job `s1`. */
async function seed(): Promise<void> {
  await apply("s1", batch([dirEntry([], "A"), entry(["A"], "a.mov"), entry(["A"], "b.mov")], [done([], 1), done(["A"], 2)]));
}

describe("listings", () => {
  test("incomplete_dir_marks_nothing_missing", async () => {
    await seed();
    // An error listing, an offline library, and a listing whose first entries were lost.
    expect(await apply("s2", batch([entry(["A"], "a.mov")], [done(["A"], 1, { complete: false, code: "TIMEOUT" })]))).toMatchObject({ missing: 0 });
    expect(dirRow("A")).toMatchObject({ state: "unreadable", code: "TIMEOUT" });
    await run(c.indexer.setOffline(lib, true));
    expect(await apply("s3", batch([entry(["A"], "a.mov")], [done(["A"], 1)]))).toMatchObject({ missing: 0, incomplete: 1 });
    await run(c.indexer.setOffline(lib, false));
    expect(await apply("s4", batch([entry(["A"], "a.mov")], [done(["A"], 2)]))).toMatchObject({ missing: 0, incomplete: 1 });
    expect(Object.values(rows()).map((r) => r.state)).toEqual(["present", "present"]);
    // A complete one marks the absent row and makes the directory present again.
    expect(await apply("s5", batch([entry(["A"], "a.mov")], [done(["A"], 1)]))).toMatchObject({ missing: 1 });
    expect(rows()["A/b.mov"]).toMatchObject({ state: "missing", missingSince: now });
    expect(dirRow("A")).toMatchObject({ state: "present", code: null, listedGen: 1 });
  });

  test("the offline flag survives a restart", async () => {
    await seed();
    await run(c.indexer.setOffline(lib, true));
    await c.shutdown();
    c = await threads(s, { clock: () => now });
    await run(c.indexer.open(lib));
    expect(await apply("s2", batch([], [done(["A"], 0)]))).toMatchObject({ missing: 0, incomplete: 1 });
  });

  test("dir_drop_to_zero_needs_confirmation", async () => {
    await seed();
    const t0 = now + 1000;
    expect(await apply("p1", batch([], [done(["A"], 0, { mtimeNs: "1700000000000000009" })]))).toMatchObject({ suspect: 1, missing: 0 });
    expect(dirRow("A")).toMatchObject({ code: "DIR_EMPTY_SUSPECT", emptySince: t0, emptyJob: "p1", mtimeNs: "1700000000000000001" });
    now = t0 + 29_000; // another job, too soon
    expect(await apply("p2", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 1, missing: 0 });
    now = t0 + 70_000; // the first job again: no confirmation
    expect(await apply("p1", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 1, missing: 0 });
    expect(dirRow("A")).toMatchObject({ emptySince: t0, emptyJob: "p1" });
    // Another job, at least 60 s after the first empty listing.
    expect(await apply("p3", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 0, missing: 2 });
    expect(dirRow("A")).toMatchObject({ code: null, emptySince: null, emptyJob: null });
  });

  test("a listing with entries clears the suspicion", async () => {
    await seed();
    await apply("p1", batch([], [done(["A"], 0)]));
    await apply("p2", batch([entry(["A"], "a.mov"), entry(["A"], "b.mov")], [done(["A"], 2)]));
    expect(dirRow("A")).toMatchObject({ emptySince: null, code: null });
    now += 120_000;
    expect(await apply("p3", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 1, missing: 0 });
  });

  test("root_empty_suspect_aborts_before_marks", async () => {
    await seed();
    const before = rows();
    const out = join(s.jobsRoot, "s2", "out");
    mkdirSync(out, { recursive: true });
    const file = writeBatch(out, [{ seq: 1, entries: [], dirsDone: [done([], 0), done(["A"], 0)] }]);
    expect(await failCode(c.indexer.applyBatchFile(lib, file, 2))).toBe("ROOT_EMPTY_SUSPECT");
    expect(rows()).toEqual(before);
    expect(dirRow("")).toMatchObject({ listedGen: 1 });
  });

  test("a poll and a full scan listing one directory never undo each other", async () => {
    await seed();
    // The full scan's listing of A starts (a.mov only, b.mov deleted since), then a poll lists
    // A with a new c.mov and closes, then the full scan closes its older listing.
    await apply("full", batch([entry(["A"], "a.mov")], []));
    await apply("poll", batch([entry(["A"], "a.mov"), entry(["A"], "c.mov")], [done(["A"], 2)]));
    await apply("full", batch([], [done(["A"], 1)]));
    expect(Object.fromEntries(Object.entries(rows()).map(([k, r]) => [k, r.state]))).toEqual({
      "A/a.mov": "present",
      "A/b.mov": "missing",
      "A/c.mov": "present",
    });
  });

  test("a subdirectory absent from a listing goes missing with its subtree", async () => {
    await apply("s1", batch([dirEntry([], "A"), dirEntry(["A"], "B"), entry(["A", "B"], "x.mov")], [done([], 1), done(["A"], 1), done(["A", "B"], 1)]));
    expect(await apply("s2", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 1 });
    await apply("s3", batch([entry(["A"], "y.mov")], [done(["A"], 1)]));
    expect(dirRow("A/B")).toMatchObject({ state: "missing" });
    expect(rows()["A/B/x.mov"]).toMatchObject({ state: "missing" });
  });

  test("a placeholder directory's rows become cloudOnly, never missing", async () => {
    await seed();
    await apply("s2", batch([dirEntry([], "A", { placeholder: "dataless" })], [done([], 1), done(["A"], 0, { complete: false, code: "PLACEHOLDER" })]));
    expect(Object.values(rows()).map((r) => r.state)).toEqual(["cloudOnly", "cloudOnly"]);
  });
});

describe("renames, hard links and moves", () => {
  const tree = (where: "A" | "B", name: string) => [withDev(entry([where], name, { fileId: id(7) }))];

  test("rename_keeps_id_and_fp", async () => {
    await apply("s1", batch([dirEntry([], "A"), dirEntry([], "B"), ...tree("A", "a.mov")], [done([], 2), done(["A"], 1), done(["B"], 0)]));
    const old = rows()["A/a.mov"]!;
    await run(probe(old.id as number, "fp1:a"));
    // B listed before A: the new row waits on `renameOf`, then A's listing resolves it.
    await apply("s2", batch(tree("B", "z.mov"), [done(["B"], 1)]));
    expect(rows()["B/z.mov"]).toMatchObject({ renameOf: old.id, hardlink: 1 });
    expect(await apply("s2", batch([], [done(["A"], 0)]))).toMatchObject({ suspect: 1 });
    now += 61_000;
    expect(await apply("s3", batch([entry(["A"], "other.mov")], [done(["A"], 1)]))).toMatchObject({ renamed: 1 });
    expect(rows()["B/z.mov"]).toMatchObject({ id: old.id, fp: "fp1:a", firstSeen: old.firstSeen, state: "present", hardlink: 0, renameOf: null });
    expect(rows()["A/a.mov"]).toBeUndefined();
    // Back to A, with B closed first this time: the missing row is taken over at once.
    await apply("s4", batch([], [done(["B"], 0)]));
    now += 61_000;
    expect(await apply("s5", batch([], [done(["B"], 0)]))).toMatchObject({ missing: 1 });
    expect(await apply("s6", batch([entry(["A"], "other.mov"), ...tree("A", "a.mov")], [done(["A"], 2)]))).toMatchObject({ renamed: 1, added: 0 });
    expect(rows()["A/a.mov"]).toMatchObject({ id: old.id, fp: "fp1:a", state: "present" });
    const hits = await run(c.reader.search(lib, { query: "a.mov", limit: 10 }));
    expect(hits.hits.map((h) => h.id)).toContain(old.id);
    expect((await run(c.reader.search(lib, { query: "z", limit: 10 }))).hits).toEqual([]);
  });

  test("hard_links_stay_two_rows", async () => {
    await apply("s1", batch([dirEntry([], "A"), ...tree("A", "a.mov"), ...tree("A", "link.mov")], [done([], 1), done(["A"], 2)]));
    const r = rows();
    expect(r["A/a.mov"]).toMatchObject({ hardlink: 1, renameOf: null, state: "present" });
    expect(r["A/link.mov"]).toMatchObject({ hardlink: 1, renameOf: null, state: "present" });
    // Across directories: the old directory lists its name too.
    await apply("s2", batch([dirEntry([], "B"), ...tree("B", "b.mov")], [done(["B"], 1)]));
    expect(rows()["B/b.mov"]).toMatchObject({ renameOf: r["A/a.mov"]!.id, hardlink: 1 });
    await apply("s2", batch([...tree("A", "a.mov"), ...tree("A", "link.mov")], [done(["A"], 2)]));
    expect(Object.values(rows()).map((x) => [x.hardlink, x.renameOf])).toEqual([[1, null], [1, null], [1, null]]);
    // One name removed: the other two stay linked; a second removed: the last is alone.
    await apply("s3", batch([...tree("A", "a.mov")], [done(["A"], 1)]));
    expect(Object.values(rows()).map((x) => x.hardlink)).toEqual([1, 0, 1]);
    await apply("s4", batch([], [done(["B"], 0)]));
    now += 61_000;
    await apply("s5", batch([], [done(["B"], 0)]));
    expect(rows()["A/a.mov"]).toMatchObject({ hardlink: 0 });
  });

  test("zero and absent file ids are no hints", async () => {
    await apply("s1", batch([dirEntry([], "A"), withDev(entry(["A"], "a.mov", { fileId: Buffer.alloc(8).toString("base64") }))], [done([], 1), done(["A"], 1)]));
    await apply("s1", batch([withDev(entry(["A"], "b.mov", { fileId: Buffer.alloc(8).toString("base64") }))], [done(["A"], 1)]));
    expect(rows()["A/b.mov"]).toMatchObject({ renameOf: null, hardlink: 0 });
  });

  test("move_by_fingerprint_merges", async () => {
    await apply("s1", batch([dirEntry([], "A"), dirEntry([], "B"), entry(["A"], "a.mov")], [done([], 2), done(["A"], 1), done(["B"], 0)]));
    const old = rows()["A/a.mov"]!;
    await run(probe(old.id as number, "fp1:same"));
    await apply("s2", batch([entry(["A"], "x.mov"), entry(["B"], "copy.mov")], [done(["A"], 1), done(["B"], 1)]));
    const fresh = rows()["B/copy.mov"]!;
    expect(rows()["A/a.mov"]).toMatchObject({ state: "missing" });
    expect(await run(probe(fresh.id as number, "fp1:same"))).toEqual({ updated: 1, stale: 0, moved: [{ from: fresh.id, to: old.id }] });
    expect(rows()["B/copy.mov"]).toMatchObject({ id: old.id, firstSeen: old.firstSeen, state: "present", fp: "fp1:same" });
    expect(rows()["A/a.mov"]).toBeUndefined();
    // A second fingerprint of a row is no move.
    const x = rows()["A/x.mov"]!;
    await run(probe(x.id as number, "fp1:x"));
    await apply("s3", batch([entry(["A"], "x2.mov")], [done(["A"], 1)]));
    expect(await run(probe(rows()["A/x2.mov"]!.id as number, "fp1:other"))).toMatchObject({ moved: [] });
  });

  test("missing_reappears_present", async () => {
    await seed();
    const b = rows()["A/b.mov"]!;
    await run(probe(b.id as number, "fp1:b"));
    await apply("s2", batch([entry(["A"], "a.mov")], [done(["A"], 1)]));
    expect(rows()["A/b.mov"]).toMatchObject({ state: "missing" });
    expect(await apply("s3", batch([entry(["A"], "a.mov"), entry(["A"], "b.mov")], [done(["A"], 2)]))).toMatchObject({ reappeared: 1 });
    expect(rows()["A/b.mov"]).toMatchObject({ id: b.id, state: "present", fp: "fp1:b", missingSince: null });
    // Back with another size: a change, the fingerprint is gone.
    await apply("s4", batch([entry(["A"], "a.mov")], [done(["A"], 1)]));
    expect(await apply("s5", batch([entry(["A"], "a.mov"), entry(["A"], "b.mov", { size: 5 })], [done(["A"], 2)]))).toMatchObject({ changed: 1 });
    expect(rows()["A/b.mov"]).toMatchObject({ id: b.id, state: "present", fp: null });
  });
});

describe("collisions", () => {
  const ci = (dir: string[], name: string, o: Parameters<typeof entry>[2] = {}): ScanEntry => {
    const e = entry(dir, name, o);
    return { ...e, relKey: e.relKey.toLowerCase().normalize("NFC") };
  };

  test("twins_keep_both_rows", async () => {
    const lossyA = Uint8Array.from([0xff, 0x2e, 0x6d]);
    const lossyB = Uint8Array.from([0xfe, 0x2e, 0x6d]);
    const lossy = (raw: Uint8Array) => ({ ...ci([], "�.m", { raw }), nameLossy: true });
    const entries = [
      ci([], "Caf\u00e9.mov"), ci([], "Cafe\u0301.mov"), ci([], "CAF\u00c9.MOV"), lossy(lossyA), lossy(lossyB), ci([], "plain.mov"),
    ];
    const counts = await apply("s1", batch(entries, [done([], entries.length)]));
    expect(counts).toMatchObject({ added: 6, collisions: 5 });
    const r = list();
    expect(r).toHaveLength(6);
    // The NFC name meets its NFD twin (nfc) and the upper-case one (case); and so on.
    expect(r.map((x) => x.collision)).toEqual([1 | 2, 1 | 2, 2, 4, 4, 0]);
    expect((await run(c.reader.counts(lib))).collisions).toBe(5);
    // A twin gone: the bits are recomputed when the listing closes.
    await apply("s2", batch([ci([], "Caf\u00e9.mov"), ci([], "plain.mov")], [done([], 2)]));
    expect(list().filter((x) => x.state === "present").map((x) => x.collision)).toEqual([0, 0]);
  });

  test("ambiguous_lookup_is_path_ambiguous", async () => {
    await apply("s1", batch([entry([], "Caf\u00e9.mov"), entry([], "Cafe\u0301.mov")], [done([], 2)]));
    const ids = list().map((r) => r.id);
    expect(ids).toHaveLength(2);
    const err = await Effect.runPromise(Effect.flip(c.reader.resolve(lib, "Caf\u00e9.mov")));
    expect(err).toMatchObject({ code: "PATH_AMBIGUOUS", details: { fileIds: ids } });
    // One twin missing: the live one answers alone.
    await apply("s2", batch([entry([], "Caf\u00e9.mov")], [done([], 1)]));
    expect(await run(c.reader.resolve(lib, "Caf\u00e9.mov"))).toMatchObject({ id: ids[0], rel: rel("Caf\u00e9.mov") });
    expect(await failCode(c.reader.resolve(lib, "nothing.mov"))).toBe("FILE_NOT_FOUND");
  });

  test("collision_never_fails_a_batch", async () => {
    const names = Array.from({ length: 300 }, (_, i) => (i % 2 === 0 ? "dup.mov" : "DUP.mov"));
    // 300 raw names, one key: every row kept, all flagged, nothing refused.
    const entries = names.map((n, i) => ({ ...ci([], n, { raw: Buffer.from(`${n}${i}`) }), relKey: "dup.mov" }));
    const counts = await apply("s1", batch(entries, [done([], entries.length)]));
    expect(counts).toMatchObject({ added: 300, collisions: 300 });
    expect((await run(c.reader.counts(lib))).collisions).toBe(300);
  });
});

test("missing_purged_after_90_days_in_batches", async () => {
  const n = 1201;
  const entries = [dirEntry([], "A"), dirEntry(["A"], "B"), ...Array.from({ length: n }, (_, i) => entry(["A", "B"], `f${i}.mov`))];
  await apply("s1", batch(entries, [done([], 1), done(["A"], 1), done(["A", "B"], n)]));
  await apply("s2", batch([], [done([], 0)])).catch(() => undefined);
  now += 1000;
  await apply("s3", batch([dirEntry([], "keep")], [done([], 1)]));
  expect((await run(c.reader.counts(lib))).files).toEqual({ missing: n });
  const before = (await run(c.indexer.stats())).transactions;
  expect(await run(c.indexer.purgeMissing(lib, now - 1))).toEqual({ deleted: 0, dirs: 0 });
  const purged = await run(c.indexer.purgeMissing(lib, now + 90 * 86_400_000));
  expect(purged).toEqual({ deleted: n, dirs: 2 });
  const stats = await run(c.indexer.stats());
  expect(stats.transactions - before).toBeGreaterThanOrEqual(Math.ceil(n / 500) + 1);
  expect((await run(c.reader.counts(lib))).dirs).toEqual({ present: 1 });
});
