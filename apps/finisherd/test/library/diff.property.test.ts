// `diff_matches_reference_model` (plan 5D Task 7b): random trees with raw and colliding names,
// renames with and without file ids, hard links, moves, rewrites, deletions, placeholder flips,
// unreadable and suddenly empty directories, resumed generations and polls interleaved with
// full scans go through the real indexer thread (batch files in jobs' `out/`) and through the
// brute-force model in `support/model.ts`; after every step both catalogs must agree row for
// row, ids included (a rename keeps its row, a move merges into the old one).
import { mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { ScanBatch } from "@finisher/core-types";
import { Effect } from "effect";
import fc from "fast-check";
import { afterAll, beforeAll, expect, test } from "vitest";
import type { Catalogs } from "../../src/library/catalog/client.ts";
import { catalogPath, libraryDir } from "../../src/library/catalog/open.ts";
import { type MFile, Model, type ProbeResult } from "./support/model.ts";
import { type Scratch, newLib, run, scratch, stopAll, threads, writeBatch } from "./support/catalog.ts";
import { type IdMode, type Item, NAMES, type WDir, type WFile, World, cut, fpOf } from "./support/world.ts";

let s: Scratch;
let c: Catalogs;
let now = 1_800_000_000_000;
/** What the runs exercised, summed: every kind must occur, or the generator misses a rule. */
const seen: Record<string, number> = {};

beforeAll(async () => {
  s = scratch();
  c = await threads(s, { clock: () => now });
});
afterAll(async () => {
  await stopAll();
  s.cleanup();
});

const sizes = fc.array(fc.integer({ min: 1, max: 7 }), { minLength: 1, maxLength: 4 });
const target = { node: fc.nat(), dir: fc.nat(), name: fc.nat() };
const step = fc.oneof(
  { weight: 6, arbitrary: fc.record({ op: fc.constant("create" as const), dir: fc.nat(), name: fc.nat(), isDir: fc.boolean(), size: fc.integer({ min: 1, max: 4 }) }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("delete" as const), node: fc.nat() }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("restore" as const) }) },
  { weight: 5, arbitrary: fc.record({ op: fc.constant("rename" as const), ...target }) },
  { weight: 3, arbitrary: fc.record({ op: fc.constant("link" as const), ...target }) },
  { weight: 3, arbitrary: fc.record({ op: fc.constant("copyMove" as const), ...target }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("rewrite" as const), node: fc.nat(), size: fc.integer({ min: 1, max: 4 }) }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("flip" as const), node: fc.nat() }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("unreadable" as const), node: fc.nat() }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("empty" as const), node: fc.nat() }) },
  { weight: 4, arbitrary: fc.record({ op: fc.constant("full" as const), sizes }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("poll" as const), pick: fc.array(fc.nat(), { maxLength: 4 }), sizes }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("both" as const), pick: fc.array(fc.nat(), { maxLength: 4 }), sizes, sizes2: sizes, order: fc.array(fc.boolean(), { maxLength: 30 }) }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("interrupt" as const), k: fc.nat(), sizes }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("resume" as const), sizes }) },
  { weight: 4, arbitrary: fc.record({ op: fc.constant("probe" as const) }) },
  { weight: 2, arbitrary: fc.record({ op: fc.constant("clock" as const), ms: fc.integer({ min: 0, max: 90_000 }) }) },
  { weight: 1, arbitrary: fc.record({ op: fc.constant("offline" as const) }) },
);
type Step = typeof step extends fc.Arbitrary<infer T> ? T : never;

const hexOf = (b64: string): string => Buffer.from(b64, "base64").toString("hex");

/** One run: a world, a catalog through the threads, the model, and their id pairing. */
class Run {
  readonly lib = newLib();
  readonly model = new Model();
  readonly world: World;
  readonly #toCat = new Map<number, number>();
  readonly #toModel = new Map<number, number>();
  #jobs = 0;
  #files = 0;
  #gen = 0;
  #pending: { job: string; gen: number; rest: ScanBatch[][] } | undefined;
  #deletedSeen = 0;
  /** Deleted nodes, for `restore` (a file back from the trash, with its id, size and mtime). */
  readonly #trash: Array<{ parent: WDir; name: string; node: WFile | WDir }> = [];

  constructor(opts: { caseInsensitive: boolean; ids: IdMode }) {
    this.world = new World(opts);
  }

  async start(): Promise<void> {
    await run(c.indexer.open(this.lib));
  }

  /** Closes the run's catalog and deletes it and its jobs' directories (runs add up fast). */
  async dispose(): Promise<void> {
    await run(c.indexer.close(this.lib));
    rmSync(libraryDir(s.serverData, this.lib), { recursive: true, force: true });
    for (let j = 1; j <= this.#jobs; j++) rmSync(join(s.jobsRoot, `j${j}-${this.lib.slice(-6)}`), { recursive: true, force: true });
  }

  #job(): string {
    const job = `j${++this.#jobs}-${this.lib.slice(-6)}`;
    mkdirSync(join(s.jobsRoot, job, "out"), { recursive: true });
    mkdirSync(join(s.jobsRoot, job, "in"), { recursive: true });
    return job;
  }

  async #apply(job: string, gen: number, files: ReadonlyArray<ScanBatch[]>): Promise<void> {
    for (const f of files) {
      now += 5;
      const path = writeBatch(join(s.jobsRoot, job, "out"), f, `b${++this.#files}.ndjson`);
      const got = await Effect.runPromise(
        Effect.match(c.indexer.applyBatchFile(this.lib, path, gen), {
          onFailure: (e) => {
            seen[e.code] = (seen[e.code] ?? 0) + 1;
            return e.code as string | undefined;
          },
          onSuccess: (n) => {
            for (const k of ["renamed", "renamePending", "reappeared", "missing", "suspect", "collisions", "changed"] as const) seen[k] = (seen[k] ?? 0) + n[k];
            return undefined;
          },
        }),
      );
      expect(got).toBe(this.model.applyFile(f, job, gen, now));
    }
  }

  #polled(pick: ReadonlyArray<number>): Item[] {
    const known = new Set(this.model.knownDirs(0).map((d) => this.model.chain(d.id).join("/")));
    const dirs = this.world.dirs().filter((l) => known.has(l.chain.join("/")));
    if (dirs.length === 0) return [];
    const chosen = [...new Set(pick.map((p) => dirs[p % dirs.length]!))].sort((a, b) => a.chain.length - b.chain.length);
    return chosen.flatMap((l) => this.world.listing(l.chain).items);
  }

  async step(st: Step): Promise<void> {
    const w = this.world;
    const nodes = w.all().slice(1);
    const node = (n: number) => (nodes.length === 0 ? undefined : nodes[n % nodes.length]);
    const dirs = w.dirs();
    const dir = (n: number) => dirs[n % dirs.length]!;
    const free = (d: WDir, n: number) => {
      const name = NAMES[n % NAMES.length]!;
      return d.children.has(name) ? undefined : name;
    };
    switch (st.op) {
      case "create": {
        const d = dir(st.dir).node as WDir;
        const name = free(d, st.name);
        if (name === undefined || nodes.length > 40) return;
        d.children.set(name, st.isDir ? { kind: "dir", children: new Map(), unreadable: false, placeholder: false } : { kind: "file", inode: w.newInode(st.size), placeholder: null });
        return;
      }
      case "delete": {
        const l = node(st.node);
        if (l?.parent == null) return;
        l.parent.children.delete(l.name);
        this.#trash.push({ parent: l.parent, name: l.name, node: l.node });
        return;
      }
      case "restore": {
        const t = this.#trash.pop();
        if (t !== undefined && !t.parent.children.has(t.name) && w.all().some((l) => l.node === t.parent)) t.parent.children.set(t.name, t.node);
        return;
      }
      case "rename":
      case "link":
      case "copyMove": {
        const l = node(st.node);
        const to = dir(st.dir);
        if (l === undefined || l.parent === null) return;
        if (l.node.kind === "dir" && (st.op !== "rename" || to.chain.join("/").startsWith(l.chain.join("/")))) return;
        const name = free(to.node as WDir, st.name);
        if (name === undefined) return;
        if (st.op !== "link") {
          l.parent.children.delete(l.name);
          if (st.op === "copyMove") this.#trash.push({ parent: l.parent, name: l.name, node: l.node });
        }
        const moved: WFile | WDir =
          st.op === "copyMove" && l.node.kind === "file"
            ? { kind: "file", inode: { ...w.newInode(l.node.inode.size), size: l.node.inode.size, mtimeNs: l.node.inode.mtimeNs, content: l.node.inode.content }, placeholder: l.node.placeholder }
            : st.op === "link" && l.node.kind === "file"
              ? { kind: "file", inode: l.node.inode, placeholder: l.node.placeholder }
              : l.node;
        (to.node as WDir).children.set(name, moved);
        return;
      }
      case "rewrite": {
        const l = node(st.node);
        if (l?.node.kind !== "file") return;
        Object.assign(l.node.inode, { size: st.size, mtimeNs: w.nextMtime(), content: l.node.inode.content + 1000 });
        return;
      }
      case "flip": {
        const l = node(st.node);
        if (l === undefined) return;
        if (l.node.kind === "file") l.node.placeholder = l.node.placeholder === null ? "nextcloud" : null;
        else l.node.placeholder = !l.node.placeholder;
        return;
      }
      case "unreadable":
      case "empty": {
        const d = dir(st.node).node as WDir;
        if (st.op === "empty") d.children.clear();
        else d.unreadable = !d.unreadable;
        return;
      }
      case "full":
        return this.#apply(this.#job(), ++this.#gen, cut(w.fullScan(), st.sizes));
      case "poll":
        return this.#apply(this.#job(), this.#gen, cut(this.#polled(st.pick), st.sizes));
      case "both": {
        const full = cut(w.fullScan(), st.sizes);
        const poll = cut(this.#polled(st.pick), st.sizes2);
        const jf = this.#job();
        const jp = this.#job();
        const gen = ++this.#gen;
        let i = 0;
        while (full.length > 0 || poll.length > 0) {
          const pollNext = poll.length > 0 && (full.length === 0 || st.order[i++ % Math.max(1, st.order.length)] === true);
          if (pollNext) await this.#apply(jp, gen, [poll.shift()!]);
          else await this.#apply(jf, gen, [full.shift()!]);
        }
        return;
      }
      case "interrupt": {
        if (this.#pending !== undefined) return;
        const files = cut(w.fullScan(), st.sizes);
        const k = st.k % (files.length + 1);
        const job = this.#job();
        const gen = ++this.#gen;
        await this.#apply(job, gen, files.slice(0, k));
        this.#pending = { job, gen, rest: [] };
        return;
      }
      case "resume": {
        const p = this.#pending;
        if (p === undefined) return;
        this.#pending = undefined;
        const path = join(s.jobsRoot, p.job, "in", `dirs-${++this.#files}.ndjson`);
        await run(c.indexer.writeKnownDirs(this.lib, p.gen, path));
        const lines = readFileSync(path, "utf8").split("\n").filter((x) => x !== "");
        const known = lines.map((x) => JSON.parse(x) as { dir: { components: string[] }; mtimeNs: string | null; listed: boolean });
        const asKey = (comps: ReadonlyArray<string>) => comps.join("/");
        expect(known.map((k) => ({ chain: asKey(k.dir.components.map(hexOf)), mtimeNs: k.mtimeNs, listed: k.listed }))).toEqual(
          this.model.knownDirs(p.gen).map((d) => ({ chain: asKey(this.model.chain(d.id)), mtimeNs: d.mtimeNs, listed: d.listed })),
        );
        const listed = new Set(known.filter((k) => k.listed).map((k) => asKey(k.dir.components.map(hexOf))));
        const all = new Set(known.map((k) => asKey(k.dir.components.map(hexOf))));
        const items = w.fullScan((ch) => listed.has(asKey(ch)), (ch) => all.has(asKey(ch)));
        return this.#apply(p.job, p.gen, cut(items, st.sizes));
      }
      case "probe":
        return this.#probe();
      case "clock":
        now += st.ms;
        return;
      case "offline": {
        this.model.offline = !this.model.offline;
        await run(c.indexer.setOffline(this.lib, this.model.offline));
        return;
      }
    }
  }

  async #probe(): Promise<void> {
    const { files } = await run(c.indexer.pending(this.lib, { stage: "probe", probeVersion: 1, exts: null, limit: 1000 }));
    expect(files.map((f) => this.#toModel.get(f.fileId)).sort()).toEqual(this.model.pending(1).sort());
    const results: ProbeResult[] = [];
    for (const f of files) {
      const node = this.world.at(f.rel.components.map(hexOf));
      if (node?.kind !== "file" || node.inode.size !== f.size || node.inode.mtimeNs !== f.mtimeNs) continue;
      results.push({
        fileId: f.fileId, size: f.size, mtimeNs: f.mtimeNs, fp: fpOf(node.inode), probeVersion: 1,
        state: node.inode.content % 5 === 0 ? "probeFailed" : "present", kind: "media",
      });
    }
    for (let i = 0; i < results.length; i += 256) {
      const chunk = results.slice(i, i + 256);
      const got = await run(c.indexer.applyProbed(this.lib, chunk));
      const want = this.model.probe(chunk.map((r) => ({ ...r, fileId: this.#toModel.get(r.fileId)! })));
      seen.moved = (seen.moved ?? 0) + got.moved.length;
      expect(got.moved.map((m) => ({ from: this.#toModel.get(m.from), to: this.#toModel.get(m.to) }))).toEqual(want);
    }
  }

  /** Both catalogs, row for row, with model ids paired to catalog ids. */
  compare(): void {
    for (const id of this.model.deleted.slice(this.#deletedSeen)) {
      const cat = this.#toCat.get(id);
      this.#toCat.delete(id);
      if (cat !== undefined) this.#toModel.delete(cat);
    }
    this.#deletedSeen = this.model.deleted.length;
    const db = new DatabaseSync(catalogPath(s.serverData, this.lib), { readOnly: true });
    try {
      const dirs = db
        .prepare(
          "SELECT id, parent, hex(name) AS name, nameNfc, relKey, state, code, placeholder, CAST(mtimeNs AS TEXT) AS mtimeNs, listedGen, " +
            "emptySince, emptyJob, collision, lastSeen FROM dirs ORDER BY id",
        )
        .all() as Array<Record<string, unknown> & { id: number; parent: number | null; name: string }>;
      const byId = new Map(dirs.map((d) => [d.id, d]));
      const dirPath = (id: number): string => {
        const parts: string[] = [];
        for (let d = byId.get(id)!; d.parent !== null; d = byId.get(d.parent)!) parts.unshift(d.name.toLowerCase());
        return parts.join("/");
      };
      const mDirPath = (id: number) => this.model.chain(id).join("/");
      const dirView = (d: Record<string, unknown>) => ({
        nameNfc: d.nameNfc, relKey: d.relKey, state: d.state, code: d.code, placeholder: d.placeholder, mtimeNs: d.mtimeNs,
        listedGen: d.listedGen, emptySince: d.emptySince, emptyJob: d.emptyJob, collision: d.collision, lastSeen: d.lastSeen,
      });
      const catDirs = Object.fromEntries(dirs.map((d) => [dirPath(d.id), dirView(d)]));
      const modelDirs = Object.fromEntries(this.model.dirs.map((d) => [mDirPath(d.id), dirView(d as unknown as Record<string, unknown>)]));
      expect(catDirs).toEqual(modelDirs);

      const files = db
        .prepare(
          "SELECT id, dirId, hex(name) AS name, nameNfc, nameLossy, relKey, ext, size, CAST(mtimeNs AS TEXT) AS mtimeNs, CASE WHEN fileId IS NULL THEN NULL ELSE hex(fileId) END AS fileId, " +
            "dev, placeholder, fp, probeVersion, kind, state, code, renameOf, collision, hardlink, firstSeen, lastSeen, lastChange, missingSince FROM files",
        )
        .all() as Array<Record<string, unknown> & { id: number; dirId: number; name: string; renameOf: number | null }>;
      const catPath = (f: { dirId: number; name: string }) => `${dirPath(f.dirId)}|${f.name.toLowerCase()}`;
      const modelPath = (f: MFile) => `${mDirPath(f.dirId)}|${f.name}`;
      const catByPath = new Map(files.map((f) => [catPath(f), f]));
      expect([...catByPath.keys()].sort()).toEqual(this.model.files.map(modelPath).sort());
      for (const m of this.model.files) {
        const f = catByPath.get(modelPath(m))!;
        const paired = this.#toCat.get(m.id);
        if (paired === undefined) {
          expect(this.#toModel.has(f.id), `catalog row ${f.id} is already another model row's`).toBe(false);
          this.#toCat.set(m.id, f.id);
          this.#toModel.set(f.id, m.id);
        } else expect(f.id, `row at ${modelPath(m)}`).toBe(paired);
      }
      for (const m of this.model.files) {
        const f = catByPath.get(modelPath(m))!;
        const view = {
          nameNfc: f.nameNfc, nameLossy: f.nameLossy === 1, relKey: f.relKey, ext: f.ext, size: f.size, mtimeNs: f.mtimeNs,
          fileId: f.fileId === null ? null : (f.fileId as string).toLowerCase(), dev: f.dev, placeholder: f.placeholder, fp: f.fp,
          probeVersion: f.probeVersion, kind: f.kind, state: f.state, code: f.code,
          renameOf: f.renameOf === null ? null : (this.#toModel.get(f.renameOf) ?? -1), collision: f.collision, hardlink: f.hardlink === 1,
          firstSeen: f.firstSeen, lastSeen: f.lastSeen, lastChange: f.lastChange, missingSince: f.missingSince,
        };
        const { id: _id, dirId: _dirId, name: _name, ...want } = m;
        expect(view, `row at ${modelPath(m)}`).toEqual(want);
      }
      seen.hardlinks = (seen.hardlinks ?? 0) + files.filter((f) => f.hardlink === 1).length;
      seen.pendingRenames = (seen.pendingRenames ?? 0) + files.filter((f) => f.renameOf !== null).length;
      const fts = (db.prepare("SELECT rowid AS id FROM names ORDER BY rowid").all() as Array<{ id: number }>).map((r) => r.id);
      expect(fts).toEqual(files.map((f) => f.id).sort((a, b) => a - b));
    } finally {
      db.close();
    }
  }
}

/** 150 runs take ~7 s here; `FIN_DIFF_RUNS` raises it for a soak, capped at 2,000. */
const RUNS = Math.min(2000, Math.max(1, Number(process.env.FIN_DIFF_RUNS ?? 150) || 150));

test("diff_matches_reference_model", { timeout: 600_000 }, async () => {
  const seedTree = fc.array(fc.record({ op: fc.constant("create" as const), dir: fc.nat(), name: fc.nat(), isDir: fc.boolean(), size: fc.integer({ min: 1, max: 4 }) }), { minLength: 4, maxLength: 16, size: "max" });
  await fc.assert(
    fc.asyncProperty(
      fc.boolean(),
      fc.constantFrom<IdMode>("ids", "ids", "ids", "none", "zeros"),
      seedTree,
      fc.array(step, { minLength: 8, maxLength: 50, size: "max" }),
      async (caseInsensitive, ids, seed, steps) => {
        const r = new Run({ caseInsensitive, ids });
        await r.start();
        try {
          for (const st of seed) await r.step(st);
          await r.step({ op: "full", sizes: [5] });
          r.compare();
          for (const st of steps) {
            await r.step(st);
            r.compare();
          }
        } finally {
          await r.dispose();
        }
      },
    ),
    { numRuns: RUNS },
  );
  console.log(`diff property coverage: ${JSON.stringify(seen)}`);
  // Every run cleaned up after itself.
  expect(readdirSync(s.jobsRoot).filter((d) => d !== "job1")).toEqual([]);
  expect(readdirSync(join(s.serverData, "libraries"))).toEqual([]);
  for (const k of ["renamed", "renamePending", "reappeared", "missing", "suspect", "collisions", "changed", "moved", "hardlinks", "ROOT_EMPTY_SUSPECT"]) {
    expect(seen[k] ?? 0, k).toBeGreaterThan(0);
  }
});
