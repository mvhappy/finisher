// The library kinds without processes (plan 5D Task 9): the family's host calls against a
// recording indexer, the params builders and pools, and what probe outcomes become.
import { mkdtempSync, rmSync } from "node:fs";
import { open as openFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FinError, type JobView } from "@finisher/contracts";
import type { LibraryScanParams, ProbeOutcome, VolumeIdentity } from "@finisher/core-types";
import { jobKindVersions } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { CallCtx, ItemOutcome, KindRegistry, PrepareCtx } from "../../src/jobs/kinds.ts";
import { makeKindRegistry } from "../../src/jobs/kinds.ts";
import type { Catalogs } from "../../src/library/catalog/client.ts";
import { hashParams, probeExtensions, probeParams, readBps, scanParams } from "../../src/library/kind-params.ts";
import { combineCounts, registerLibraryKinds } from "../../src/library/kinds.ts";
import { LibraryPoolBook, libraryPools } from "../../src/library/pools.ts";
import { mediaColumns, planProbed, rationalMs } from "../../src/library/probed.ts";
import type { LibraryRow } from "../../src/library/registry.ts";

const KEY = "0123456789abcdef0123456789abcdef";
const volume: VolumeIdentity = { key: KEY, kind: "apfs", devId: "1", mountRoot: true, caseSensitive: false, enumeration: "fake", remote: false, sample: [] };

function lib(over: Partial<LibraryRow> = {}): LibraryRow {
  return {
    id: "L1", name: "Lib", root: "/media/lib", rootKey: "/media/lib", aliases: [], volume, storage: "local", storageDetected: "local",
    state: "online", exclude: ["*.tmp"], kinds: ["image", "lut"], scan: { poll: "demand", full: "weekly" }, hydrate: false, stages: [],
    gen: 1, createdBy: "local:test", createdAt: 1, ...over,
  };
}

const scratch: string[] = [];
afterEach(() => {
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});

interface Recorded {
  calls: Array<{ method: string; args: unknown[] }>;
}

/** Catalogs whose every method records its arguments and answers a plausible value. */
function recordingCatalogs(): { catalogs: Catalogs; rec: Recorded } {
  const rec: Recorded = { calls: [] };
  const answer: Record<string, unknown> = {
    attemptStart: 4242,
    knownDirs: { text: '{"dir":{"components":[]},"mtimeNs":null,"listed":false,"mountRoot":false}\n', dirs: 1 },
    applyBatchFile: { added: 3, changed: 0, seen: 0, reappeared: 0, renamed: 0, transactions: 1 },
    applyProbed: { updated: 1, stale: 0, moved: [] },
    fileLines: { lines: [{ id: 7, rel: { components: ["YQ==", "Yi5wbmc="] }, expect: { size: 10, mtimeNs: "5", dev: "1:2" } }], absent: [] },
  };
  const proxy = (): unknown =>
    new Proxy({}, { get: (_t, method: string) => (...args: unknown[]) => {
      rec.calls.push({ method, args });
      return Effect.succeed(answer[method] ?? {});
    } });
  return { catalogs: { indexer: proxy(), reader: proxy() } as unknown as Catalogs, rec };
}

function setup() {
  const jobsRoot = mkdtempSync(join(tmpdir(), "fin-libk-unit-"));
  scratch.push(jobsRoot);
  const registry: KindRegistry = makeKindRegistry();
  const { catalogs, rec } = recordingCatalogs();
  const rescans: unknown[] = [];
  const kinds = registerLibraryKinds(registry, {
    catalogs,
    jobsRoot,
    library: () => lib(),
    online: () => Effect.succeed(true),
    events: { rescan: (_l, dirs) => void rescans.push(dirs) },
  });
  return { registry, rec, jobsRoot, kinds, rescans };
}

const job = (id: string, kind: string, params: unknown): JobView =>
  ({ id, kind, kindVersion: 1, family: "library", params, owner: { kind: "system", id: "library" }, priority: "background", state: "running", attempts: 1, maxAttempts: 3, warm: false, createdAt: 1 }) as JobView;

/** A hook ctx whose `open` writes into a temp directory. */
function hookCtx(view: JobView, dir: string): PrepareCtx<never> {
  return {
    job: view as JobView & { params: never },
    attempt: 1,
    open: (name, how = "r") => openFile(join(dir, name.replaceAll("/", "_")), how === "w" ? "w" : how === "rw" ? "r+" : "r"),
  };
}

const exitOf = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(Effect.exit(e));
const failCode = async (e: Effect.Effect<unknown, FinError>): Promise<string> => {
  const x = await exitOf(e);
  if (Exit.isSuccess(x)) throw new Error("expected a failure");
  return (x.cause.reasons.find((r) => r._tag === "Fail") as { error: FinError }).error.code;
};

describe("host calls", () => {
  test("on_call_passes_paths_only", async () => {
    const s = setup();
    const params = scanParams(lib(), { mode: "full", gen: 3 });
    const view = job("J1", "library.scan", params);
    await Effect.runPromise(s.registry.kind("library.scan")!.prepare!(hookCtx(view, s.jobsRoot) as PrepareCtx<never>) as Effect.Effect<unknown, FinError>);
    expect(s.rec.calls.map((c) => c.method)).toEqual(["attemptStart", "knownDirs"]);
    const onCall = s.registry.family("library")!.onCall!;
    const ctx: CallCtx = { job: view, role: "worker", owner: view.owner };
    // The file need not exist: the main thread never opens it, the indexer does.
    const out = await Effect.runPromise(onCall(ctx, "library.batch", { file: "scan-00000000.ndjson", seq: 0 }));
    expect(out).toEqual({ applied: 3 });
    const apply = s.rec.calls.find((c) => c.method === "applyBatchFile")!;
    expect(apply.args).toEqual(["L1", join(s.jobsRoot, "J1", "out", "scan-00000000.ndjson"), { gen: 3, startedAt: 4242, seq: 0 }]);
    const n = s.rec.calls.length;
    for (const file of ["../x.ndjson", "a/b.ndjson", "..", ".", "", ".hidden", "a\\b", "x\u0000y"]) {
      expect(await failCode(onCall(ctx, "library.batch", { file, seq: 1 }))).toBe("INVALID_REQUEST");
    }
    expect(await failCode(onCall(ctx, "library.batch", { file: "ok.ndjson", seq: -1 }))).toBe("INVALID_REQUEST");
    expect(await failCode(onCall({ ...ctx, role: "helper" }, "library.batch", { file: "ok.ndjson", seq: 1 }))).toBe("INVALID_REQUEST");
    expect(await failCode(onCall({ ...ctx, job: job("J2", "library.probe", {}) }, "library.batch", { file: "ok.ndjson", seq: 1 }))).toBe("INVALID_REQUEST");
    expect(await failCode(onCall(ctx, "library.other", {}))).toBe("INVALID_REQUEST");
    expect(s.rec.calls.length).toBe(n);
    // A scan whose attempt has no catalog start (no prepare ran in this process) cannot land batches.
    expect(await failCode(onCall({ ...ctx, job: job("J3", "library.scan", params) }, "library.batch", { file: "ok.ndjson", seq: 0 }))).toBe("INTERNAL");
    s.kinds.jobDone(view);
    expect(s.kinds.live()).toBe(0);
  });

  test("probed_results_land_with_the_attempts_expectations", async () => {
    const s = setup();
    const params = probeParams(lib(), [7])!;
    const view = job("P1", "library.probe", params);
    await Effect.runPromise(s.registry.kind("library.probe")!.prepare!(hookCtx(view, s.jobsRoot) as PrepareCtx<never>) as Effect.Effect<unknown, FinError>);
    const lines = s.rec.calls.find((c) => c.method === "fileLines")!;
    expect(lines.args).toEqual(["L1", { fileIds: [7], hydrate: false }]);
    const onCall = s.registry.family("library")!.onCall!;
    const ctx: CallCtx = { job: view, role: "worker", owner: view.owner };
    const probe = { kind: "image", fingerprint: "fp1:ab", size: 10, image: { width: 4, height: 3 } };
    await Effect.runPromise(onCall(ctx, "library.probed", { results: [{ file: 7, fingerprint: "fp1:ab", probe, probeVersion: 1, state: "present" }] }));
    const applied = s.rec.calls.find((c) => c.method === "applyProbed")!;
    expect(applied.args[1]).toEqual([
      { fileId: 7, size: 10, mtimeNs: "5", fp: "fp1:ab", probe, probeVersion: 1, state: "present", code: null, kind: "image", durationMs: null, width: 4, height: 3 },
    ]);
    // A changed file is never stored: its directory is rescanned.
    await Effect.runPromise(onCall(ctx, "library.probed", { results: [{ file: 7, probeVersion: 1, state: "changed", code: "FILE_CHANGED" }] }));
    expect(s.rescans).toEqual([[{ components: ["YQ=="] }]]);
    // Untrusted: a file outside the job, an unknown state, too many results.
    expect(await failCode(onCall(ctx, "library.probed", { results: [{ file: 8, probeVersion: 1, state: "present" }] }))).toBe("INVALID_REQUEST");
    expect(await failCode(onCall(ctx, "library.probed", { results: [{ file: 7, probeVersion: 1, state: "cancelled" }] }))).toBe("INVALID_REQUEST");
    const many = Array.from({ length: 33 }, () => ({ file: 7, probeVersion: 1, state: "present" }));
    expect(await failCode(onCall(ctx, "library.probed", { results: many }))).toBe("INVALID_REQUEST");
  });
});

describe("params and pools", () => {
  test("bandwidth_splits_over_slots", () => {
    expect(readBps(lib({ bandwidthMiBs: 8, storage: "network" }))).toBe(4 * 1024 * 1024);
    expect(readBps(lib({ bandwidthMiBs: 8, storage: "local" }))).toBe(2 * 1024 * 1024);
    expect(readBps(lib())).toBeUndefined();
    expect(probeParams(lib({ bandwidthMiBs: 8, storage: "sync" }), [1, 2])).toMatchObject({ readBps: 4 * 1024 * 1024, items: ["1", "2"] });
    // A volume shared with a network library: the local one's bandwidth splits over its 2 slots.
    const defined: Array<[string, number]> = [];
    const book = new LibraryPoolBook((name, size) => void defined.push([name, size]));
    book.add({ id: "A", volume, storage: "local" });
    expect(book.readSlots("A")).toBe(4);
    book.add({ id: "B", volume, storage: "network" });
    expect(book.readSlots("A")).toBe(2);
    expect(probeParams(lib({ bandwidthMiBs: 8 }), [1], book.readSlots("A"))?.readBps).toBe(4 * 1024 * 1024);
    book.remove("B");
    expect(book.readSlots("A")).toBe(4);
    expect(defined.at(-1)).toEqual([libraryPools(KEY).read, 4]);
  });

  test("scan_params_from_the_row", () => {
    const p: LibraryScanParams = scanParams(lib({ include: ["**/*.mov"], storage: "network" }), { mode: "dirs", gen: 2, dirs: [{ components: ["YQ=="] }] });
    expect(p).toEqual({
      library: "L1", root: "/media/lib", mode: "dirs", gen: 2, dirs: [{ components: ["YQ=="] }],
      excludes: { caseSensitive: false, exclude: ["*.tmp"], include: ["**/*.mov"] }, threads: 32, volume,
    });
    expect(scanParams(lib(), { mode: "poll", gen: 2 }).dirs).toBeUndefined();
    expect(probeExtensions(["image", "lut"])).toEqual(["bmp", "cube", "gif", "heic", "jpeg", "jpg", "png", "tif", "tiff", "webp"]);
    expect(probeExtensions("all")).toBeUndefined();
    expect(probeParams(lib({ kinds: "all" }), [1])?.extensions).toBeUndefined();
    // Decision 14: a Linux FUSE sync root without hydrate is metadata-only.
    const fuse = lib({ storage: "sync", volume: { ...volume, kind: "fuse.rclone" } });
    if (process.platform === "linux") {
      expect(probeParams(fuse, [1])).toBeUndefined();
      expect(hashParams(fuse, [1])).toBeUndefined();
    }
    expect(probeParams({ ...fuse, hydrate: true }, [1])).toBeDefined();
    expect(() => probeParams(lib(), [])).toThrow();
    expect(() => probeParams(lib(), Array.from({ length: 257 }, (_, i) => i + 1))).toThrow();
    expect(() => hashParams(lib(), Array.from({ length: 65 }, (_, i) => i + 1))).toThrow();
  });

  test("library_kinds_match_rust_params", () => {
    const s = setup();
    const known = jobKindVersions();
    const kinds = s.registry.kinds();
    expect(kinds.map((k) => k.kind).sort()).toEqual(["library.hash", "library.probe", "library.scan"]);
    for (const k of kinds) expect(known, k.kind).toContainEqual({ kind: k.kind, version: k.version });
  });

  test("kinds_declare_pools_dedupe_and_scopes", () => {
    const s = setup();
    const scan = s.registry.kind("library.scan")!;
    const pools = libraryPools(KEY);
    const resources = scan.resources as (p: unknown) => string[];
    expect(resources(scanParams(lib(), { mode: "full", gen: 1 }))).toEqual([pools.scan]);
    expect(resources(scanParams(lib(), { mode: "poll", gen: 1 }))).toEqual([pools.poll]);
    expect(resources(scanParams(lib(), { mode: "dirs", gen: 1, dirs: [] }))).toEqual([pools.poll]);
    expect(scan.dedupe!(scanParams(lib(), { mode: "full", gen: 1 }))).toBe("L1:full");
    expect(scan.dedupe!(scanParams(lib(), { mode: "poll", gen: 1 }))).toBe("L1:poll");
    expect(scan.dedupe!(scanParams(lib(), { mode: "dirs", gen: 1, dirs: [] }))).toBeUndefined();
    expect(scan).toMatchObject({ qos: "utility", stallS: 600, timeoutS: 86_400, resumable: true, prepareEachAttempt: true, scopes: ["library.scan"] });
    const probe = s.registry.kind("library.probe")!;
    expect((probe.resources as (p: unknown) => string[])(probeParams(lib(), [1]))).toEqual([pools.read, "cpu"]);
    expect(probe.dedupe).toBeUndefined();
    expect((s.registry.kind("library.hash")!.resources as (p: unknown) => string[])(hashParams(lib(), [1]))).toEqual([pools.read]);
  });
});

describe("outcomes", () => {
  const expected = new Map([[1, { size: 5, mtimeNs: "9", rel: { components: ["ZA==", "Zg=="] } }]]);
  const o = (state: string, more: Partial<ProbeOutcome> = {}): ProbeOutcome => ({ file: 1, probeVersion: 1, state, ...more });

  test("states_map_to_rows_or_rescans", () => {
    expect(planProbed([o("unreadable", { code: "ASSET_UNREADABLE", fingerprint: "fp1:x" })], expected).rows[0]).toMatchObject({
      state: "probeFailed", code: "ASSET_UNREADABLE", fp: "fp1:x", probe: null,
    });
    expect(planProbed([o("skipped", { code: "KIND_FILTER" })], expected).rows[0]).toMatchObject({ state: "skipped", code: "KIND_FILTER" });
    for (const s of ["changed", "missing", "cloudOnly"]) {
      expect(planProbed([o(s)], expected)).toEqual({ rows: [], rescan: [{ components: ["ZA=="] }] });
    }
    expect(planProbed([o("changed"), o("missing")], expected).rescan).toHaveLength(1);
  });

  test("media_columns_from_probes", () => {
    expect(rationalMs("3/2")).toBe(1500);
    expect(rationalMs("10")).toBe(10_000);
    expect(rationalMs("1/0")).toBeNull();
    expect(rationalMs("x")).toBeNull();
    expect(mediaColumns({ kind: "media", media: { duration: "1001/30", video: { display: { width: 1920, height: 1080 } } } })).toEqual({
      kind: "media", durationMs: 33_367, width: 1920, height: 1080,
    });
    expect(mediaColumns({ kind: "lottieTemplate", lottie: { duration: "2", width: 512, height: 256 } })).toEqual({
      kind: "lottieTemplate", durationMs: 2000, width: 512, height: 256,
    });
    expect(mediaColumns(null)).toEqual({ kind: null, durationMs: null, width: null, height: null });
  });

  test("combine_sums_runs_and_keeps_the_batch_shape", () => {
    const items = new Map<string, ItemOutcome>([
      ["1", {}],
      ["2", { error: new FinError({ code: "POISON_ITEM", message: "x", details: {} }) }],
    ]);
    const r = combineCounts([{ states: { present: 2 }, changed: 1 }, { states: { present: 1, skipped: 1 }, changed: 0, datalessPolicy: "off" }], items) as Record<string, unknown>;
    expect(r).toMatchObject({ states: { present: 3, skipped: 1 }, changed: 1, datalessPolicy: "off", items: { "1": null } });
    expect((r.itemErrors as unknown[]).length).toBe(1);
  });
});
