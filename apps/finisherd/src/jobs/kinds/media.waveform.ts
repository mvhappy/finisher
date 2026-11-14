// The `media.waveform` kind (6C plan Task 19, Decisions 7, 19 and 20; sub-project 6 spec §7.2): an
// asset's waveform peaks (`peaks.bin`) as a `wave` cache entry, made by `fin-workers media`, cold,
// committed into the media cache.
// - A batch of one item, the fingerprint. The source is a project asset (`prepare` writes the
//   one-asset manifest and fails `ASSET_CHANGED` when the fingerprint moved).
// - **From the asset's `pcm48` entry** when `pcmKey` names one the cache holds: `prepare` writes
//   `in/pcm.json {key, dir}` (the entry's canonical directory) and grants the worker the entry's
//   files, read only. The worker verifies the entry and reads the original instead when it is not
//   usable (evicted meanwhile, changed), so no pin is held: the bytes are the same either way.
// - Priority `background`, `qos: background`, pool `cpu`; memory `batch`'s `min(4 GiB, 25 %)`.
// - Identity (`dedupe`): the cache key. `crashRestarts` and `rememberPoison` as `media.pcm`: a worker
//   crash on this one item is retried from the start while attempts remain, and poisons it only
//   once they are spent.
// - A retry restarts from sample 0 (Decision 23; the entry is one small file). `onSucceeded`: the
//   item's error fails the job with it; otherwise `out/<key>/` is committed by `MediaCache.commit`
//   for the project that asked.
import { realpath, writeFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { EntryMeta, WaveParams } from "@finisher/core-types";
import type { MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { BATCH_MEMORY } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec } from "../kinds.ts";
import { combineWithStats, mediaJournal } from "./media.journal.ts";
import { outcomeOf } from "./media.proxy.ts";

export const MEDIA_WAVEFORM = "media.waveform";

/** The longest a waveform may take: a day-long source on a slow machine. */
export const WAVE_TIMEOUT_S = 4 * 3600;

export interface WaveKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** The media cache (`undefined` when it did not open: the commit is then `CACHE_IO`). */
  readonly cache: () => MediaCache | undefined;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

/** `["media.waveform", key]`: one job per cache entry (per owner, 5C-2 Decision 7). */
export function waveDedupe(p: WaveParams): string {
  return JSON.stringify([MEDIA_WAVEFORM, p.key]);
}

const invalid = (field: string, why: string): FinError =>
  finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "ask for a waveform through media.waveform");

async function canonical(paths: ReadonlyArray<string>): Promise<string[]> {
  const out: string[] = [];
  for (const p of paths) {
    if (!isAbsolute(p)) continue;
    try {
      out.push(await realpath(p));
    } catch {
      // Missing or unreadable: not granted (the worker reports it).
    }
  }
  return out.sort();
}

function readsOf(extra: unknown): ReadonlyArray<string> {
  const reads = (extra as { reads?: unknown } | null | undefined)?.reads;
  return Array.isArray(reads) ? reads.filter((r): r is string => typeof r === "string") : [];
}

/**
 * The `pcm48` entry `key` as `{dir, files}` (canonical paths of `meta.json` and the chunks), or
 * `undefined` when the cache does not hold it.
 */
async function pcmEntry(cache: MediaCache | undefined, key: string): Promise<{ readonly dir: string; readonly files: string[] } | undefined> {
  if (cache === undefined) return undefined;
  try {
    const meta = (await cache.lookup(key)) as EntryMeta | null;
    const dir = cache.entryDir(key);
    if (meta === null || dir == null) return undefined;
    const real = await realpath(dir);
    return { dir: real, files: ["meta.json", ...meta.files.map((f) => f.name)].map((n) => join(real, n)) };
  } catch {
    return undefined;
  }
}

/** `media.waveform` v1. */
export function mediaWaveformKind(deps: WaveKindDeps): KindSpec<WaveParams, unknown> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_WAVEFORM,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    resources: ["cpu"],
    warm: false,
    defaultPriority: "background",
    qos: "background",
    timeoutS: WAVE_TIMEOUT_S,
    stallS: 600,
    maxAttempts: 2,
    batch: { items: (p) => [p.fingerprint], restrict: (p) => p, combine: combineWithStats },
    dedupe: waveDedupe,
    journal: mediaJournal,
    rememberPoison: true,
    crashRestarts: true,
    memory: BATCH_MEMORY,
    files: (p) => [{ name: "in/manifest.json", mode: "read" as const }, ...(p.pcmKey == null ? [] : [{ name: "in/pcm.json", mode: "read" as const }]), { name: "out/", mode: "write" as const }],
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required"));
        const dirs = jobDirs(root, ctx.job.id);
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const done = yield* deps.projects.onBranch<{ rev: number; fingerprint: string; sources: ReadonlyArray<string> }>(p.projectId, p.branch ?? "main", (engine, branch) =>
          engine.prepareAssetJob(branch, p.asset, join(dirs.job, "in")),
        );
        if (done.fingerprint !== p.fingerprint) {
          return yield* Effect.fail(finError("ASSET_CHANGED", `asset ${p.asset} changed since its waveform was asked for`, { asset: p.asset, reason: "fingerprint" }, "ask for the waveform again"));
        }
        const entry = p.pcmKey == null ? undefined : yield* Effect.promise(() => pcmEntry(deps.cache(), p.pcmKey!));
        if (p.pcmKey != null) {
          // An entry gone since the ask: an empty directory, which the worker does not use.
          const ref = JSON.stringify({ key: p.pcmKey, dir: entry?.dir ?? "" });
          yield* Effect.tryPromise({ try: () => writeFile(join(dirs.job, "in", "pcm.json"), ref, { flag: "wx", mode: 0o600 }), catch: toFinError });
        }
        const reads = yield* Effect.promise(() => canonical([...done.sources, ...(entry?.files ?? [])]));
        return { extra: { rev: done.rev, reads } };
      }),
    reads: readsOf,
    onSucceeded: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        const failed = outcomeOf(ctx.items, p.fingerprint);
        if (failed !== undefined) return yield* Effect.fail(failed);
        const cache = deps.cache();
        if (cache === undefined) {
          return yield* Effect.fail(finError("CACHE_IO", "the media cache is not open", { reason: "io" }, "check the volume that holds the server data, then restart finisherd"));
        }
        const out = join(jobDirs(root, ctx.job.id).job, "out");
        yield* Effect.tryPromise({ try: () => cache.commit(out, p.key, p.projectId == null ? [] : [p.projectId]), catch: toFinError });
        return ctx.result;
      }),
  };
}
