// The library budget's generated tree and batches (plan 5D Task 15): `top × sub` leaf
// directories of `per` files, names in several scripts, every fifth one in NFD (as macOS
// writes them), and in every tenth directory a case twin and an NFC twin of its first file
// (on a filesystem that folds case or normalisation the twin is the same file, so the tree
// then holds fewer files: the budget reads the count back from the catalog).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { DirDone, ScanBatch, ScanEntry } from "@finisher/core-types";

export interface TreeShape {
  readonly top: number;
  readonly sub: number;
  readonly per: number;
}

/** 100k files in 5k leaf directories (plus 50 top directories). */
export const FULL: TreeShape = { top: 50, sub: 100, per: 20 };

const WORDS = ["Interview", "Café", "Hochzeit", "Свадьба", "インタビュー", "Entrevista", "B-roll", "Señal", "Drone", "Ünterwegs"];
const EXTS = ["mov", "mp4", "wav", "jpg", "mxf"];

/** Words a search budget asks for (whole words, prefixes, a folded diacritic). */
export const QUERIES = ["interview", "cafe", "hochzeit", "свадьба", "entrev", "take1", "drone take", "shoot 4", "señal", "ünter", "b-roll", "インタビュー"];

const topName = (t: number): string => `${WORDS[t % WORDS.length]} ${2000 + t}`;
const subName = (d: number): string => `Shoot ${d} – Day ${d % 7}`;

function fileName(t: number, d: number, f: number): string {
  const w = WORDS[(t + d + f) % WORDS.length]!;
  const name = `${w}_${t}-${d}_take${f}.${EXTS[f % EXTS.length]}`;
  return f % 5 === 0 ? name.normalize("NFD") : name;
}

/** The names of one leaf directory: `per` files, and in every tenth directory two twins of the first. */
function leafNames(shape: TreeShape, t: number, d: number): string[] {
  const names = Array.from({ length: shape.per }, (_, f) => fileName(t, d, f));
  if ((t * shape.sub + d) % 10 === 0) names.push(names[0]!.toUpperCase(), names[0]!.normalize("NFC"));
  return names;
}

/** Writes the tree under `root` (each file 16–79 bytes); answers how many files were written. */
export function writeTree(root: string, shape: TreeShape): number {
  let n = 0;
  for (let t = 0; t < shape.top; t++) {
    for (let d = 0; d < shape.sub; d++) {
      const dir = join(root, topName(t), subName(d));
      mkdirSync(dir, { recursive: true });
      for (const name of leafNames(shape, t, d)) {
        writeFileSync(join(dir, name), Buffer.alloc(16 + (n % 64), n % 251));
        n++;
      }
    }
  }
  return n;
}

const b64 = (s: string): string => Buffer.from(s, "utf8").toString("base64");
const rel = (parts: ReadonlyArray<string>) => ({ components: parts.map(b64) });

function entry(dir: ReadonlyArray<string>, name: string, type: ScanEntry["entryType"], size: number, fileId: string): ScanEntry {
  const nameNfc = name.normalize("NFC");
  return {
    dir: rel(dir),
    name: b64(name),
    nameNfc,
    nameLossy: false,
    relKey: [...dir.map((x) => x.normalize("NFC")), nameNfc].join("/"),
    entryType: type,
    size,
    mtimeNs: "1700000000000000000",
    mountRoot: false,
    automount: false,
    fileId: b64(fileId),
  };
}

const done = (dir: ReadonlyArray<string>, entries: number): DirDone => ({ dir: rel(dir), complete: true, entries, mtimeNs: "1700000000000000001" });

/**
 * The tree's listings as the walker emits them (breadth first), cut into batches of ≤ 4,096
 * entries: what the indexer's apply budget is measured on, without a scan in front of it.
 */
export function treeBatches(shape: TreeShape): { readonly batches: ScanBatch[]; readonly entries: number } {
  const listings: Array<{ dir: string[]; entries: ScanEntry[] }> = [];
  const tops = Array.from({ length: shape.top }, (_, t) => topName(t));
  listings.push({ dir: [], entries: tops.map((n) => entry([], n, "dir", 0, `t${n}`)) });
  for (const top of tops) listings.push({ dir: [top], entries: Array.from({ length: shape.sub }, (_, d) => entry([top], subName(d), "dir", 0, `s${top}/${d}`)) });
  for (let t = 0; t < shape.top; t++) {
    for (let d = 0; d < shape.sub; d++) {
      const dir = [tops[t]!, subName(d)];
      listings.push({ dir, entries: leafNames(shape, t, d).map((n, f) => entry(dir, n, "file", 1000 + f, `f${t}.${d}.${f}`)) });
    }
  }
  const batches: ScanBatch[] = [];
  let cur: ScanBatch = { seq: 1, entries: [], dirsDone: [] };
  let entries = 0;
  for (const l of listings) {
    if (cur.entries.length + l.entries.length > 4096) {
      batches.push(cur);
      cur = { seq: batches.length + 1, entries: [], dirsDone: [] };
    }
    cur.entries.push(...l.entries);
    cur.dirsDone.push(done(l.dir, l.entries.length));
    entries += l.entries.length;
  }
  batches.push(cur);
  return { batches, entries };
}
