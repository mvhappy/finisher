// The library kinds and the `library` worker family on the shared engine (plan 5D Task 9;
// Decisions 26, 27, 32–34, 38; spec §6.0: 5D contributes only this family).
//
// - **`library.scan`** (`full | dirs | poll`): one slot of the volume's `scan` pool (full) or
//   `poll` pool (polls and targeted scans), so a poll never waits behind a full scan.
//   `prepare` runs at **every attempt** (`prepareEachAttempt`): it takes the catalog's
//   `attemptStart` before the worker reads anything and writes `in/dirs.ndjson` (the known
//   directories, with `listed` for this generation: a resumed full scan skips what it already
//   listed but still descends). The worker hands each batch file over with `call library.batch
//   {file, seq}`; `onCall` checks only that `file` is a plain name and passes **the path** in
//   the job's `out/` to the indexer, which opens, validates and applies it (the main thread
//   never reads a batch), with that attempt's start.
// - **`library.probe`** (batch ≤ 256 file ids): `prepare` (every attempt) writes
//   `in/files.ndjson` from the catalog (raw chains and `Expect` with `dev`); results arrive
//   through `call library.probed {results ≤ 32}`, are checked and land with the attempt's
//   size and mtime (a row changed since is left alone). A crash isolates the files in flight
//   (5C-1 Decision 32; the up to 32 begun-but-unacknowledged items are a set); `onSucceeded`
//   records each `POISON_ITEM` as `probeFailed`, not retried until the file changes.
// - **`library.hash`** (batch ≤ 64): item values `blake3:<hex>`; `onSucceeded` stores them.
//
// Every kind answers `resourceHealthy` from the library's root state (Decision 33), so a crash
// on a dying mount is a resource fault, never a poisoned file. Hooks read job files only
// through `ctx.open`. Outcomes the catalog cannot store (`changed`, `missing`, `cloudOnly`)
// become `events.rescan` of their directories (the service lists them, Task 10).
import { join } from "node:path";
import type { LibraryHashParams, LibraryProbeParams, LibraryScanParams, RawRel } from "@finisher/core-types";
import { FinError, type JobView, type MappedFile } from "@finisher/contracts";
import { libraryProbeVersion } from "@finisher/fin-node";
import { Effect } from "effect";
import { toFinError } from "../errors.ts";
import type { CallCtx, ItemOutcome, KindRegistry, KindSpec, OnCall, PrepareCtx } from "../jobs/kinds.ts";
import { defaultCombine } from "../jobs/retry.ts";
import type { BatchApplied, Catalogs, ProbedApplied } from "./catalog/client.ts";
import { HASH_BATCH, PROBE_BATCH } from "./kind-params.ts";
import { type LibraryPools, libraryPools } from "./pools.ts";
import { type ExpectedFile, decodeProbed, planProbed } from "./probed.ts";
import type { LibraryRow } from "./registry.ts";

export const LIBRARY_FAMILY = "library";
export const CALL_BATCH = "library.batch";
export const CALL_PROBED = "library.probed";
/** Outcomes per `library.probed` call (Decision 34). */
export const PROBED_MAX = 32;
export const DIRS_FILE = "in/dirs.ndjson";
export const FILES_FILE = "in/files.ndjson";
const OUT_DIR = "out";
const SCOPES = ["library.scan"] as const;
/** Item error codes of a hash whose file is not what the catalog says: its directory is rescanned. */
const RESCAN_CODES = new Set(["FILE_CHANGED", "FILE_NOT_FOUND", "FILE_CLOUD_ONLY"]);
const BATCH_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const HASH = /^blake3:[0-9a-f]{64}$/;

/** What the kinds tell the service (Task 10); every callback is optional. */
export interface LibraryKindEvents {
  /** A scan batch file landed in the catalog. */
  batchApplied?(lib: string, job: string, applied: BatchApplied): void;
  /** Probe outcomes landed (`moved`: rows merged by fingerprint). */
  probed?(lib: string, job: string, applied: ProbedApplied): void;
  /** Directories to list again: files a probe or hash found changed, missing or cloud-only. */
  rescan?(lib: string, dirs: ReadonlyArray<RawRel>): void;
  /** A library job ended `LIBRARY_OFFLINE` (Decision 33): the service puts the library offline. */
  offline?(lib: string, job: JobView): void;
}

export interface LibraryKindsDeps {
  readonly catalogs: Pick<Catalogs, "indexer" | "reader">;
  /** `<server-data>/jobs` (the engine's root): batch files are `<root>/<job>/out/<file>`. */
  readonly jobsRoot: string;
  /** The library's row (the registry's `get`), for params that do not carry its volume. */
  readonly library: (id: string) => LibraryRow | undefined;
  /** Whether the library's root is usable now (`resourceHealthy`, Decision 33). */
  readonly online: (id: string) => Effect.Effect<boolean>;
  readonly events?: LibraryKindEvents;
}

export interface LibraryKinds {
  /** Drops the per-attempt state of an ended job and reports `LIBRARY_OFFLINE`; wire to `JobEngine.onDone`. */
  jobDone(job: JobView): void;
  /** Jobs with per-attempt state (tests). */
  live(): number;
}

interface AttemptState {
  /** The catalog's `attemptStart` of the current attempt (scans). */
  startedAt?: number;
  /** What the current attempt's worker reads (probes). */
  expected?: Map<number, ExpectedFile>;
}

const invalid = (method: string, why: string, details: Record<string, unknown> = {}): FinError =>
  new FinError({ code: "INVALID_REQUEST", message: `${method}: ${why}`, details: { method, ...details } });

const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

/** A volume with no known key: its pools are never defined, so a job on it cannot be submitted. */
const NO_POOLS: LibraryPools = { scan: "io:none.scan", poll: "io:none.poll", read: "io:none.read" };

/** Writes `text` to a job file through the hook's `open` (created or truncated, 0600). */
async function writeJobFile(ctx: PrepareCtx<unknown>, name: string, text: string): Promise<void> {
  const fh = await ctx.open(name, "w");
  try {
    await fh.writeFile(text);
  } finally {
    await fh.close();
  }
}

/** The `in/files.ndjson` a hook wrote, read back through `ctx.open` (a worker may have tampered with it: every line is checked). */
async function readFileLines(ctx: PrepareCtx<unknown>): Promise<Map<number, ExpectedFile>> {
  const fh = await ctx.open(FILES_FILE, "r");
  let text: string;
  try {
    text = await fh.readFile("utf8");
  } finally {
    await fh.close();
  }
  const out = new Map<number, ExpectedFile>();
  for (const line of text.split("\n")) {
    if (line === "") continue;
    const v = JSON.parse(line) as unknown;
    const ok =
      isObj(v) && Number.isSafeInteger(v.id) && isObj(v.rel) && Array.isArray(v.rel.components) &&
      v.rel.components.every((c) => typeof c === "string") && isObj(v.expect) && Number.isSafeInteger(v.expect.size) &&
      typeof v.expect.mtimeNs === "string" && /^-?\d{1,19}$/.test(v.expect.mtimeNs);
    if (!ok) throw new FinError({ code: "JOB_FAILED", message: "the job's in/files.ndjson is not as prepared", details: { file: FILES_FILE } });
    const e = v.expect as { size: number; mtimeNs: string };
    out.set(v.id as number, { size: e.size, mtimeNs: e.mtimeNs, rel: v.rel as unknown as RawRel });
  }
  return out;
}

const fx = <A>(f: () => Promise<A>): Effect.Effect<A, FinError> => Effect.tryPromise({ try: f, catch: toFinError });

/** Registers the `library` family and its three kinds. */
export function registerLibraryKinds(registry: KindRegistry, deps: LibraryKindsDeps): LibraryKinds {
  const { indexer, reader } = deps.catalogs;
  const attempts = new Map<string, AttemptState>();
  const state = (job: string): AttemptState => {
    let s = attempts.get(job);
    if (s === undefined) attempts.set(job, (s = {}));
    return s;
  };
  const poolsOf = (p: { readonly library: string; readonly volume?: { readonly key: string } | null }): LibraryPools => {
    const key = p.volume?.key ?? deps.library(p.library)?.volume.key;
    if (key === undefined) return NO_POOLS;
    try {
      return libraryPools(key);
    } catch {
      return NO_POOLS;
    }
  };
  const healthy = (job: JobView & { params: { library: string } }) => deps.online(job.params.library);

  /** `call library.batch {file, seq}` of a scan's worker: the indexer applies `out/<file>`. */
  const onBatch = (ctx: CallCtx, params: unknown): Effect.Effect<unknown, FinError> => {
    if (ctx.job.kind !== "library.scan") return Effect.fail(invalid(CALL_BATCH, "only a scan hands over batch files"));
    if (!isObj(params) || typeof params.file !== "string" || !Number.isSafeInteger(params.seq) || (params.seq as number) < 0) {
      return Effect.fail(invalid(CALL_BATCH, "params are {file, seq}"));
    }
    const file = params.file;
    if (!BATCH_NAME.test(file) || file === "." || file === "..") return Effect.fail(invalid(CALL_BATCH, "file must be a plain name in out/", { at: "file" }));
    const startedAt = attempts.get(ctx.job.id)?.startedAt;
    if (startedAt === undefined) {
      return Effect.fail(new FinError({ code: "INTERNAL", message: "the scan's attempt has no catalog start", details: { job: ctx.job.id } }));
    }
    const p = ctx.job.params as LibraryScanParams;
    const path = join(deps.jobsRoot, ctx.job.id, OUT_DIR, file);
    return indexer.applyBatchFile(p.library, path, { gen: p.gen, startedAt, seq: params.seq as number }).pipe(
      Effect.tap((applied) => Effect.sync(() => deps.events?.batchApplied?.(p.library, ctx.job.id, applied))),
      Effect.map((applied) => ({ applied: applied.added + applied.changed + applied.seen + applied.reappeared + applied.renamed })),
    );
  };

  /** `call library.probed {results}` of a probe's worker: checked, then stored with the attempt's expectations. */
  const onProbed = (ctx: CallCtx, params: unknown): Effect.Effect<unknown, FinError> => {
    if (ctx.job.kind !== "library.probe") return Effect.fail(invalid(CALL_PROBED, "only a probe reports outcomes"));
    const expected = attempts.get(ctx.job.id)?.expected;
    if (expected === undefined) {
      return Effect.fail(new FinError({ code: "INTERNAL", message: "the probe's attempt has no prepared files", details: { job: ctx.job.id } }));
    }
    const lib = (ctx.job.params as LibraryProbeParams).library;
    return Effect.try({ try: () => planProbed(decodeProbed(params, PROBED_MAX, libraryProbeVersion()), expected), catch: toFinError }).pipe(
      Effect.flatMap((plan) => {
        if (plan.rescan.length > 0) deps.events?.rescan?.(lib, plan.rescan);
        if (plan.rows.length === 0) return Effect.succeed({ updated: 0, stale: 0, moved: [] } satisfies ProbedApplied);
        return indexer.applyProbed(lib, plan.rows);
      }),
      Effect.tap((applied) => Effect.sync(() => deps.events?.probed?.(lib, ctx.job.id, applied))),
      Effect.map((applied) => ({ updated: applied.updated, stale: applied.stale })),
    );
  };

  const onCall: OnCall = (ctx, method, params) => {
    if (ctx.role !== "worker") return Effect.fail(invalid(method, "only the family's worker calls the host"));
    if (method === CALL_BATCH) return onBatch(ctx, params);
    if (method === CALL_PROBED) return onProbed(ctx, params);
    return Effect.fail(invalid(method, `no host call ${method}`));
  };

  registry.registerFamily({ family: LIBRARY_FAMILY, profile: "worker", program: { binary: "fin-workers", args: ["library"] }, onCall });

  const scanFiles: ReadonlyArray<MappedFile> = [
    { name: `${OUT_DIR}/`, mode: "write" },
    { name: DIRS_FILE, mode: "read" },
  ];
  const scan: KindSpec<LibraryScanParams> = {
    // The library root, read under the worker profile (plan 7C Task 7; params carry it canonical).
    reads: (_extra, p) => [p.root],
    kind: "library.scan",
    version: 1,
    family: LIBRARY_FAMILY,
    scopes: SCOPES,
    resources: (p) => [p.mode === "full" ? poolsOf(p).scan : poolsOf(p).poll],
    defaultPriority: "background",
    qos: "utility",
    timeoutS: 86_400,
    stallS: 600,
    resumable: true,
    files: scanFiles,
    // Full scans and polls of one library run once at a time; targeted scans are not deduplicated.
    dedupe: (p) => (p.mode === "dirs" ? undefined : `${p.library}:${p.mode}`),
    resourceHealthy: healthy,
    prepareEachAttempt: true,
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        // Before the worker reads anything: every listing of this attempt starts here (7b review).
        const startedAt = yield* indexer.attemptStart(p.library);
        const known = yield* indexer.knownDirs(p.library, p.gen);
        yield* fx(() => writeJobFile(ctx as PrepareCtx<unknown>, DIRS_FILE, known.text));
        state(ctx.job.id).startedAt = startedAt;
      }),
  };

  const batchOf = <P extends { items: string[] }>(): NonNullable<KindSpec<P>["batch"]> => ({
    items: (p) => p.items,
    restrict: (p, items) => ({ ...p, items: [...items] }),
    combine: (results, items) => combineCounts(results, items),
  });

  /** Writes `in/files.ndjson` for the job's files; answers what the worker will read. */
  const prepareFiles = (ctx: PrepareCtx<LibraryProbeParams | LibraryHashParams>) =>
    Effect.gen(function* () {
      const p = ctx.job.params;
      const fileIds = p.items.map(Number).filter((n) => Number.isSafeInteger(n) && n > 0);
      const { lines } = yield* reader.fileLines(p.library, { fileIds, hydrate: p.hydrate });
      const text = lines.map((l) => JSON.stringify(l)).join("\n");
      yield* fx(() => writeJobFile(ctx as PrepareCtx<unknown>, FILES_FILE, text === "" ? "" : `${text}\n`));
      return new Map(lines.map((l) => [l.id, { size: l.expect.size, mtimeNs: l.expect.mtimeNs, rel: l.rel }] as const));
    });

  const probe: KindSpec<LibraryProbeParams> = {
    // The library root, read under the worker profile (plan 7C Task 7; params carry it canonical).
    reads: (_extra, p) => [p.root],
    kind: "library.probe",
    version: 1,
    family: LIBRARY_FAMILY,
    scopes: SCOPES,
    resources: (p) => [poolsOf(p).read, "cpu"],
    defaultPriority: "background",
    qos: "utility",
    timeoutS: 3600,
    stallS: 600,
    batch: batchOf<LibraryProbeParams>(),
    files: [{ name: FILES_FILE, mode: "read" }],
    resourceHealthy: healthy,
    // Every attempt reads the catalog's current rows; `library.probed` lands against them.
    prepareEachAttempt: true,
    prepare: (ctx) =>
      prepareFiles(ctx).pipe(
        Effect.tap((expected) => Effect.sync(() => void (state(ctx.job.id).expected = expected))),
        Effect.asVoid,
      ),
    onSucceeded: (ctx) =>
      Effect.gen(function* () {
        const poisoned = [...ctx.items].filter(([, o]) => "error" in o && o.error.code === "POISON_ITEM").map(([item]) => Number(item));
        if (poisoned.length === 0) return;
        const expected = yield* fx(() => readFileLines(ctx as PrepareCtx<unknown>));
        const p = ctx.job.params;
        const rows = poisoned.flatMap((id) => {
          const e = expected.get(id);
          if (e === undefined) return [];
          // At this probe version: not picked again until the file changes or the version does.
          return [{ fileId: id, size: e.size, mtimeNs: e.mtimeNs, probeVersion: libraryProbeVersion(), state: "probeFailed" as const, code: "POISON_ITEM" }];
        });
        if (rows.length > 0) yield* indexer.applyProbed(p.library, rows);
      }),
  };

  const hash: KindSpec<LibraryHashParams> = {
    // The library root, read under the worker profile (plan 7C Task 7; params carry it canonical).
    reads: (_extra, p) => [p.root],
    kind: "library.hash",
    version: 1,
    family: LIBRARY_FAMILY,
    scopes: SCOPES,
    resources: (p) => [poolsOf(p).read],
    defaultPriority: "normal",
    qos: "utility",
    timeoutS: 6 * 3600,
    stallS: 600,
    batch: batchOf<LibraryHashParams>(),
    files: [{ name: FILES_FILE, mode: "read" }],
    resourceHealthy: healthy,
    // Once per job: the stored hashes are matched against the expectations they were read with.
    prepare: (ctx) => prepareFiles(ctx).pipe(Effect.asVoid),
    onSucceeded: (ctx) =>
      Effect.gen(function* () {
        const expected = yield* fx(() => readFileLines(ctx as PrepareCtx<unknown>));
        const p = ctx.job.params;
        const hashes: Array<{ fileId: number; size: number; mtimeNs: string; fullHash: string }> = [];
        const dirs = new Map<string, RawRel>();
        for (const [item, o] of ctx.items) {
          const e = expected.get(Number(item));
          if (e === undefined) continue;
          if ("error" in o) {
            if (RESCAN_CODES.has(o.error.code)) {
              const d = { components: e.rel.components.slice(0, -1) };
              dirs.set(d.components.join("/"), d);
            }
          } else if (typeof o.value === "string" && HASH.test(o.value)) {
            hashes.push({ fileId: Number(item), size: e.size, mtimeNs: e.mtimeNs, fullHash: o.value });
          }
        }
        if (dirs.size > 0) deps.events?.rescan?.(p.library, [...dirs.values()]);
        for (let i = 0; i < hashes.length; i += 1000) yield* indexer.applyHashes(p.library, hashes.slice(i, i + 1000));
      }),
  };

  registry.registerKind(scan);
  registry.registerKind(probe);
  registry.registerKind(hash);

  return {
    jobDone(job) {
      attempts.delete(job.id);
      if (job.family !== LIBRARY_FAMILY || job.error?.code !== "LIBRARY_OFFLINE") return;
      const lib = (job.params as { library?: unknown } | null)?.library;
      if (typeof lib === "string") deps.events?.offline?.(lib, job);
    },
    live: () => attempts.size,
  };
}

/**
 * A probe or hash job's result: the default batch shape (`items`, `itemErrors`) plus the
 * workers' counts by state, summed over the job's runs.
 */
export function combineCounts(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown {
  const states: Record<string, number> = {};
  let changed = 0;
  let datalessPolicy: string | undefined;
  for (const r of results) {
    if (!isObj(r)) continue;
    if (isObj(r.states)) for (const [k, n] of Object.entries(r.states)) if (typeof n === "number") states[k] = (states[k] ?? 0) + n;
    if (typeof r.changed === "number") changed += r.changed;
    if (typeof r.datalessPolicy === "string") datalessPolicy = r.datalessPolicy;
  }
  return { ...(defaultCombine(items) as object), states, changed, ...(datalessPolicy === undefined ? {} : { datalessPolicy }) };
}

export { HASH_BATCH, PROBE_BATCH };
