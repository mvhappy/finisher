// The `media.thumbs` kind (6B-1 plan Task 15, Decision 17; sub-project 6 spec §3.2 item 6):
// thumbnails at source times, or a filmstrip, of one project asset, by `fin-workers media`, cold,
// committed into the media cache as one entry.
// - A batch whose items are the thumbnails (`"0"`, `"1"`, …) or the filmstrip's sprites, so a
//   crash keeps what was done: the engine retries with the params restricted to the items it has
//   not acknowledged (`only`), in the same job directory, and the worker makes just those and then
//   the entry's `meta.json`.
// - Priority `background`, `qos: background`; resources `["gpu"]` (the scale pass); memory by the
//   source's display size (Decision 18).
// - Identity (`dedupe`): the cache key (Decision 11). `rememberPoison`: an item that crashes the
//   worker stays failed until the content or `RENDER_VERSION` changes, i.e. its key.
// - `prepare` writes the one-asset manifest (`ASSET_CHANGED` when the fingerprint moved) and grants
//   the worker the `1080` proxy entry's directory the service named, read only.
// - `onSucceeded`: the first item's error fails the job (so a waiting caller sees the cause and a
//   poisoned item fails it `POISON_ITEM`); otherwise `out/<key>/` is committed by
//   `MediaCache.commit` (rename, then insert) for the project that asked.
import { realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { ThumbsParams } from "@finisher/core-types";
import type { MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { mediaBatchMemory } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec } from "../kinds.ts";
import { outcomeOf } from "./media.proxy.ts";

export const MEDIA_THUMBS = "media.thumbs";

/** The longest a thumbnail job may take: a long filmstrip from a slow original, in software. */
export const THUMBS_TIMEOUT_S = 6 * 3600;

export interface ThumbsKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** The media cache (`undefined` when it did not open: the commit is then `CACHE_IO`). */
  readonly cache: () => MediaCache | undefined;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

/** The job's items: one per thumbnail, or per filmstrip sprite. */
export function thumbsItems(p: ThumbsParams): string[] {
  const n = "thumbs" in p.mode ? p.mode.thumbs.times.length : (p.mode.strip.sprites ?? 0);
  return Array.from({ length: n }, (_, i) => String(i));
}

/** `["media.thumbs", key]`: one job per cache entry (per owner, 5C-2 Decision 7). */
export function thumbsDedupe(p: ThumbsParams): string {
  return JSON.stringify([MEDIA_THUMBS, p.key]);
}

const invalid = (field: string, why: string): FinError =>
  finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "ask for thumbnails through media.thumbs or fin media thumbs");

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

/** `media.thumbs` v1. */
export function mediaThumbsKind(deps: ThumbsKindDeps): KindSpec<ThumbsParams, unknown> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_THUMBS,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    resources: () => ["gpu"],
    warm: false,
    defaultPriority: "background",
    qos: "background",
    timeoutS: THUMBS_TIMEOUT_S,
    stallS: 600,
    maxAttempts: 2,
    batch: { items: thumbsItems, restrict: (p, items) => ({ ...p, only: [...items] }) },
    dedupe: thumbsDedupe,
    rememberPoison: true,
    memory: (p) => mediaBatchMemory(p.display ?? undefined),
    files: () => [
      { name: "in/manifest.json", mode: "read" as const },
      { name: "out/", mode: "write" as const },
    ],
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
          return yield* Effect.fail(finError("ASSET_CHANGED", `asset ${p.asset} changed since the thumbnails were asked for`, { asset: p.asset, reason: "fingerprint" }, "ask again"));
        }
        const reads = yield* Effect.promise(() => canonical([...done.sources, ...(p.proxy == null ? [] : [p.proxy.dir])]));
        return { extra: { rev: done.rev, reads } };
      }),
    reads: readsOf,
    onSucceeded: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        for (const item of thumbsItems(p)) {
          const failed = outcomeOf(ctx.items, item);
          if (failed !== undefined) return yield* Effect.fail(failed);
        }
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
