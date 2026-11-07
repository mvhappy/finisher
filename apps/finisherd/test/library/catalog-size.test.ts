// The catalog's size per file with the FTS `dir` column (plan 5D Task 7a, Decision 23): a
// generated 100k-file tree in 5k directories, scanned through batch files and probed, must
// stay ≤ 1 KiB per file; otherwise directory tokens move to a `dirnames` table. The figure
// and the throughput are printed for TASKS.md.
import { statSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import type { ScanBatch, ScanEntry } from "@finisher/core-types";
import { afterEach, beforeEach, expect, test } from "vitest";
import { catalogPath } from "../../src/library/catalog/open.ts";
import { type Scratch, dirEntry, done, entry, newLib, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";

let s: Scratch;
beforeEach(() => {
  s = scratch();
});
afterEach(async () => {
  await stopAll();
  s.cleanup();
});

const TOP = 50;
const SUB = 100;
const PER_DIR = 20;
const WORDS = ["Interview", "Café", "Hochzeit", "Свадьба", "インタビュー", "Entrevista", "B-roll", "Señal", "Drone", "Ünterwegs"];

/** A file name in several scripts; every fifth one in NFD, as macOS writes them. */
function fileName(t: number, d: number, f: number): string {
  const w = WORDS[(t + d + f) % WORDS.length]!;
  const name = `${w}_${t}-${d}_take${f}.${["mov", "mp4", "wav", "jpg", "mxf"][f % 5]}`;
  return f % 5 === 0 ? name.normalize("NFD") : name;
}

/** BFS listings of the tree, as the walker emits them, cut into batches of ≤ 4,096 entries. */
function* listings(): Generator<{ entries: ScanEntry[]; dir: string[] }> {
  const tops = Array.from({ length: TOP }, (_, t) => `${WORDS[t % WORDS.length]} ${2000 + t}`);
  yield { dir: [], entries: tops.map((n) => dirEntry([], n)) };
  for (const top of tops) yield { dir: [top], entries: Array.from({ length: SUB }, (_, d) => dirEntry([top], `Shoot ${d} – Day ${d % 7}`)) };
  for (let t = 0; t < TOP; t++) {
    for (let d = 0; d < SUB; d++) {
      const dir = [tops[t]!, `Shoot ${d} – Day ${d % 7}`];
      yield {
        dir,
        entries: Array.from({ length: PER_DIR }, (_, f) =>
          entry(dir, fileName(t, d, f), { size: 1_000_000 + f * 1001, fileId: Buffer.from(`${t}.${d}.${f}`).toString("base64") }),
        ),
      };
    }
  }
}

function batches(): ScanBatch[] {
  const out: ScanBatch[] = [];
  let cur: ScanBatch = { seq: 1, entries: [], dirsDone: [] };
  for (const l of listings()) {
    if (cur.entries.length + l.entries.length > 4096) {
      out.push(cur);
      cur = { seq: out.length + 1, entries: [], dirsDone: [] };
    }
    cur.entries.push(...l.entries);
    cur.dirsDone.push(done(l.dir, l.entries.length));
  }
  out.push(cur);
  return out;
}

/** A probe as fin-probe answers for a typical camera MP4. */
const probe = (i: number) => ({
  kind: "media",
  fingerprint: `fp1:${i.toString(16).padStart(32, "0")}`,
  size: 1_000_000 + i,
  media: {
    container: "mp4",
    duration: "1001/30",
    video: {
      codec: "h264", fourcc: "avc1", bitDepth: 8, chroma: "420", coded: { width: 1920, height: 1088 },
      display: { width: 1920, height: 1080 }, colour: { primaries: 1, transfer: 1, matrix: 1, fullRange: false },
      frames: 900, nominalRate: "30000/1001", pixelAspect: "1", rotation: 0, profile: 100, level: 41,
    },
    audio: { codec: "aac", fourcc: "mp4a", channels: 2, sampleRate: 48000, padding: 0, priming: 1024 },
  },
});

test("bytes_per_file_with_fts_dir", { timeout: 300_000 }, async () => {
  const lib = newLib();
  const c = await threads(s);
  await run(c.indexer.open(lib));
  const files = batches().map((b) => writeBatch(s.outDir, [b]));
  const t0 = performance.now();
  let entries = 0;
  for (const f of files) entries += (await run(c.indexer.applyBatchFile(lib, f, 1))).dirs + 0;
  const applied = await run(c.reader.counts(lib));
  const scanS = (performance.now() - t0) / 1000;
  expect(applied.files).toEqual({ present: TOP * SUB * PER_DIR });
  expect(entries).toBe(TOP + TOP * SUB);
  const total = TOP * SUB * PER_DIR + TOP + TOP * SUB;

  // Probe every file (fingerprint, probe JSON and the derived columns).
  for (;;) {
    const { files: next } = await run(c.indexer.pending(lib, { stage: "probe", probeVersion: 1, exts: null, limit: 1000 }));
    if (next.length === 0) break;
    for (let i = 0; i < next.length; i += 256) {
      const rows = next.slice(i, i + 256).map((f) => ({
        fileId: f.fileId, size: f.size, mtimeNs: f.mtimeNs, fp: probe(f.fileId).fingerprint, probe: probe(f.fileId),
        probeVersion: 1, state: "present" as const, kind: "media", durationMs: 33_366, width: 1920, height: 1080,
      }));
      await run(c.indexer.applyProbed(lib, rows));
    }
  }
  await run(c.indexer.close(lib));
  const path = catalogPath(s.serverData, lib);
  const bytes = statSync(path).size;
  const n = TOP * SUB * PER_DIR;
  const db = new DatabaseSync(path, { readOnly: true });
  const byTable = db
    .prepare("SELECT name, sum(pgsize) AS bytes FROM dbstat GROUP BY name ORDER BY bytes DESC")
    .all() as Array<{ name: string; bytes: number }>;
  const probeBytes = (db.prepare("SELECT avg(length(probe)) AS n FROM files").get() as { n: number }).n;
  // What the `dir` column costs: the same names in an index without it.
  const alt = new DatabaseSync(":memory:");
  alt.exec("CREATE VIRTUAL TABLE n USING fts5 (name, content = '', contentless_delete = 1, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3')");
  const ins = alt.prepare("INSERT INTO n (rowid, name) VALUES (?, ?)");
  alt.exec("BEGIN");
  for (const r of db.prepare("SELECT id, nameNfc FROM files").iterate() as Iterable<{ id: number; nameNfc: string }>) ins.run(r.id, r.nameNfc);
  alt.exec("COMMIT");
  const nameOnly = (alt.prepare("SELECT sum(pgsize) AS b FROM dbstat WHERE name LIKE 'n%'").get() as { b: number }).b;
  alt.close();
  db.close();
  const fts = byTable.filter((r) => r.name.startsWith("names")).reduce((a, r) => a + r.bytes, 0);
  const perFile = bytes / n;
  console.log(
    `catalog size: ${bytes} bytes for ${n} files in ${TOP * SUB} dirs = ${perFile.toFixed(0)} B/file ` +
      `(FTS ${(fts / n).toFixed(0)} B/file, of which the dir column ${((fts - nameOnly) / n).toFixed(0)}; ` +
      `probe ${probeBytes.toFixed(0)} B stored); scan apply ${(total / scanS).toFixed(0)} entries/s; ` +
      `tables: ${byTable.slice(0, 8).map((r) => `${r.name} ${(r.bytes / n).toFixed(0)}`).join(", ")}`,
  );
  expect(perFile).toBeLessThanOrEqual(1024);
});
