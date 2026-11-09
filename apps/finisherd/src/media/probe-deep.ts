// `media.probeDeep` (6B-1 plan Task 11): the RPC that probes one asset at the decode level.
// It reads the branch head once (the asset's kind, fingerprint and recorded size), asks the
// cached capability report for the worker's `renderVersion` and a digest of the report (the
// job's identity with the fingerprint, `probeDeepDedupe`), submits a `media.probeDeep` job and
// answers its `ProbeDeepResult`. Nothing is committed: the model's `VideoInfo` is unchanged.
import { createHash } from "node:crypto";
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { ProbeDeepParams } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { MEDIA_PROBE_DEEP } from "../jobs/kinds/media.probeDeep.ts";
import type { MediaRpcDeps } from "./look.ts";

/** How long the RPC waits for the job. */
export const PROBE_WAIT_MS = 120_000;

export interface ProbeDeepRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
}

export interface ProbeDeepDeps extends MediaRpcDeps {
  /** The cached `media.capabilities` answer (`media/caps.ts`). */
  readonly capabilities: (owner: JobOwner) => Effect.Effect<unknown, FinError>;
}

interface AssetJson {
  readonly kind?: string;
  readonly fingerprint?: string;
  readonly video?: { readonly width?: number; readonly height?: number } | null;
}

/** SHA-256 of the report's JSON: equal reports, equal digests (the worker answers it the same way each run). */
export function capsDigest(report: unknown): string {
  return createHash("sha256").update(JSON.stringify(report)).digest("hex");
}

function renderVersionOf(report: unknown): number | undefined {
  const v = (report as { worker?: { renderVersion?: unknown } } | null | undefined)?.worker?.renderVersion;
  return typeof v === "number" && Number.isSafeInteger(v) ? v : undefined;
}

function resultOf(job: JobView): Effect.Effect<unknown, FinError> {
  if (job.state === "succeeded") return Effect.succeed(job.result);
  const e = job.error;
  return Effect.fail(
    e === undefined ? finError("JOB_FAILED", `the probe ended ${job.state}`, { job: job.id, state: job.state }) : finError(e.code, e.message, e.details, e.fix),
  );
}

/**
 * `media.probeDeep`. Errors: `PROJECT_NOT_FOUND`, `BRANCH_NOT_FOUND`, `ASSET_NOT_FOUND`,
 * `ASSET_TYPE_UNSUPPORTED` (not a `media` asset), `ASSET_CHANGED` (its fingerprint changed
 * before the job ran), `ORIGINALS_MISSING`, `MEDIA_CORRUPT`, `WAIT_TIMEOUT`, then the job's own.
 * A codec this machine cannot decode is not an error: the answer says `decodable: false` with
 * the decoder's `failure`.
 */
export function probeDeep(deps: ProbeDeepDeps, owner: JobOwner, req: ProbeDeepRequest): Effect.Effect<unknown, FinError> {
  return Effect.gen(function* () {
    yield* deps.registry.ensure(req.projectId);
    const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
    const snap = yield* deps.registry.snapshot(req.projectId, branch);
    const assets = ((snap.project as { assets?: Record<string, AssetJson> } | null)?.assets ?? {}) as Record<string, AssetJson>;
    const a = Object.hasOwn(assets, req.asset) ? assets[req.asset] : undefined;
    if (a === undefined) {
      return yield* Effect.fail(finError("ASSET_NOT_FOUND", `no asset ${req.asset} on ${branch}`, { asset: req.asset, branch }, "use the id of an existing asset"));
    }
    if (a.kind !== "media" || typeof a.fingerprint !== "string") {
      return yield* Effect.fail(finError("ASSET_TYPE_UNSUPPORTED", `asset ${req.asset} is not a media asset`, { asset: req.asset, kind: a.kind ?? null }, "probe a video asset"));
    }
    const report = yield* deps.capabilities(owner);
    const renderVersion = renderVersionOf(report);
    const { width, height } = a.video ?? {};
    const params: ProbeDeepParams = {
      asset: req.asset,
      fingerprint: a.fingerprint,
      projectId: req.projectId,
      branch,
      ...(typeof width === "number" && typeof height === "number" ? { display: { width, height } } : {}),
      ...(renderVersion === undefined ? {} : { renderVersion, capsDigest: capsDigest(report) }),
    };
    const job = yield* deps.engine.submit(owner, { kind: MEDIA_PROBE_DEEP, params, project: req.projectId });
    const [done] = yield* deps.engine.wait([job.id], { timeoutMs: PROBE_WAIT_MS }).pipe(
      Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the probe keeps running: ask for its result with its job id") : e)),
    );
    return yield* resultOf(done as JobView);
  });
}
