// The `media.pcm` kind (6C plan Task 18, Decisions 7, 20 and 23; sub-project 6 spec §7.2): an
// asset's or a library file's audio as a `pcm48` or `pcm16m` cache entry, made by
// `fin-workers media`, cold, committed into the media cache.
// - A batch of one item, the fingerprint, so the `pcm16m` library stage (5D, whose kinds are
//   batches of fingerprints) and a project's `media.pcm` share it. The source is a project asset
//   (`asset`: `prepare` writes the one-asset manifest and fails `ASSET_CHANGED` when the
//   fingerprint moved) or a library file (`file`), as `media.proxy`'s.
// - Priority `background`, `qos: background`, pool `cpu`; memory `batch`'s `min(4 GiB, 25 %)`.
// - Identity (`dedupe`): the cache key. `crashRestarts`: a worker crash mid-decode is retried from
//   sample 0 (an attempt spent), and only a crash of the last attempt poisons the source, which
//   `rememberPoison` then keeps until its key changes (a new fingerprint, `AUDIO_VERSION` or
//   decoder class); a crash after the item was answered commits it (the entry was complete).
// - **Retries restart from sample 0** (Decision 23): not resumable, no checkpoint; 5C empties
//   `out/` before every attempt, and a `finisherd` restart interrupts the job (`JOB_INTERRUPTED`),
//   so the next ask runs a new one from the start. Nothing is indexed until `onSucceeded`.
// - `onSucceeded`: the item's error fails the job with it; otherwise `out/<key>/` is committed by
//   `MediaCache.commit` (which checks every listed size and BLAKE3) for the project that asked.
import { realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { PcmParams } from "@finisher/core-types";
import type { MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { BATCH_MEMORY } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec } from "../kinds.ts";
import { outcomeOf } from "./media.proxy.ts";

export const MEDIA_PCM = "media.pcm";

/** The longest a feed may take: a day-long source on a slow machine. */
export const PCM_TIMEOUT_S = 4 * 3600;

export interface PcmKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** The media cache (`undefined` when it did not open: the commit is then `CACHE_IO`). */
  readonly cache: () => MediaCache | undefined;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

/** `["media.pcm", key]`: one job per cache entry (per owner, 5C-2 Decision 7). */
export function pcmDedupe(p: PcmParams): string {
  return JSON.stringify([MEDIA_PCM, p.key]);
}

const invalid = (field: string, why: string): FinError =>
  finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "ask for a PCM feed through media.pcm or the pcm16m stage");

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

/** `media.pcm` v1. */
export function mediaPcmKind(deps: PcmKindDeps): KindSpec<PcmParams, unknown> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_PCM,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    resources: ["cpu"],
    warm: false,
    defaultPriority: "background",
    qos: "background",
    timeoutS: PCM_TIMEOUT_S,
    stallS: 600,
    maxAttempts: 2,
    batch: { items: (p) => [p.fingerprint], restrict: (p) => p },
    dedupe: pcmDedupe,
    rememberPoison: true,
    crashRestarts: true,
    memory: BATCH_MEMORY,
    files: (p) => [...(p.asset == null ? [] : [{ name: "in/manifest.json", mode: "read" as const }]), { name: "out/", mode: "write" as const }],
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.file != null) {
          const reads = yield* Effect.promise(() => canonical([p.file!.root]));
          return { extra: { reads } };
        }
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required for an asset"));
        if (p.asset == null) return yield* Effect.fail(invalid("asset", "required"));
        const dirs = jobDirs(root, ctx.job.id);
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const done = yield* deps.projects.onBranch<{ rev: number; fingerprint: string; sources: ReadonlyArray<string> }>(p.projectId, p.branch ?? "main", (engine, branch) =>
          engine.prepareAssetJob(branch, p.asset!, join(dirs.job, "in")),
        );
        if (done.fingerprint !== p.fingerprint) {
          return yield* Effect.fail(finError("ASSET_CHANGED", `asset ${p.asset} changed since its PCM feed was asked for`, { asset: p.asset, reason: "fingerprint" }, "ask for the feed again"));
        }
        const reads = yield* Effect.promise(() => canonical(done.sources));
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
