// The `media.listen` kind (6C plan Task 21, Decisions 20 and 21; sub-project 6 spec §8.7): loudness,
// peaks, clipping and silences of a range of a timeline, per track and for the mix, measured by
// `fin-workers media`, cold.
// - Priority `normal`, pool `cpu`, memory `batch`'s `min(4 GiB, 25 %)`. Identity (`dedupe`): the
//   project, branch, rev, timeline and range.
// - `prepare` makes the job's `in/` and asks the project's engine to write `snapshot.json`,
//   `manifest.json` and `params.json` there (`Engine.prepareListenJob`: the plan hash of every
//   10 s window of the range, handed to the worker as `extra.planSamples`), then lists the
//   `pcm48` entries the media cache holds for the manifest's assets (`MediaCache.attachPcm`),
//   pinned for the job's life (`job:<id>`, released when it ends: `media/wiring.ts`). A cache that
//   is down lists none: the worker decodes the originals.
// - The worker writes nothing; `onSucceeded` answers its `ListenResult`.
import { realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { ListenParams, ListenResult } from "@finisher/core-types";
import type { MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { BATCH_MEMORY } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec } from "../kinds.ts";
import { mediaJournal } from "./media.journal.ts";

export const MEDIA_LISTEN = "media.listen";

/** The longest a listen may take: a long timeline decoded from originals on a slow machine. */
export const LISTEN_TIMEOUT_S = 3600;

export interface ListenKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** The media cache (`undefined` when it did not open: nothing is listed). */
  readonly cache: () => MediaCache | undefined;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

/** `(project, branch, rev, timeline, from, to)`: equal listens of one head coalesce. */
export function listenDedupe(p: ListenParams): string | undefined {
  if (p.projectId == null || p.branch == null || p.rev == null) return undefined;
  return JSON.stringify([MEDIA_LISTEN, p.projectId, p.branch, p.rev, p.timeline, p.from, p.to]);
}

const invalid = (field: string, why: string): FinError => finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "submit a media.listen through fin listen");

async function canonical(paths: ReadonlyArray<string>): Promise<string[]> {
  const out = new Set<string>();
  for (const p of paths) {
    if (!isAbsolute(p)) continue;
    try {
      out.add(await realpath(p));
    } catch {
      // Missing or unreadable: not granted (the worker notes the source missing).
    }
  }
  return [...out].sort();
}

function readsOf(extra: unknown): ReadonlyArray<string> {
  const reads = (extra as { reads?: unknown } | null | undefined)?.reads;
  return Array.isArray(reads) ? reads.filter((r): r is string => typeof r === "string") : [];
}

/** `media.listen` v1. */
export function mediaListenKind(deps: ListenKindDeps): KindSpec<ListenParams, ListenResult> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_LISTEN,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    resources: ["cpu"],
    warm: false,
    defaultPriority: "normal",
    qos: "normal",
    timeoutS: LISTEN_TIMEOUT_S,
    stallS: 600,
    maxAttempts: 2,
    memory: BATCH_MEMORY,
    files: [
      { name: "in/snapshot.json", mode: "read" },
      { name: "in/manifest.json", mode: "read" },
    ],
    dedupe: listenDedupe,
    journal: mediaJournal,
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required"));
        if (p.branch == null) return yield* Effect.fail(invalid("branch", "required"));
        const projectId = p.projectId;
        const dirs = jobDirs(root, ctx.job.id);
        const inDir = join(dirs.job, "in");
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const { planSamples: _, ...request } = p;
        const done = yield* deps.projects.onBranch<{ rev: number; planSamples: ReadonlyArray<{ at: string; blake3: string }>; sources: ReadonlyArray<string> }>(projectId, p.branch, (engine, branch) =>
          engine.prepareListenJob(branch, p.timeline, String(p.from), String(p.to), inDir, JSON.stringify(request), p.rev ?? null),
        );
        const cache = deps.cache();
        const pcmFiles = cache === undefined ? [] : yield* Effect.tryPromise({ try: () => cache.attachPcm(inDir, `job:${ctx.job.id}`, [projectId]), catch: toFinError });
        const reads = yield* Effect.promise(() => canonical([...done.sources, ...pcmFiles]));
        return { extra: { planSamples: done.planSamples, rev: done.rev, reads } };
      }),
    reads: readsOf,
    onSucceeded: (ctx) => Effect.succeed(ctx.result),
  };
}
