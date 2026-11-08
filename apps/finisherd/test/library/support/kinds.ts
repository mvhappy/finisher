// A real job engine with the `library` family over the test-hooks `fin-workers`, real catalog
// threads and a registry in the engine's `control.db` (plan 5D Task 9). Library roots are real
// temp trees, outside the server data; a `.fin-fake.json` overlay at a root slows, breaks or
// poisons its paths.
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { JobView } from "@finisher/contracts";
import type { LibraryScanParams, Timing } from "@finisher/core-types";
import { Effect } from "effect";
import { type Catalogs, startCatalogThreads } from "../../../src/library/catalog/client.ts";
import { rootHealth } from "../../../src/library/health.ts";
import { scanParams, type ScanRequest } from "../../../src/library/kind-params.ts";
import { type LibraryKindEvents, type LibraryKinds, registerLibraryKinds } from "../../../src/library/kinds.ts";
import { LibraryPoolBook } from "../../../src/library/pools.ts";
import { finNodeRootTools, type LibraryRow, type NewLibrary, Registry } from "../../../src/library/registry.ts";
import { OWNER, run, type TestEngine, testEngine } from "../../support/jobs.ts";

export const PNG_1X1 = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

/** Scaled walker deadlines: batches flush every 50 ms. */
export const FAST: Timing = { firstStat: 60_000, entry: 30_000, flush: 50 };

/** Everything a rig's events reported. */
export interface Seen {
  readonly batches: Array<{ lib: string; job: string }>;
  readonly rescans: Array<{ lib: string; dirs: number }>;
  readonly offline: Array<{ lib: string; job: string }>;
}

export interface Rig {
  readonly t: TestEngine;
  readonly catalogs: Catalogs;
  readonly registry: Registry;
  readonly kinds: LibraryKinds;
  readonly book: LibraryPoolBook;
  readonly serverData: string;
  readonly seen: Seen;
  /** Adds a library over `root`, opens its catalog and defines its pools. */
  library(root: string, input?: Omit<NewLibrary, "root">): Promise<LibraryRow>;
  /** Submits a scan of `lib` (threads and timing overridable). */
  scan(lib: LibraryRow, req: Partial<ScanRequest> & { threads?: number }, spec?: Record<string, unknown>): Promise<JobView>;
  /** Stops engine and catalogs as if `finisherd` died, keeping the data. */
  crash(): Promise<void>;
  close(): Promise<void>;
}

export interface RigOptions {
  readonly online?: (id: string) => Effect.Effect<boolean>;
  readonly serverData?: string;
  readonly cores?: number;
}

const rigs: Rig[] = [];
const dirs: string[] = [];

/** A temp directory for library roots (never inside a server data folder). */
export function rootsDir(): string {
  const d = realpathSync(mkdtempSync(join(tmpdir(), "fin-libk-")));
  dirs.push(d);
  return d;
}

/** Writes a tree under `root`: `files[dir] = names` (each file `size` bytes, PNGs for `.png`). */
export function tree(root: string, files: Readonly<Record<string, ReadonlyArray<string>>>, size = 16): void {
  for (const [dir, names] of Object.entries(files)) {
    const d = join(root, dir);
    mkdirSync(d, { recursive: true });
    for (const n of names) writeFileSync(join(d, n), n.endsWith(".png") ? PNG_1X1 : Buffer.alloc(size, 7));
  }
}

/** `count` directories `d00…` of `per` files each. */
export function grid(count: number, per: number, ext = "bin"): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (let d = 0; d < count; d++) out[`d${String(d).padStart(2, "0")}`] = Array.from({ length: per }, (_, i) => `f${String(i).padStart(3, "0")}.${ext}`);
  return out;
}

export function overlay(root: string, spec: Record<string, unknown>): void {
  writeFileSync(join(root, ".fin-fake.json"), JSON.stringify(spec));
}

export async function libraryRig(opts: RigOptions = {}): Promise<Rig> {
  const serverData = opts.serverData ?? realpathSync(mkdtempSync(join(tmpdir(), "fin-libk-data-")));
  if (opts.serverData === undefined) dirs.push(serverData);
  const jobsRoot = join(serverData, "jobs");
  mkdirSync(jobsRoot, { recursive: true, mode: 0o700 });
  const catalogs = await startCatalogThreads({ serverData, jobsRoot, testHooks: true });
  const seen: Seen = { batches: [], rescans: [], offline: [] };
  const events: LibraryKindEvents = {
    batchApplied: (lib, job) => void seen.batches.push({ lib, job }),
    rescan: (lib, d) => void seen.rescans.push({ lib, dirs: d.length }),
    offline: (lib, job) => void seen.offline.push({ lib, job: job.id }),
  };
  const holder: { registry?: Registry; kinds?: LibraryKinds } = {};
  const health = rootHealth({ library: (id) => holder.registry?.get(id), probeRoot: finNodeRootTools.probeRoot });
  let t: TestEngine;
  try {
    t = await testEngine({
      serverData,
      keepData: true,
      cores: opts.cores ?? 4,
      register: (kinds) => {
        holder.kinds = registerLibraryKinds(kinds, {
          catalogs,
          jobsRoot,
          library: (id) => holder.registry?.get(id),
          online: opts.online ?? ((id) => health.online(id)),
          events,
        });
      },
    });
  } catch (e) {
    await catalogs.shutdown();
    throw e;
  }
  const registry = new Registry(t.control, { serverData });
  holder.registry = registry;
  const kinds = holder.kinds!;
  const unsubscribe = t.engine.onDone((job) => kinds.jobDone(job));
  const book = new LibraryPoolBook((name, size) => t.engine.definePool(name, size));
  for (const lib of registry.list()) {
    book.add(lib);
    await run(catalogs.indexer.open(lib.id));
  }
  let stopped = false;
  const rig: Rig = {
    t,
    catalogs,
    registry,
    kinds,
    book,
    serverData,
    seen,
    async library(root, input = {}) {
      const lib = await run(registry.add({ root, ...input }, { kind: "local", id: "test" }));
      book.add(lib);
      await run(catalogs.indexer.open(lib.id));
      return lib;
    },
    async scan(lib, req, spec = {}) {
      const { threads, ...rest } = req;
      const params: LibraryScanParams = {
        ...scanParams(lib, { mode: "full", gen: 1, timing: FAST, ...rest }),
        ...(threads === undefined ? {} : { threads }),
      };
      return run(t.engine.submit(OWNER, { kind: "library.scan", params, library: lib.id, ...spec }));
    },
    async crash() {
      if (stopped) return;
      stopped = true;
      unsubscribe();
      await t.crash();
      await catalogs.shutdown();
    },
    async close() {
      if (stopped) return;
      stopped = true;
      unsubscribe();
      await t.close();
      await catalogs.shutdown();
    },
  };
  rigs.push(rig);
  return rig;
}

/** Closes every rig and removes every temp directory (call from `afterEach`). */
export async function closeRigs(): Promise<void> {
  for (const r of rigs.splice(0)) await r.close();
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
}

/** Every catalogued file id of `lib` that the probe stage would take now. */
export async function pendingIds(rig: Rig, lib: string, limit = 1000): Promise<number[]> {
  const { files } = await run(rig.catalogs.indexer.pending(lib, { stage: "probe", probeVersion: 1, exts: null, limit }));
  return files.map((f) => f.fileId);
}

export const counts = (rig: Rig, lib: string) => run(rig.catalogs.reader.counts(lib));
