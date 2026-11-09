// The `media.probeDeep` kind (6B-1 plan Task 11; sub-project 6 spec §3.3, §6.3): the
// decode-level probe of one asset by `fin-workers media`, cold. The worker decodes the first
// frame with the decoder its probe picks and reads in-band HDR SEI when the container has none;
// the answer is a `ProbeDeepResult` and nothing is written to the project.
// - `prepare` writes `in/manifest.json` naming only the asset (`Engine.prepareAssetJob`) and
//   fails `ASSET_CHANGED` when the asset's fingerprint at the branch head is no longer the one
//   the job was submitted for; the original is granted to the worker per file (7C Task 7).
// - Identity (`dedupe`): the project, the asset, its fingerprint, the worker's `RENDER_VERSION`
//   and a digest of its capability report, so one asset on the same decoders is probed once at a
//   time and a new decoder set probes again. Without the host fields the job does not coalesce.
// - Memory: the batch media rule of `media/memory.ts` (Decision 18) by the source's size.
import { realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { ProbeDeepParams, ProbeDeepResult } from "@finisher/core-types";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { mediaBatchMemory } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec } from "../kinds.ts";

export const MEDIA_PROBE_DEEP = "media.probeDeep";

export interface ProbeDeepKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

/** What `Engine.prepareAssetJob` resolves to. */
interface AssetJobPrepared {
  readonly rev: number;
  readonly fingerprint: string;
  readonly sources: ReadonlyArray<string>;
}

const invalid = (field: string, why: string): FinError =>
  finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "submit a media.probeDeep through the media.probeDeep RPC");

/**
 * `[projectId, asset, fingerprint, renderVersion, capsDigest]`, or `undefined` (no coalescing)
 * without the host's fields. The project and asset are in the key because the result names its
 * asset and `prepare` reads that project's manifest: one file under two assets (or projects)
 * shares a fingerprint but not an answer.
 */
export function probeDeepDedupe(p: ProbeDeepParams): string | undefined {
  if (p.projectId == null || p.renderVersion == null || p.capsDigest == null) return undefined;
  return JSON.stringify([MEDIA_PROBE_DEEP, p.projectId, p.asset, p.fingerprint, p.renderVersion, p.capsDigest]);
}

/** The original's path, resolved (a missing one is left out: the worker reports it missing). */
async function canonical(paths: ReadonlyArray<string>): Promise<string[]> {
  const out: string[] = [];
  for (const p of paths) {
    if (!isAbsolute(p)) continue;
    try {
      out.push(await realpath(p));
    } catch {
      // Missing or unreadable: not granted.
    }
  }
  return out.sort();
}

function readsOf(extra: unknown): ReadonlyArray<string> {
  const reads = (extra as { reads?: unknown } | null | undefined)?.reads;
  return Array.isArray(reads) ? reads.filter((r): r is string => typeof r === "string") : [];
}

/** `media.probeDeep` v1. */
export function mediaProbeDeepKind(deps: ProbeDeepKindDeps): KindSpec<ProbeDeepParams, ProbeDeepResult> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_PROBE_DEEP,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    requiresProject: true,
    resources: ["cpu"],
    warm: false,
    defaultPriority: "normal",
    qos: "normal",
    timeoutS: 120,
    stallS: 60,
    maxAttempts: 2,
    files: [{ name: "in/manifest.json", mode: "read" }],
    memory: (p) => mediaBatchMemory(p.display ?? undefined),
    dedupe: probeDeepDedupe,
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required"));
        const dirs = jobDirs(root, ctx.job.id);
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const done = yield* deps.projects.onBranch<AssetJobPrepared>(p.projectId, p.branch ?? "main", (engine, branch) =>
          engine.prepareAssetJob(branch, p.asset, join(dirs.job, "in")),
        );
        if (done.fingerprint !== p.fingerprint) {
          return yield* Effect.fail(
            finError(
              "ASSET_CHANGED",
              `asset ${p.asset} changed since the probe was asked for`,
              { asset: p.asset, reason: "fingerprint" },
              "probe the asset again",
            ),
          );
        }
        const reads = yield* Effect.promise(() => canonical(done.sources));
        return { extra: { rev: done.rev, reads } };
      }),
    reads: readsOf,
  };
}
