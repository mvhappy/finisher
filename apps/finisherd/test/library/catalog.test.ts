// Library catalogs: schema, migrations, rebuilds and batch files (plan 5D Task 7a, Decisions 23, 26).
import { existsSync, mkdirSync, readFileSync, readdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BATCH_MAX_BYTES, SPECS } from "../../src/library/catalog/batch-file.ts";
import { catalogCore0001 } from "../../src/library/catalog/core/migrations/0001.ts";
import { decodeProbe, encodeProbe } from "../../src/library/catalog/core/probe-codec.ts";
import { coreQueries } from "../../src/library/catalog/core/queries.ts";
import { ftsAdd, ftsRemove, ftsReplace, matchExpression, prepareFts } from "../../src/library/catalog/fts.ts";
import { catalogPath } from "../../src/library/catalog/open.ts";
import {
  type Scratch,
  dirEntry,
  done,
  entry,
  failCode,
  newLib,
  rel,
  run,
  scratch,
  stopAll,
  threads,
  writeBatch,
  applied,
} from "./support/catalog.ts";

let s: Scratch;
beforeEach(() => {
  s = scratch();
});
afterEach(async () => {
  await stopAll();
  s.cleanup();
});

const peek = <A>(path: string, f: (db: DatabaseSync) => A): A => {
  const db = new DatabaseSync(path, { readOnly: true });
  try {
    return f(db);
  } finally {
    db.close();
  }
};

test("core_and_extension_migrations_apply_once_per_catalog", async () => {
  const a = newLib();
  const b = newLib();
  let c = await threads(s);
  expect(await run(c.indexer.open(a))).toMatchObject({ created: true });
  await run(c.indexer.open(b));
  await run(c.indexer.open(a));
  await c.shutdown();
  c = await threads(s);
  expect(await run(c.indexer.open(a))).toMatchObject({ created: false });
  await c.shutdown();
  for (const lib of [a, b]) {
    const path = catalogPath(s.serverData, lib);
    const rows = peek(path, (db) => db.prepare("SELECT chain, n FROM migrations ORDER BY chain").all());
    expect(rows).toEqual([
      { chain: "core", n: 1 },
      { chain: "core", n: 2 },
      { chain: "notes", n: 1 },
    ]);
    const meta = peek(path, (db) => Object.fromEntries((db.prepare("SELECT key, value FROM meta").all() as Array<{ key: string; value: string }>).map((r) => [r.key, r.value])));
    expect(meta).toMatchObject({ libraryId: lib, "formatVersion.core": "2", "minReaderVersion.core": "1", "formatVersion.notes": "1" });
    if (process.platform !== "win32") expect(readFileSync(path).length).toBeGreaterThan(0);
  }
});

test("catalog_of_another_library_moved_aside", async () => {
  const a = newLib();
  const b = newLib();
  const c = await threads(s);
  await run(c.indexer.open(a));
  await run(c.indexer.exec(a, "notes", "add", { text: "from a" }));
  await run(c.indexer.close(a));
  mkdirSync(join(s.serverData, "libraries", b), { recursive: true });
  writeFileSync(catalogPath(s.serverData, b), readFileSync(catalogPath(s.serverData, a)));
  expect(await run(c.indexer.open(b))).toMatchObject({ created: true, rebuilt: "foreign" });
  expect(await run(c.reader.query(b, "notes", "list", {}))).toEqual([]);
  expect(readdirSync(join(s.serverData, "libraries", b)).some((n) => /^catalog\.db\.foreign-[0-9a-z]{26}$/.test(n))).toBe(true);
});

test("corrupt_catalog_rebuilt", async () => {
  const lib = newLib();
  mkdirSync(join(s.serverData, "libraries", lib), { recursive: true });
  writeFileSync(catalogPath(s.serverData, lib), Buffer.alloc(8192, 0x5a));
  const c = await threads(s);
  expect(await run(c.indexer.open(lib))).toMatchObject({ created: true, rebuilt: "corrupt" });
  expect(await run(c.reader.counts(lib))).toEqual({ files: {}, dirs: {}, collisions: 0 });
  expect(readdirSync(join(s.serverData, "libraries", lib)).some((n) => /^catalog\.db\.corrupt-[0-9a-z]{26}$/.test(n))).toBe(true);
});

test("too_new_extension_is_offline", async () => {
  const lib = newLib();
  let c = await threads(s);
  await run(c.indexer.open(lib));
  await c.shutdown();
  const db = new DatabaseSync(catalogPath(s.serverData, lib));
  db.prepare("UPDATE meta SET value = '7' WHERE key = 'minReaderVersion.notes'").run();
  db.close();
  c = await threads(s);
  expect(await failCode(c.indexer.open(lib))).toBe("CATALOG_TOO_NEW");
  expect(await failCode(c.reader.counts(lib))).toBe("CATALOG_NOT_OPEN");
  // The file is left as it was, for a newer finisherd.
  expect(existsSync(catalogPath(s.serverData, lib))).toBe(true);
  expect(readdirSync(join(s.serverData, "libraries", lib)).filter((n) => n.includes("corrupt") || n.includes("foreign"))).toEqual([]);
});

/** A small tree: `Interviews/` with two files, a placeholder and an excluded file, plus a root file. */
const tree = () => [
  {
    seq: 1,
    entries: [dirEntry([], "Interviews"), entry([], "notes.txt", { size: 5 })],
    dirsDone: [done([], 2)],
  },
  {
    seq: 1,
    entries: [
      entry(["Interviews"], "interview_take3.final.mov", { size: 1000, fileId: "AQI=" }),
      entry(["Interviews"], "Café.wav", { size: 2000 }),
      entry(["Interviews"], "remote.mov", { placeholder: "dataless" }),
      entry(["Interviews"], ".DS_Store", { excluded: "DEFAULT" }),
    ],
    dirsDone: [done(["Interviews"], 4)],
  },
];

describe("batch files", () => {
  test("a valid file lands in the catalog", async () => {
    const lib = newLib();
    const c = await threads(s);
    await run(c.indexer.open(lib));
    const counts = await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, tree()), applied(1, 1)));
    expect(counts).toMatchObject({ dirs: 1, added: 4, skipped: 1, listings: 2, transactions: 1 });
    expect(await run(c.reader.counts(lib))).toEqual({ files: { present: 3, cloudOnly: 1 }, dirs: { present: 1 }, collisions: 0 });
    const listing = await run(c.reader.files(lib, { dir: "Interviews", limit: 2 }));
    expect(listing.files.map((f) => f.nameNfc)).toEqual(["Café.wav", "interview_take3.final.mov"]);
    expect(listing.next).toBeDefined();
    const page2 = await run(c.reader.files(lib, { dirId: listing.dir!.id, after: listing.next, limit: 2 }));
    expect(page2.files.map((f) => f.nameNfc)).toEqual(["remote.mov"]);
    expect(page2.next).toBeUndefined();
    const one = await run(c.reader.file(lib, { relKey: "Interviews/interview_take3.final.mov" }));
    expect(one.matches).toHaveLength(1);
    expect(one.matches[0]).toMatchObject({ rel: rel("Interviews", "interview_take3.final.mov"), fileId: "AQI=", ext: "mov", state: "present" });
    const root = await run(c.reader.files(lib, { limit: 10 }));
    expect(root.dir).toMatchObject({ id: 1, parent: null, name: "", relKey: "" });
    expect(root.dirs.map((d) => d.nameNfc)).toEqual(["Interviews"]);
    expect(root.dirs[0]?.mtimeNs).toBe("1700000000000000001");
  });

  test("raw names are the key: NFC/NFD twins and non-UTF-8 names keep their own rows", async () => {
    const lib = newLib();
    const c = await threads(s);
    await run(c.indexer.open(lib));
    const nfc = "Café.mov";
    const nfd = "Café.mov";
    const latin1 = Uint8Array.from([0x66, 0xe9, 0x2e, 0x6d, 0x6f, 0x76]);
    const batch = { seq: 1, entries: [entry([], nfc), entry([], nfd), { ...entry([], "f\uFFFD.mov", { raw: latin1 }), nameLossy: true }], dirsDone: [done([], 3)] };
    await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [batch]), applied(1)));
    const twins = await run(c.reader.file(lib, { relKey: nfc }));
    expect(twins.matches).toHaveLength(2);
    expect(new Set(twins.matches.map((m) => m.name)).size).toBe(2);
    const lossy = await run(c.reader.file(lib, { relKey: "f�.mov" }));
    expect(Buffer.from(lossy.matches[0]!.name, "base64")).toEqual(Buffer.from(latin1));
  });

  test("changed rows lose their fingerprint; unchanged rows keep it", async () => {
    const lib = newLib();
    const c = await threads(s);
    await run(c.indexer.open(lib));
    const first = { seq: 1, entries: [entry([], "a.mov", { size: 10 }), entry([], "b.mov", { size: 20 })], dirsDone: [done([], 2)] };
    await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [first]), applied(1)));
    const ids = (await run(c.reader.files(lib, { limit: 10 }))).files.map((f) => f.id);
    const probed = await run(
      c.indexer.applyProbed(
        lib,
        ids.map((fileId, i) => ({
          fileId, size: (i + 1) * 10, mtimeNs: "1700000000000000000", fp: `fp1:${i}`, probeVersion: 1, state: "present",
          kind: "media", durationMs: 1000, probe: { kind: "media", fingerprint: `fp1:${i}`, size: (i + 1) * 10, media: { duration: "1" } },
        })),
      ),
    );
    expect(probed).toEqual({ updated: 2, stale: 0, moved: [] });
    const second = { seq: 2, entries: [entry([], "a.mov", { size: 11 }), entry([], "b.mov", { size: 20 })], dirsDone: [done([], 2)] };
    expect(await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [second]), applied(2)))).toMatchObject({ changed: 1, seen: 1 });
    const rows = (await run(c.reader.files(lib, { limit: 10 }))).files;
    expect(rows.map((f) => [f.nameNfc, f.fp, f.kind, f.probeVersion])).toEqual([
      ["a.mov", null, null, null],
      ["b.mov", "fp1:1", "media", 1],
    ]);
    expect(rows[1]?.probe).toEqual({ kind: "media", fingerprint: "fp1:1", size: 20, media: { duration: "1" } });
    // A probe of the old size lands nowhere.
    const late = await run(c.indexer.applyProbed(lib, [{ fileId: ids[0]!, size: 10, mtimeNs: "1700000000000000000", fp: "fp1:old", probeVersion: 1, state: "present" }]));
    expect(late).toEqual({ updated: 0, stale: 1, moved: [] });
    expect((await run(c.reader.lookup(lib, { fingerprints: ["fp1:1", "fp1:old"] }))).matches.map((m) => m.key)).toEqual(["fp1:1"]);
  });

  test("batch_file_read_and_validated_in_indexer", async () => {
    const lib = newLib();
    const c = await threads(s);
    await run(c.indexer.open(lib));
    const good = tree();
    const reason = async (path: string) => {
      const exit = await run(c.indexer.applyBatchFile(lib, path, applied(1)).pipe((e) => e, (e) => e)).then(
        () => "ok",
        (e: { code: string; details: { reason: string } }) => `${e.code}:${e.details.reason}`,
      );
      return exit;
    };
    // Outside a job's out/.
    const elsewhere = join(s.root, "out");
    mkdirSync(elsewhere);
    expect(await reason(writeBatch(elsewhere, good))).toBe("LIBRARY_BATCH_INVALID:outsideJobs");
    expect(await reason(writeBatch(s.inDir, good))).toBe("LIBRARY_BATCH_INVALID:outDir");
    // A symlink to a valid file.
    const target = writeBatch(s.inDir, good, "target.ndjson");
    if (process.platform !== "win32") {
      symlinkSync(target, join(s.outDir, "link.ndjson"));
      expect(await reason(join(s.outDir, "link.ndjson"))).toBe("LIBRARY_BATCH_INVALID:notRegular");
    }
    // Oversized.
    const big = join(s.outDir, "big.ndjson");
    writeFileSync(big, Buffer.alloc(BATCH_MAX_BYTES + 1, 0x20));
    expect(await reason(big)).toBe("LIBRARY_BATCH_INVALID:tooLarge");
    // Schema-invalid lines: an unknown key, a missing key, a bad base64 name, an i64 overflow.
    const line = (mut: (e: Record<string, unknown>) => void) => {
      const b = structuredClone(good);
      mut(b[1]!.entries[0] as unknown as Record<string, unknown>);
      return writeBatch(s.outDir, b);
    };
    expect(await reason(line((e) => (e.extra = 1)))).toBe("LIBRARY_BATCH_INVALID:schema");
    expect(await reason(line((e) => delete e.relKey))).toBe("LIBRARY_BATCH_INVALID:schema");
    expect(await reason(line((e) => (e.name = "not base64!")))).toBe("LIBRARY_BATCH_INVALID:schema");
    expect(await reason(line((e) => (e.mtimeNs = "9223372036854775808")))).toBe("LIBRARY_BATCH_INVALID:schema");
    const torn = join(s.outDir, "torn.ndjson");
    writeFileSync(torn, `${JSON.stringify(good[0])}\n{"seq":`);
    expect(await reason(torn)).toBe("LIBRARY_BATCH_INVALID:json");
    // An entry whose directory the catalog does not know.
    expect(await reason(writeBatch(s.outDir, [good[1]!]))).toBe("LIBRARY_BATCH_INVALID:unknownDir");
    // A seq other than the call's.
    expect(await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, good), applied(1, 9))).catch((e: { details: { reason: string } }) => e.details.reason)).toBe("seq");
    // Nothing of the above was written.
    expect(await run(c.reader.counts(lib))).toEqual({ files: {}, dirs: {}, collisions: 0 });
  });

  test("the validator matches the generated schema field for field", () => {
    const defs = JSON.parse(readFileSync(new URL("../../../../packages/core-types/schema/fin-core.schema.json", import.meta.url), "utf8")).$defs as Record<
      string,
      { properties: Record<string, unknown>; required?: string[]; additionalProperties?: boolean }
    >;
    for (const [name, spec] of Object.entries(SPECS)) {
      const def = defs[name]!;
      expect(Object.keys(spec).sort(), name).toEqual(Object.keys(def.properties).sort());
      const required = Object.entries(spec).filter(([, f]) => f.required).map(([k]) => k).sort();
      expect(required, name).toEqual([...(def.required ?? [])].sort());
      expect(def.additionalProperties, name).toBe(false);
    }
  });
});

test("transactions_capped_at_2000_rows", async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  const entries = Array.from({ length: 4096 }, (_, i) => entry([], `clip-${i}.mov`));
  const out = await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, [{ seq: 1, entries, dirsDone: [done([], 4096)] }]), applied(1)));
  expect(out).toMatchObject({ added: 4096, listings: 1, transactions: 3 });
  const st = await run(c.indexer.stats());
  expect(st.maxTxRows).toBeLessThanOrEqual(2000);
  expect(st.batchFiles).toBe(1);
  expect((await run(c.reader.counts(lib))).files).toEqual({ present: 4096 });
});

test("probes are stored deflated without the row's own fields, and read back whole", () => {
  const row = { fp: "fp1:ab", size: 42, kind: "media" };
  const p = { kind: "media", fingerprint: "fp1:ab", size: 42, media: { container: "mp4", duration: "1001/30" } };
  const enc = encodeProbe(p, row)!;
  expect(enc[0]).toBe(1);
  expect(enc.length).toBeLessThan(JSON.stringify(p).length);
  expect(decodeProbe(enc, row)).toEqual(p);
  // Fields that differ from the row are kept as they were.
  const odd = { kind: "image", fingerprint: "fp1:zz", size: 7 };
  expect(decodeProbe(encodeProbe(odd, row), row)).toEqual(odd);
  expect(encodeProbe(null, row)).toBeNull();
  expect(decodeProbe(null, row)).toBeNull();
  expect(() => decodeProbe(Uint8Array.of(9, 1), row)).toThrow(/format/);
});

describe("fts_rows_follow_inserts_renames_deletes", () => {
  test("through the helpers the diff uses", () => {
    const db = new DatabaseSync(":memory:");
    catalogCore0001.up(db);
    const fts = prepareFts(db);
    const add = db.prepare(
      "INSERT INTO files (dirId, name, nameNfc, relKey, size, mtimeNs, state, firstSeen, lastSeen, lastChange) VALUES (1, ?, ?, ?, 1, 1, 'present', 0, 0, ?) RETURNING id",
    );
    const id = (add.get(Buffer.from("interview_take3.final.mov"), "interview_take3.final.mov", "interview_take3.final.mov", 1) as { id: number }).id;
    ftsAdd(fts, id, "interview_take3.final.mov", "projects/café");
    const hits = (q: string) => coreQueries.search(db, { query: q, limit: 10 }).hits.map((h) => h.id);
    expect(hits("interv")).toEqual([id]);
    expect(hits("take3 final")).toEqual([id]);
    expect(hits("cafe")).toEqual([id]);
    ftsReplace(fts, id, "wedding_toast.mov", "archive");
    expect(hits("interv")).toEqual([]);
    expect(hits("toast arch")).toEqual([id]);
    ftsRemove(fts, id);
    expect(hits("toast")).toEqual([]);
    expect(db.prepare("SELECT count(*) AS n FROM names").get()).toEqual({ n: 0 });
    db.close();
  });

  test("through batches and the purge", async () => {
    const lib = newLib();
    const c = await threads(s);
    await run(c.indexer.open(lib));
    await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, tree()), applied(1)));
    const names = async (q: string) => (await run(c.reader.search(lib, { query: q, limit: 10 }))).hits.map((h) => h.nameNfc);
    expect(await names("take")).toEqual(["interview_take3.final.mov"]);
    // A directory's tokens match every file in it.
    expect((await names("interv")).sort()).toEqual(["Café.wav", "interview_take3.final.mov", "remote.mov"]);
    expect(await names("cafe")).toEqual(["Café.wav"]);
    expect(await names('"); DROP TABLE files; --')).toEqual([]);
    const db = new DatabaseSync(catalogPath(s.serverData, lib));
    db.exec("PRAGMA busy_timeout = 1000");
    db.prepare("UPDATE files SET state = 'missing', missingSince = 5 WHERE nameNfc = 'Café.wav'").run();
    db.close();
    expect(await run(c.indexer.purgeMissing(lib, { before: 10, beforeGen: 1 }))).toEqual({ deleted: 1, dirs: 0 });
    expect(await names("cafe")).toEqual([]);
  });

  test("query terms become quoted prefixes", () => {
    expect(matchExpression("Interv take_3.final")).toBe('"Interv"* "take"* "3"* "final"*');
    expect(matchExpression(' "*()^ ')).toBeUndefined();
  });
});

test("writeKnownDirs writes every known directory's raw chain", async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, tree()), applied(3)));
  const path = join(s.inDir, "dirs.ndjson");
  expect(await run(c.indexer.writeKnownDirs(lib, 3, path))).toEqual({ dirs: 2 });
  const lines = readFileSync(path, "utf8").trim().split("\n").map((l) => JSON.parse(l) as unknown);
  expect(lines).toEqual([
    { dir: { components: [] }, mtimeNs: "1700000000000000001", listed: true, entries: 2, mountRoot: false },
    { dir: rel("Interviews"), mtimeNs: "1700000000000000001", listed: true, entries: 4, mountRoot: false },
  ]);
  // Exclusive create, and only inside a job's in/.
  expect(await failCode(c.indexer.writeKnownDirs(lib, 3, path))).toBe("CATALOG_OP_FAILED");
  expect(await failCode(c.indexer.writeKnownDirs(lib, 3, join(s.outDir, "dirs.ndjson")))).toBe("CATALOG_ARGS_INVALID");
});

test("stages, scans and pending", async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  await run(c.indexer.applyBatchFile(lib, writeBatch(s.outDir, tree()), applied(1)));
  const probe = await run(c.indexer.pending(lib, { stage: "probe", probeVersion: 1, exts: ["mov"], limit: 10 }));
  expect(probe.files.map((f) => f.rel)).toEqual([rel("Interviews", "interview_take3.final.mov")]);
  const all = await run(c.indexer.pending(lib, { stage: "probe", probeVersion: 1, exts: null, limit: 10 }));
  expect(all.files).toHaveLength(3);
  const f = probe.files[0]!;
  await run(c.indexer.applyProbed(lib, [{ fileId: f.fileId, size: f.size, mtimeNs: f.mtimeNs, fp: "fp1:a", probeVersion: 1, state: "present" }]));
  expect((await run(c.indexer.pending(lib, { stage: "proxy540", version: "v1", limit: 10 }))).files.map((p) => p.fp)).toEqual(["fp1:a"]);
  await run(c.indexer.stagesSet(lib, [{ fp: "fp1:a", stage: "proxy540", version: "v1", state: "done", at: 5 }]));
  expect((await run(c.indexer.pending(lib, { stage: "proxy540", version: "v1", limit: 10 }))).files).toEqual([]);
  expect((await run(c.indexer.pending(lib, { stage: "proxy540", version: "v2", limit: 10 }))).files).toHaveLength(1);
  expect(await run(c.reader.stageCounts(lib))).toEqual({ stages: [{ stage: "proxy540", version: "v1", state: "done", n: 1 }] });
  expect((await run(c.reader.file(lib, { fileId: f.fileId }))).stages).toHaveLength(1);
  const scan = await run(c.indexer.finishScan(lib, { mode: "full", trigger: "add", gen: 1, startedAt: 1, finishedAt: 2, outcome: "ok", counts: { files: 3 } }));
  expect(scan.scanId).toBe(1);
  expect(await failCode(c.indexer.stagesSet(lib, [{ fp: "x", stage: "s", version: "1", state: "bogus" as "done", at: 1 }]))).toBe("CATALOG_ARGS_INVALID");
});
