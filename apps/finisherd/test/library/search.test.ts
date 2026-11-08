// Name search and fingerprint lookup (plan 5D Task 12, query core; Decisions 43, 44): prefix
// terms over names and directories, filters, snapshot paging across libraries, the score-free
// keyset, escaping, lookup and its hash candidates, offline marking. The freshness part
// (`search_waits_for_freshness_then_stale`) waits for Task 10.
import { DatabaseSync } from "node:sqlite";
import type { JobView } from "@finisher/contracts";
import type { ScanBatch, ScanEntry } from "@finisher/core-types";
import { Effect } from "effect";
import fc from "fast-check";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { Catalogs } from "../../src/library/catalog/client.ts";
import { Snapshots } from "../../src/library/catalog/core/snapshot.ts";
import { type HashSubmit, type LookupRequest, lookupLibraries } from "../../src/library/lookup.ts";
import { type SearchRequest, searchLibraries } from "../../src/library/search.ts";
import { type Scratch, applied, dirEntry, done, entry, failCode, newLib, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";

let s: Scratch;
beforeEach(() => {
  s = scratch();
});
afterEach(async () => {
  await stopAll();
  s.cleanup();
});

interface FileSpec {
  readonly path: string;
  readonly size?: number;
}

/** One complete scan of `files` (slash-separated paths, listed breadth-first as a walker does). */
function treeBatch(files: ReadonlyArray<FileSpec | string>, seq = 1): ScanBatch {
  const dirs = new Map<string, { subdirs: Set<string>; files: Array<{ name: string; size?: number }> }>();
  const at = (d: string[]) => {
    const key = d.join("/");
    let v = dirs.get(key);
    if (v === undefined) dirs.set(key, (v = { subdirs: new Set(), files: [] }));
    return v;
  };
  at([]);
  for (const f of files) {
    const spec = typeof f === "string" ? { path: f } : f;
    const parts = spec.path.split("/");
    for (let i = 0; i < parts.length - 1; i++) at(parts.slice(0, i)).subdirs.add(parts[i]!);
    at(parts.slice(0, -1)).files.push({ name: parts.at(-1)!, ...(spec.size === undefined ? {} : { size: spec.size }) });
  }
  const keys = [...dirs.keys()].sort((a, b) => a.split("/").length - b.split("/").length || (a < b ? -1 : 1));
  const entries: ScanEntry[] = [];
  const dirsDone = [];
  for (const key of keys) {
    const dir = key === "" ? [] : key.split("/");
    const v = dirs.get(key)!;
    const list = [...[...v.subdirs].map((n) => dirEntry(dir, n)), ...v.files.map((f) => entry(dir, f.name, f.size === undefined ? {} : { size: f.size }))];
    entries.push(...list);
    dirsDone.push(done(dir, list.length));
  }
  return { seq, entries, dirsDone };
}

/** A clock that moves 10 ms on every reading, so one scan always starts after the previous one. */
function ticking(): () => number {
  let t = 1_000_000;
  return () => (t += 10);
}
const start = (opts: Parameters<typeof threads>[1] = {}) => threads(s, { clock: ticking(), ...opts });

let gen = 0;
/** Applies a complete scan of `files` to `lib`. */
async function scan(c: Catalogs, lib: string, files: ReadonlyArray<FileSpec | string>): Promise<void> {
  gen++;
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [treeBatch(files)]), applied(gen)));
}

async function open(c: Catalogs, files: ReadonlyArray<FileSpec | string>): Promise<string> {
  const lib = newLib();
  await run(c.indexer.open(lib));
  await scan(c, lib, files);
  return lib;
}

interface ProbeSpec {
  readonly kind?: string;
  readonly durationMs?: number;
  readonly width?: number;
  readonly height?: number;
  readonly fp?: string;
  readonly state?: "present" | "probeFailed" | "skipped";
}

/** Stores a probe result for the file at `relKey`. */
async function probe(c: Catalogs, lib: string, relKey: string, p: ProbeSpec): Promise<number> {
  const row = await run(c.reader.resolve(lib, relKey));
  await run(
    c.indexer.applyProbed(lib, [
      { fileId: row.id, size: row.size, mtimeNs: row.mtimeNs, probeVersion: 1, state: p.state ?? "present", ...(p.kind === undefined ? {} : { kind: p.kind }),
        ...(p.durationMs === undefined ? {} : { durationMs: p.durationMs }), ...(p.width === undefined ? {} : { width: p.width }),
        ...(p.height === undefined ? {} : { height: p.height }), ...(p.fp === undefined ? {} : { fp: p.fp }) },
    ]),
  );
  return row.id;
}

const find = (c: Catalogs, libraries: string[], req: Partial<SearchRequest> = {}) => run(searchLibraries(c.reader, { libraries, ...req }));
const names = async (c: Catalogs, libs: string[], query: string, req: Partial<SearchRequest> = {}): Promise<string[]> =>
  (await find(c, libs, { query, limit: 200, ...req })).hits.map((h) => h.display);
const sorted = (xs: string[]) => [...xs].sort();

/** Every page of a search, following `next`. */
async function allPages(c: Catalogs, libs: string[], req: Partial<SearchRequest>, limit: number) {
  const hits = [];
  let cursor: string | undefined;
  let truncated = false;
  do {
    const page = await find(c, libs, { ...req, limit, ...(cursor === undefined ? {} : { cursor }) });
    hits.push(...page.hits);
    truncated = page.truncated;
    cursor = page.next;
  } while (cursor !== undefined);
  return { hits, truncated };
}

describe("terms", () => {
  test("terms_are_prefix_and_diacritic_insensitive", async () => {
    const c = await start();
    // NFD names are what macOS writes; the index holds their NFC form.
    const lib = await open(c, [
      "Café_Interview.mov",
      "Cafe\u0301_Wedding.mov".normalize("NFD"),
      "Ünterwegs.wav",
      "Свадьба_2020.mp4",
      "Йод.mov".normalize("NFD"),
      "インタビュー_01.mov",
      "ไทย_กาแฟ.mov",
      "한국어_영상.mov".normalize("NFD"),
      "Straße.jpg",
      "interview_take3.final.mov",
      "Shoot Day 2/clip_a.mov",
      "Shoot Day 2/clip_b.mov",
      "Other/clip_c.mov",
    ]);
    const q = (query: string) => names(c, [lib], query).then(sorted);
    const cafes = ["Café_Interview.mov", "Café_Wedding.mov"];
    for (const query of ["cafe", "CAFÉ", "caf", "Cafe\u0301", "café".normalize("NFD"), "  cafe  "]) expect(await q(query)).toEqual(cafes);
    expect(await q("cafe wedding")).toEqual(["Café_Wedding.mov"]);
    expect(await q("cafe interview")).toEqual(["Café_Interview.mov"]);
    expect(await q("wedding interview")).toEqual([]);
    expect(await q("ünter")).toEqual(["Ünterwegs.wav"]);
    expect(await q("UNTER")).toEqual(["Ünterwegs.wav"]);
    // Scripts: Cyrillic folds case; a decomposed Й (И + breve) finds its composed twin.
    expect(await q("СВАД")).toEqual(["Свадьба_2020.mp4"]);
    expect(await q("свадьба 2020")).toEqual(["Свадьба_2020.mp4"]);
    expect(await q("и\u0306од")).toEqual(["Йод.mov"]);
    // Kana and Hangul: queries are composed first; a Han/kana run is one term, found by its prefix.
    expect(await q("インタ")).toEqual(["インタビュー_01.mov"]);
    expect(await q("インタヒ\u3099ュー")).toEqual(["インタビュー_01.mov"]);
    expect(await q("ビュー")).toEqual([]);
    expect(await q("한국".normalize("NFD"))).toEqual(["한국어_영상.mov"]);
    expect(await q("영상")).toEqual(["한국어_영상.mov"]);
    expect(await q("ไทย")).toEqual(["ไทย_กาแฟ.mov"]);
    expect(await q("กาแฟ")).toEqual(["ไทย_กาแฟ.mov"]);
    // ß is a letter of its own, not folded to ss.
    expect(await q("straße")).toEqual(["Straße.jpg"]);
    expect(await q("strasse")).toEqual([]);
    // Directory names are searched too, and the display path keeps the names' case.
    expect(await q("shoot")).toEqual(["Shoot Day 2/clip_a.mov", "Shoot Day 2/clip_b.mov"]);
    expect(await q("shoot clip_b")).toEqual(["Shoot Day 2/clip_b.mov"]);
    expect(await q("clip").then((x) => x.length)).toBe(3);
    // A name in the root has no directory prefix.
    const hit = (await find(c, [lib], { query: "take3" })).hits[0]!;
    expect(hit).toMatchObject({ library: lib, display: "interview_take3.final.mov", relKey: "interview_take3.final.mov", state: "present", collision: false, offline: false });
    expect(hit.score).toBeLessThan(0);
  });

  test("separators_split_names", async () => {
    const c = await start();
    const lib = await open(c, ["interview_take3.final.mov", "Interview-Take4.FINAL.MOV", "take3x.wav"]);
    const q = (query: string) => names(c, [lib], query).then(sorted);
    expect(await q("take")).toEqual(["Interview-Take4.FINAL.MOV", "interview_take3.final.mov", "take3x.wav"]);
    expect(await q("final mov")).toEqual(["Interview-Take4.FINAL.MOV", "interview_take3.final.mov"]);
    expect(await q("final.mov")).toEqual(["Interview-Take4.FINAL.MOV", "interview_take3.final.mov"]);
    expect(await q("interview_take3.final.mov")).toEqual(["interview_take3.final.mov"]);
    expect(await q("take3")).toEqual(["interview_take3.final.mov", "take3x.wav"]);
    // Tokens, not substrings: only a term's start matches.
    expect(await q("ake")).toEqual([]);
    expect(await q("3")).toEqual([]);
    expect(await q("mo")).toEqual(["Interview-Take4.FINAL.MOV", "interview_take3.final.mov"]);
  });

  test("fts_syntax_in_queries_is_escaped", async () => {
    const c = await start();
    const lib = await open(c, ["and.mov", "or.mov", "near.mov", "not_here.mov", "a b.mov", "name.mov", "star.mov"]);
    const q = (query: string) => names(c, [lib], query);
    // Operators are words; quotes, colons, parentheses, carets and dashes separate.
    expect(await q("AND")).toEqual(["and.mov"]);
    expect(await q("and*")).toEqual(["and.mov"]);
    expect(await q('"and')).toEqual(["and.mov"]);
    expect(await q("-and")).toEqual(["and.mov"]);
    expect(await q("^and")).toEqual(["and.mov"]);
    expect(await q("name:and")).toEqual([]);
    expect(await q("{name dir}: name")).toEqual([]);
    expect(await q("NOT here")).toEqual(["not_here.mov"]);
    expect(await q("NEAR(a b)")).toEqual([]);
    expect(await q("a OR b")).toEqual([]);
    expect(await q('x" OR "star')).toEqual([]);
    expect(await q('"); DROP TABLE files; --')).toEqual([]);
    // Nothing but syntax is a query without terms: it lists, newest first.
    for (const none of ["", "*", "()", '"', "-", "\u0301", "!!!"]) expect(await q(none)).toHaveLength(7);
    expect(await names(c, [lib], "a b")).toEqual(["a b.mov"]);
    // Limits on the query itself.
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "x ".repeat(17) }))).toBe("SEARCH_QUERY_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "x".repeat(513) }))).toBe("SEARCH_QUERY_INVALID");
    expect(await names(c, [lib], "x ".repeat(16))).toEqual([]);
    for (const limit of [0, 201, 1.5]) expect(await failCode(searchLibraries(c.reader, { libraries: [lib], limit }))).toBe("INVALID_REQUEST");
    expect(await failCode(searchLibraries(c.reader, { libraries: [] }))).toBe("INVALID_REQUEST");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], duration: { min: 5, max: 1 } }))).toBe("INVALID_REQUEST");
    expect(await failCode(searchLibraries(c.reader, { libraries: [newLib()] }))).toBe("CATALOG_NOT_OPEN");
  });

  test("ranks_names_above_directories", async () => {
    const c = await start();
    // `name` weighs 4.0, `dir` 1.0: a file named for the term beats one that only sits in a folder of that name.
    const lib = await open(c, ["wedding/clip_1.mov", "wedding/clip_2.mov", "other/wedding_toast.mov"]);
    const hits = (await find(c, [lib], { query: "wedding" })).hits;
    expect(hits.map((h) => h.display)[0]).toBe("other/wedding_toast.mov");
    expect(hits).toHaveLength(3);
    expect(hits.map((h) => h.score)).toEqual([...hits.map((h) => h.score)].sort((a, b) => a - b));
  });
});

describe("filters", () => {
  test("filters_kind_duration_size_since_state", async () => {
    let now = 1000;
    const c = await threads(s, { clock: () => now });
    const lib = newLib();
    await run(c.indexer.open(lib));
    const files: FileSpec[] = [
      { path: "clip_a.mov", size: 1000 }, { path: "clip_b.mov", size: 5000 }, { path: "clip_c.wav", size: 300 },
      { path: "clip_d.jpg", size: 200 }, { path: "clip_e.mov", size: 100 }, { path: "clip_f.mov", size: 100 },
    ];
    await scan(c, lib, files);
    await probe(c, lib, "clip_a.mov", { kind: "media", durationMs: 1000, width: 1920, height: 1080, fp: "fp1:a" });
    await probe(c, lib, "clip_b.mov", { kind: "media", durationMs: 5000, width: 3840, height: 2160, fp: "fp1:b" });
    await probe(c, lib, "clip_c.wav", { kind: "audio", durationMs: 3000, fp: "fp1:c" });
    await probe(c, lib, "clip_d.jpg", { kind: "image", width: 1000, height: 1000, fp: "fp1:d" });
    await probe(c, lib, "clip_f.mov", { state: "probeFailed" });
    now = 2000;
    const withG = [...files, { path: "clip_g.mov", size: 7000 }];
    await scan(c, lib, withG);
    const q = (req: Partial<SearchRequest>) => names(c, [lib], "clip", req).then(sorted);
    expect(await q({})).toHaveLength(7);
    expect(await q({ kind: "media" })).toEqual(["clip_a.mov", "clip_b.mov"]);
    expect(await q({ kind: "image" })).toEqual(["clip_d.jpg"]);
    expect(await q({ duration: { min: 2000 } })).toEqual(["clip_b.mov", "clip_c.wav"]);
    expect(await q({ duration: { max: 2000 } })).toEqual(["clip_a.mov"]);
    expect(await q({ duration: { min: 1000, max: 3000 } })).toEqual(["clip_a.mov", "clip_c.wav"]);
    expect(await q({ width: { min: 1920 } })).toEqual(["clip_a.mov", "clip_b.mov"]);
    expect(await q({ width: { max: 1000 } })).toEqual(["clip_d.jpg"]);
    expect(await q({ height: { min: 1080, max: 1080 } })).toEqual(["clip_a.mov"]);
    expect(await q({ size: { min: 1000 } })).toEqual(["clip_a.mov", "clip_b.mov", "clip_g.mov"]);
    expect(await q({ size: { max: 200 } })).toEqual(["clip_d.jpg", "clip_e.mov", "clip_f.mov"]);
    expect(await q({ since: 1500 })).toEqual(["clip_g.mov"]);
    expect(await q({ since: 1000 })).toHaveLength(7);
    expect(await q({ state: "probeFailed" })).toEqual(["clip_f.mov"]);
    expect(await q({ state: "present" })).toHaveLength(6);
    expect(await q({ kind: "media", duration: { min: 2000 }, since: 500 })).toEqual(["clip_b.mov"]);
    expect(await q({ kind: "media", width: { min: 5000 } })).toEqual([]);
    // Without terms the same filters apply.
    expect(await names(c, [lib], "", { kind: "audio" })).toEqual(["clip_c.wav"]);
    // A file that left the directory is `missing`: out of searches unless asked for.
    now = 3000;
    await scan(c, lib, withG.filter((f) => f.path !== "clip_e.mov"));
    expect(await q({})).toHaveLength(6);
    expect(await q({ state: "missing" })).toEqual(["clip_e.mov"]);
    expect(await names(c, [lib], "", { state: "missing" })).toEqual(["clip_e.mov"]);
  });

  test("offline_hits_marked", async () => {
    const c = await start();
    const a = await open(c, ["clip_1.mov", "clip_2.mov"]);
    const b = await open(c, ["clip_3.mov"]);
    const marks = async (query = "clip") => Object.fromEntries((await find(c, [a, b], { query })).hits.map((h) => [h.display, h.offline]));
    expect(await marks()).toEqual({ "clip_1.mov": false, "clip_2.mov": false, "clip_3.mov": false });
    // An offline library stays searchable (Decision 18), its hits marked.
    await run(c.indexer.setOffline(a, true));
    expect(await marks()).toEqual({ "clip_1.mov": true, "clip_2.mov": true, "clip_3.mov": false });
    expect(await marks("")).toEqual({ "clip_1.mov": true, "clip_2.mov": true, "clip_3.mov": false });
    await run(c.indexer.setOffline(a, false));
    expect(await marks()).toEqual({ "clip_1.mov": false, "clip_2.mov": false, "clip_3.mov": false });
  });
});

describe("paging", () => {
  test("merge_orders_by_score_then_library_then_row", async () => {
    const c = await start();
    const libs = [await open(c, ["take_1.mov", "take_2.mov"]), await open(c, ["take_1.mov", "take_2.mov"])].sort();
    const hits = (await find(c, [libs[1]!, libs[0]!], { query: "take" })).hits;
    // Equal documents score equally: library id, then row id.
    expect(hits.map((h) => [h.library, h.display])).toEqual([[libs[0], "take_1.mov"], [libs[0], "take_2.mov"], [libs[1], "take_1.mov"], [libs[1], "take_2.mov"]]);
    // A better match in the later library comes first whatever the library.
    const better = await open(c, ["take.mov"]);
    const all = (await find(c, [...libs, better], { query: "take" })).hits;
    expect(all.map((h) => h.score)).toEqual([...all.map((h) => h.score)].sort((x, y) => x - y));
  });

  test("snapshot_cursor_stable_while_catalogs_change", async () => {
    const c = await start();
    const lib = await open(c, Array.from({ length: 30 }, (_, i) => `take_${i}${i % 3 === 0 ? "_take" : ""}.mov`));
    const key = (h: { library: string; fileId: number }) => `${h.library}:${h.fileId}`;
    const reference = (await allPages(c, [lib], { query: "take" }, 200)).hits.map(key);
    expect(reference).toHaveLength(30);
    const first = await find(c, [lib], { query: "take", limit: 10 });
    expect(first.next).toBeDefined();
    // Writes change the corpus statistics, drop matches and add better ones ...
    await scan(c, lib, [...Array.from({ length: 300 }, (_, i) => `other_${i}.mov`), ...Array.from({ length: 30 }, (_, i) => `take_${i}${i % 3 === 0 ? "" : "_take"}.mov`)]);
    const fresh = (await allPages(c, [lib], { query: "take" }, 200)).hits.map(key);
    expect(fresh).not.toEqual(reference);
    // ... and the cursor still pages the ranking the first page was cut from.
    const rest = await allPages(c, [lib], { query: "take", cursor: first.next! }, 10);
    expect([...first.hits, ...rest.hits].map(key)).toEqual(reference);
  });

  test("snapshot pages never repeat or skip while catalogs change (property)", async () => {
    const c = await start();
    const word = fc.constantFrom("take", "clip", "interview", "take_take", "clip_take");
    const world = fc.uniqueArray(fc.tuple(word, fc.nat(999)), { selector: ([w, n]) => `${w}${n}`, minLength: 12, maxLength: 60 });
    const step = fc.oneof(
      fc.record({ op: fc.constant("add" as const), lib: fc.nat(2), names: fc.uniqueArray(fc.tuple(word, fc.nat(999)), { selector: ([w, n]) => `${w}${n}`, minLength: 1, maxLength: 40 }) }),
      fc.record({ op: fc.constant("drop" as const), lib: fc.nat(2), count: fc.integer({ min: 1, max: 15 }) }),
    );
    const RUNS = Number(process.env.FIN_SEARCH_RUNS ?? 25);
    let paged = 0;
    let shifted = 0;
    const sameOrder = (x: string[], y: string[]) => x.length === y.length && x.every((v, i) => v === y[i]);
    await fc.assert(
      fc.asyncProperty(fc.record({ libs: fc.array(world, { minLength: 1, maxLength: 3 }), page: fc.integer({ min: 1, max: 9 }), steps: fc.array(step, { maxLength: 10 }) }), async (cfg) => {
        const names = cfg.libs.map((w) => new Set(w.map(([x, n]) => `${x}${n}.mov`)));
        const ids: string[] = [];
        for (const set of names) ids.push(await open(c, [...set]));
        const q = "take";
        // The reference: the whole ranking before anything changes.
        const reference = (await allPages(c, ids, { query: q }, 200)).hits.map((h) => `${h.library}:${h.fileId}`);
        const got: string[] = [];
        let cursor: string | undefined;
        let k = 0;
        do {
          const page = await find(c, ids, { query: q, limit: cfg.page, ...(cursor === undefined ? {} : { cursor }) });
          got.push(...page.hits.map((h) => `${h.library}:${h.fileId}`));
          cursor = page.next;
          const st = cfg.steps[k++];
          if (st !== undefined) {
            const i = st.lib % ids.length;
            const set = names[i]!;
            if (st.op === "add") for (const [w, n] of st.names) set.add(`${w}x${n}.mov`);
            else for (const n of [...set].slice(0, Math.min(st.count, set.size - 1))) set.delete(n);
            await scan(c, ids[i]!, [...set]);
          }
        } while (cursor !== undefined);
        expect(got).toEqual(reference);
        expect(new Set(got).size).toBe(got.length);
        // The property has teeth when the world moved under a multi-page search: count the runs where a fresh search differs.
        if (k > 1) paged++;
        if (k > 1 && !sameOrder((await allPages(c, ids, { query: q }, 200)).hits.map((h) => `${h.library}:${h.fileId}`), reference)) shifted++;
      }),
      { numRuns: RUNS },
    );
    expect(paged).toBeGreaterThan(RUNS / 3);
    expect(shifted).toBeGreaterThan(0);
  }, 120_000);

  test("snapshot_ttl_and_lru", async () => {
    const c = await start({ snapshot: { max: 2 } });
    const lib = await open(c, Array.from({ length: 30 }, (_, i) => `a${i}_b${i}_c${i}.mov`));
    const begin = (query: string) => find(c, [lib], { query, limit: 5 });
    const next = (query: string, cursor: string) => find(c, [lib], { query, limit: 5, cursor });
    const p1 = await begin("a");
    const p2 = await begin("b");
    // Using the first one makes the second the least recently used.
    expect((await next("a", p1.next!)).hits).toHaveLength(5);
    const p3 = await begin("c");
    expect((await c.reader.stats().pipe(run)).snapshots).toBe(2);
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "b", limit: 5, cursor: p2.next! }))).toBe("CURSOR_EXPIRED");
    expect((await next("a", p1.next!)).hits).toHaveLength(5);
    expect((await next("c", p3.next!)).hits).toHaveLength(5);
    // A cursor is bound to its query, its libraries and its form.
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "b", limit: 5, cursor: p1.next! }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "a", kind: "media", limit: 5, cursor: p1.next! }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "a", limit: 5, cursor: "k1.e30" }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "a", limit: 5, cursor: "nonsense" }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: [lib], query: "a", limit: 5, cursor: "s1.AAAAAAAAAAAAAAAA.5" }))).toBe("CURSOR_EXPIRED");
    // A search that fits one page keeps nothing.
    const before = (await run(c.reader.stats())).snapshots;
    expect((await find(c, [lib], { query: "a", limit: 200 })).next).toBeUndefined();
    expect((await run(c.reader.stats())).snapshots).toBe(before);
  });

  test("snapshots expire after their idle time (fake clock)", async () => {
    const c = await start();
    const lib = await open(c, Array.from({ length: 40 }, (_, i) => `clip_${i}.mov`));
    const db = new DatabaseSync((await run(c.indexer.open(lib))).path, { readOnly: true });
    try {
      let now = 0;
      let n = 0;
      const snaps = new Snapshots(new Map([[lib, db]]), { ttlMs: 5 * 60_000, now: () => now, token: () => `token${String(++n).padStart(8, "0")}` });
      const args = { query: "clip" };
      const page = (cursor?: string) => snaps.search({ libraries: [lib], args, limit: 10, ...(cursor === undefined ? {} : { cursor }) });
      let p = page();
      expect(snaps.size).toBe(1);
      // Each use renews the lease: five minutes of idleness, not of age.
      now = 4 * 60_000;
      p = page(p.next);
      now = 8 * 60_000;
      p = page(p.next);
      expect(p.hits).toHaveLength(10);
      expect(p.next).toBeDefined();
      now = 8 * 60_000 + 5 * 60_000 - 1;
      expect(snaps.size).toBe(1);
      now = 8 * 60_000 + 5 * 60_000;
      expect(snaps.size).toBe(0);
      expect(() => page(p.next)).toThrowError(expect.objectContaining({ code: "CURSOR_EXPIRED" }));
    } finally {
      db.close();
    }
  });

  test("truncated_past_2000", async () => {
    const c = await start();
    const a = await open(c, [...Array.from({ length: 1500 }, (_, i) => `alpha_${i}.mov`), ...Array.from({ length: 2000 }, (_, i) => `bound_${i}.mov`)]);
    const b = await open(c, Array.from({ length: 700 }, (_, i) => `alpha_${i}.mov`));
    const libs = [a, b];
    const alpha = await allPages(c, libs, { query: "alpha" }, 200);
    expect(alpha.truncated).toBe(true);
    expect(alpha.hits).toHaveLength(2000);
    expect(new Set(alpha.hits.map((h) => `${h.library}:${h.fileId}`)).size).toBe(2000);
    // The best 2,000 of 2,200 by (score, library, row), as a brute-force merge sees them.
    const every = (await Promise.all(libs.map((lib) => run(c.reader.search(lib, { query: "alpha", limit: 2000 })).then((r) => r.hits.map((h) => ({ lib, id: h.id, score: h.score })))))).flat();
    every.sort((x, y) => x.score - y.score || (x.lib < y.lib ? -1 : x.lib > y.lib ? 1 : 0) || x.id - y.id);
    expect(alpha.hits.map((h) => `${h.library}:${h.fileId}`)).toEqual(every.slice(0, 2000).map((h) => `${h.lib}:${h.id}`));
    // Exactly 2,000 matches are not truncated; one more is.
    const bound = await allPages(c, libs, { query: "bound" }, 200);
    expect(bound).toMatchObject({ truncated: false });
    expect(bound.hits).toHaveLength(2000);
    await scan(c, b, [...Array.from({ length: 700 }, (_, i) => `alpha_${i}.mov`), "bound_x.mov"]);
    const more = await allPages(c, libs, { query: "bound" }, 200);
    expect(more.truncated).toBe(true);
    expect(more.hits).toHaveLength(2000);
  }, 60_000);

  test("empty_query_keyset_is_score_free", async () => {
    let now = 100;
    const c = await threads(s, { clock: () => now });
    const libs = [newLib(), newLib()].sort();
    for (const lib of libs) await run(c.indexer.open(lib));
    // Ten files per library, three per clock tick, so times tie across rows and libraries.
    for (let tick = 0; tick < 4; tick++) {
      now = 100 + tick;
      for (const [j, lib] of libs.entries()) {
        await scan(c, lib, Array.from({ length: Math.min(10, (tick + 1) * 3) }, (_, i) => `f${j}_${i}.mov`));
      }
    }
    const expected = async () => {
      const rows = (await Promise.all(libs.map((lib) => run(c.reader.files(lib, { limit: 500 })).then((r) => r.files.map((f) => ({ lib, id: f.id, t: f.lastChange, name: f.nameNfc })))))).flat();
      return rows.sort((x, y) => y.t - x.t || (x.lib < y.lib ? -1 : x.lib > y.lib ? 1 : 0) || x.id - y.id);
    };
    const all = await expected();
    expect(all).toHaveLength(20);
    expect(new Set(all.map((r) => r.t)).size).toBeGreaterThan(1);
    const seen: string[] = [];
    let cursor: string | undefined;
    let k = 0;
    do {
      const page = await find(c, libs, { limit: 3, ...(cursor === undefined ? {} : { cursor }) });
      expect(page.truncated).toBe(false);
      expect(page.hits.every((h) => h.score === 0)).toBe(true);
      seen.push(...page.hits.map((h) => h.display));
      cursor = page.next;
      if (k++ === 1) {
        // New files appear above the cursor (never repeated below it), a deleted one below it is not skipped over.
        now = 500;
        await scan(c, libs[0]!, [...Array.from({ length: 10 }, (_, i) => `f0_${i}.mov`).filter((n) => n !== "f0_0.mov"), "new_1.mov", "new_2.mov"]);
      }
    } while (cursor !== undefined);
    // Nothing was kept in the reader and no row is repeated.
    expect((await run(c.reader.stats())).snapshots).toBe(0);
    expect(new Set(seen).size).toBe(seen.length);
    // Every page was a continuation: the first six are the original order, the rest follow it minus the deleted file.
    const original = all.map((r) => r.name);
    expect(seen.slice(0, 6)).toEqual(original.slice(0, 6));
    expect(seen.slice(6)).toEqual(original.slice(6).filter((n) => n !== "f0_0.mov"));
    expect(seen).not.toContain("new_1.mov");
    // Filters bind the cursor.
    const p1 = await find(c, libs, { limit: 3 });
    expect(await failCode(searchLibraries(c.reader, { libraries: libs, limit: 3, kind: "media", cursor: p1.next! }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: libs, limit: 3, query: "f0", cursor: p1.next! }))).toBe("CURSOR_INVALID");
    expect(await failCode(searchLibraries(c.reader, { libraries: libs, limit: 3, cursor: "k1.not-json" }))).toBe("CURSOR_INVALID");
  });
});

describe("lookup", () => {
  const job = (id: string): JobView => ({ id }) as JobView;

  test("lookup_by_fingerprint_and_hash", async () => {
    const c = await start();
    const a = await open(c, ["Shoot/clip_a.mov", "Shoot/clip_b.mov", "copy of a.mov"]);
    const b = await open(c, ["backup/clip_a.mov", "gone.mov"]);
    const hashes = Array.from({ length: 3 }, (_, i) => `blake3:${String(i + 1).repeat(64)}`);
    const fa = await probe(c, a, "Shoot/clip_a.mov", { fp: "fp1:a" });
    const fb = await probe(c, a, "Shoot/clip_b.mov", { fp: "fp1:b" });
    await probe(c, a, "copy of a.mov", { fp: "fp1:a" });
    const ba = await probe(c, b, "backup/clip_a.mov", { fp: "fp1:a" });
    await probe(c, b, "gone.mov", { fp: "fp1:gone" });
    for (const [lib, relKey, id, h] of [[a, "Shoot/clip_a.mov", fa, hashes[0]!], [a, "Shoot/clip_b.mov", fb, hashes[1]!], [b, "backup/clip_a.mov", ba, hashes[0]!]] as const) {
      const row = await run(c.reader.resolve(lib, relKey));
      await run(c.indexer.applyHashes(lib, [{ fileId: id, size: row.size, mtimeNs: row.mtimeNs, fullHash: h }]));
    }
    const l = (req: Omit<Parameters<typeof lookupLibraries>[1], "libraries">, libs = [a, b]) => run(lookupLibraries(c.reader, { libraries: libs, ...req }));
    const r1 = await l({ fingerprints: ["fp1:b", "fp1:a", "fp1:none"] });
    expect(r1.pending).toBeUndefined();
    expect(r1.matches.map((m) => [m.key, m.library === a ? "a" : "b", m.display])).toEqual([
      ["fp1:b", "a", "Shoot/clip_b.mov"],
      ["fp1:a", "a", "copy of a.mov"],
      ["fp1:a", "a", "Shoot/clip_a.mov"],
      ["fp1:a", "b", "backup/clip_a.mov"],
    ]);
    expect(r1.matches[0]).toEqual({ key: "fp1:b", library: a, fileId: fb, relKey: "Shoot/clip_b.mov", display: "Shoot/clip_b.mov", state: "present" });
    const r2 = await l({ hashes: [hashes[0]!, hashes[2]!] });
    expect(r2.matches.map((m) => [m.key, m.fileId])).toEqual([[hashes[0], fa], [hashes[0], ba]]);
    expect((await l({ hashes: [hashes[0]!] }, [b])).matches.map((m) => m.library)).toEqual([b]);
    // A file that went missing is still answered, with its state.
    await scan(c, b, ["backup/clip_a.mov"]);
    expect((await l({ fingerprints: ["fp1:gone"] })).matches.map((m) => m.state)).toEqual(["missing"]);
    // Requests are checked.
    for (const bad of [{}, { fingerprints: ["x"], hashes: [hashes[0]!] }, { fingerprints: [] }, { hashes: ["blake3:abc"] }, { hashes: [hashes[0]!.toUpperCase()] }, { fingerprints: Array.from({ length: 1001 }, (_, i) => `fp1:${i}`) }]) {
      expect(await failCode(lookupLibraries(c.reader, { libraries: [a], ...bad }))).toBe("INVALID_REQUEST");
    }
    expect((await l({ fingerprints: Array.from({ length: 1000 }, (_, i) => `fp1:${i}`) })).matches).toEqual([]);
  });

  test("hash_candidates_submit_one_job", async () => {
    const c = await start();
    const a = await open(c, [{ path: "x1.mov", size: 500 }, { path: "x2.mov", size: 500 }, { path: "x3.mov", size: 500 }, { path: "x4.mov", size: 500 }, { path: "y.mov", size: 600 }]);
    const b = await open(c, [{ path: "z.mov", size: 500 }, { path: "w.mov", size: 700 }]);
    const h = `blake3:${"a".repeat(64)}`;
    const x3 = await probe(c, a, "x3.mov", { fp: "fp1:x" });
    await run(c.indexer.applyHashes(a, [{ fileId: x3, size: 500, mtimeNs: "1700000000000000000", fullHash: h }]));
    const ids = async (lib: string, ...rels: string[]) => Promise.all(rels.map((r) => run(c.reader.resolve(lib, r)).then((x) => x.id)));
    // x4 is gone: a missing row is not hashed.
    await scan(c, a, [{ path: "x1.mov", size: 500 }, { path: "x2.mov", size: 500 }, { path: "x3.mov", size: 500 }, { path: "y.mov", size: 600 }]);
    const calls: Array<[string, ReadonlyArray<number>]> = [];
    const submit: HashSubmit = (lib, fileIds) => {
      calls.push([lib, fileIds]);
      return Effect.succeed(job(`job-${calls.length}`));
    };
    const lookup = (req: Partial<LookupRequest>, libs = [a, b], hash: HashSubmit | undefined = submit) =>
      run(lookupLibraries(c.reader, { libraries: libs, hashes: [h], ...req }, hash));
    // Without the flag nothing is submitted.
    expect((await lookup({})).pending).toBeUndefined();
    expect(calls).toEqual([]);
    // Rows of the matches' size without a full hash: one job per library, each with all its rows.
    const r = await lookup({ hashCandidates: true });
    expect(r.matches.map((m) => m.fileId)).toEqual([x3]);
    expect(calls).toEqual([[a, await ids(a, "x1.mov", "x2.mov")], [b, await ids(b, "z.mov")]]);
    expect(r.pending).toEqual({ job: "job-1", jobs: [{ library: a, job: "job-1" }, { library: b, job: "job-2" }], more: false });
    // One library: exactly one job.
    calls.length = 0;
    expect((await lookup({ hashCandidates: true }, [a])).pending?.jobs).toHaveLength(1);
    expect(calls).toHaveLength(1);
    // Fingerprints work the same way.
    calls.length = 0;
    const byFp = await run(lookupLibraries(c.reader, { libraries: [a, b], fingerprints: ["fp1:x"], hashCandidates: true }, submit));
    expect(byFp.matches.map((m) => m.fileId)).toEqual([x3]);
    expect(calls.map(([lib]) => lib)).toEqual([a, b]);
    // No match, no sizes, no job.
    calls.length = 0;
    expect((await lookup({ hashCandidates: true, hashes: [`blake3:${"b".repeat(64)}`] })).pending).toBeUndefined();
    // Nothing left to hash, no job.
    const done = await Promise.all(["x1.mov", "x2.mov"].map((rel) => run(c.reader.resolve(a, rel)).then((x) => ({ fileId: x.id, size: x.size, mtimeNs: x.mtimeNs, fullHash: `blake3:${"c".repeat(64)}` }))));
    await run(c.indexer.applyHashes(a, done));
    expect((await lookup({ hashCandidates: true }, [a])).pending).toBeUndefined();
    expect(calls).toEqual([]);
    // A service that cannot hash refuses the flag.
    expect(await failCode(lookupLibraries(c.reader, { libraries: [a], hashes: [h], hashCandidates: true }))).toBe("INVALID_REQUEST");
  });
});
