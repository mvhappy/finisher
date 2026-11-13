// Proxies of project assets (6B-1 plan Task 14; sub-project 6 spec §7.1): the `media.proxy`
// and `media.proxy.status` RPCs, `PROXY_UNAVAILABLE` for a consumer that needs a proxy being
// made (Tasks 15, 16 and 6F), and the background `1080` proxy of an asset's first use in a
// project here.
// - **The key** (Decision 11) is planned before anything is submitted: the source's video codec
//   (fin-node's `probe` of the asset's file, kept per path and fingerprint), the capability
//   report's decoders for it and `mediaProxyPlan` (fin-media's `plan_key`). The worker computes
//   it again from the decoder it opens.
// - **The codec** (Decision 5): `codec` when asked for, else H.264 where the report names a
//   hardware H.264 encoder, else AV1. An H.264 proxy on a machine without one fails in the
//   worker, typed: `CODEC_UNAVAILABLE` with `open_encoder`'s guidance.
// - **A cache hit** answers at once (`serve` records that the entry serves the project and pins
//   it while the project is open); otherwise a job is submitted (dedupe: the key) and, unless
//   `wait: false`, waited for. An explicit request runs at `normal` priority; first-use proxies at
//   the kind's `background`.
import { isAbsolute, join } from "node:path";
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { EntryMeta, ProxyParams, ProxyResult, ProxyStatus, ProxyTierStatus } from "@finisher/core-types";
import { type MediaCache, mediaProxyPlan, probe as probeFile } from "@finisher/fin-node";
import { Context, Effect } from "effect";
import { attempt, finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_PROXY, decodersOf, proxyCodec } from "../jobs/kinds/media.proxy.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import type { CapabilitiesCache } from "./caps.ts";

/** How long `media.proxy` waits for its job. */
export const PROXY_WAIT_MS = 10 * 60_000;
/** The owner of the proxies the server makes on its own (first use). */
export const MEDIA_SYSTEM_OWNER: JobOwner = { kind: "system", id: "media" };

export type ProxyTier = "540" | "1080";
export type ProxyCodec = "h264" | "av1";
const TIERS: ReadonlyArray<ProxyTier> = ["540", "1080"];

export interface ProxyRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
  readonly tier?: ProxyTier | undefined;
  readonly codec?: ProxyCodec | undefined;
  /** Default true: wait up to `PROXY_WAIT_MS` for the job. */
  readonly wait?: boolean | undefined;
}

/** What `media.proxy` answers. */
export interface ProxyAnswer {
  readonly asset: string;
  readonly tier: ProxyTier;
  readonly codec: ProxyCodec;
  readonly key: string;
  /** `ready`: in the cache (`cached`: it was before this call); `making`: `job` makes it. */
  readonly state: "ready" | "making";
  readonly cached?: boolean;
  readonly job?: string;
  readonly result?: ProxyResult;
}

export interface ProxyServiceDeps {
  readonly registry: Pick<ProjectRegistryShape, "ensure" | "snapshot" | "resolveBranch" | "dirOf">;
  readonly engine: Pick<JobEngine, "submit" | "wait" | "get">;
  /** The cached `media.capabilities` answer (`media/caps.ts`). */
  readonly capabilities: (owner: JobOwner) => Effect.Effect<unknown, FinError>;
  readonly cache: () => MediaCache | undefined;
  /** fin-node's `probe` (JSON); tests replace it. */
  readonly probe?: (path: string) => Promise<string>;
  readonly log?: (message: string) => void;
}

export interface ProxyService {
  readonly request: (owner: JobOwner, req: ProxyRequest) => Effect.Effect<ProxyAnswer, FinError>;
  readonly status: (owner: JobOwner, req: Pick<ProxyRequest, "projectId" | "asset" | "branch">) => Effect.Effect<ProxyStatus, FinError>;
  /**
   * The entry of `asset`'s proxy at `tier` (served for the project and pinned while it is open),
   * or `PROXY_UNAVAILABLE {asset, tier, job}` with the job now making it (one is submitted when
   * none runs). For consumers that read proxies (Tasks 15, 16; 6F).
   */
  readonly ensure: (owner: JobOwner, req: Pick<ProxyRequest, "projectId" | "asset" | "branch" | "tier">) => Effect.Effect<EntryMeta, FinError>;
  /** A project opened or committed here: a background `1080` proxy of each video asset not asked for yet this run. */
  readonly used: (projectId: string) => void;
}

interface AssetJson {
  readonly kind?: string;
  readonly path?: string;
  readonly fingerprint?: string;
  readonly video?: { readonly width?: number; readonly height?: number; readonly display?: { readonly width?: number; readonly height?: number } } | null;
}

/** One asset as the planner needs it. */
interface Source {
  readonly projectId: string;
  readonly branch: string;
  readonly asset: string;
  readonly fingerprint: string;
  readonly sourceCodec: string;
  readonly display?: { readonly width: number; readonly height: number };
}

interface Planned {
  readonly params: ProxyParams;
  readonly codec: ProxyCodec;
  readonly key: string;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** `PROXY_UNAVAILABLE`: the proxy is being made (exit 5). */
export function proxyUnavailable(asset: string, tier: ProxyTier, job: string): FinError {
  return finError("PROXY_UNAVAILABLE", `the ${tier} proxy of asset ${asset} is being made`, { asset, tier, job }, "wait for the job, then ask again (fin job wait <job>)");
}

function errorOf(job: JobView): FinError {
  const e = job.error;
  return e === undefined || e === null ? finError("JOB_FAILED", `the proxy job ended ${job.state}`, { job: job.id, state: job.state }) : finError(e.code, e.message, e.details, e.fix);
}

/** The item's `ProxyResult` of a succeeded job. */
function resultOf(job: JobView, fingerprint: string): ProxyResult | undefined {
  const items = isObj(job.result) && isObj(job.result.items) ? job.result.items : {};
  const r = items[fingerprint];
  return isObj(r) ? (r as unknown as ProxyResult) : undefined;
}

const MAKING = new Set(["queued", "running", "paused"]);

/** The server's capability cache and proxy service, for the handlers (absent in handler tests built without them). */
export class MediaProxyTag extends Context.Service<MediaProxyTag, { readonly capabilities: CapabilitiesCache; readonly proxies: ProxyService }>()("finisherd/MediaProxy") {}

export function makeProxyService(deps: ProxyServiceDeps): ProxyService {
  const probe = deps.probe ?? probeFile;
  const log = deps.log ?? (() => {});
  /** The source codec of a file, by path and fingerprint (a new fingerprint probes again). */
  const codecs = new Map<string, string>();
  /** The last job of each key this run (status, ensure). */
  const jobs = new Map<string, string>();
  /** `<project>\0<fingerprint>` already asked for at first use this run. */
  const used = new Set<string>();

  const sourceCodec = (path: string, fingerprint: string): Effect.Effect<string, FinError> => {
    const id = `${path}\0${fingerprint}`;
    const known = codecs.get(id);
    if (known !== undefined) return Effect.succeed(known);
    return Effect.flatMap(
      attempt(() => probe(path)),
      (json) => {
        const codec = (JSON.parse(json) as { media?: { video?: { codec?: unknown } } }).media?.video?.codec;
        if (typeof codec !== "string") return Effect.fail(finError("ASSET_TYPE_UNSUPPORTED", "the asset's file has no video track", { reason: "noVideo" }, "make proxies of video assets"));
        codecs.set(id, codec);
        return Effect.succeed(codec);
      },
    );
  };

  const source = (projectId: string, asset: string, requested: string | undefined): Effect.Effect<Source, FinError> =>
    Effect.gen(function* () {
      yield* deps.registry.ensure(projectId);
      const branch = yield* deps.registry.resolveBranch(projectId, requested ?? "main");
      const snap = yield* deps.registry.snapshot(projectId, branch);
      const assets = ((snap.project as { assets?: Record<string, AssetJson> } | null)?.assets ?? {}) as Record<string, AssetJson>;
      const a = Object.hasOwn(assets, asset) ? assets[asset] : undefined;
      if (a === undefined) return yield* Effect.fail(finError("ASSET_NOT_FOUND", `no asset ${asset} on ${branch}`, { asset, branch }, "use the id of an existing asset"));
      if (a.kind !== "media" || typeof a.fingerprint !== "string" || typeof a.path !== "string" || !isObj(a.video)) {
        return yield* Effect.fail(finError("ASSET_TYPE_UNSUPPORTED", `asset ${asset} is not a video asset`, { asset, kind: a.kind ?? null }, "make proxies of video assets"));
      }
      const dir = deps.registry.dirOf(projectId);
      const path = isAbsolute(a.path) || dir === undefined ? a.path : join(dir, a.path);
      const codec = yield* sourceCodec(path, a.fingerprint);
      const d = a.video.display ?? a.video;
      const display = typeof d.width === "number" && typeof d.height === "number" ? { width: d.width, height: d.height } : undefined;
      return { projectId, branch, asset, fingerprint: a.fingerprint, sourceCodec: codec, ...(display === undefined ? {} : { display }) };
    });

  const plan = (report: unknown, src: Source, tier: ProxyTier, asked: ProxyCodec | undefined): Effect.Effect<Planned, FinError> =>
    Effect.map(
      attempt(async () => {
        const codec = asked ?? proxyCodec(report);
        const p = mediaProxyPlan({ fingerprint: src.fingerprint, tier, codec, sourceCodec: src.sourceCodec, decoders: decodersOf(report, src.sourceCodec) });
        return { codec, p };
      }),
      ({ codec, p }): Planned => ({
        codec,
        key: p.key,
        params: {
          fingerprint: src.fingerprint,
          tier,
          codec,
          decoderClass: p.decoderClass,
          key: p.key,
          asset: src.asset,
          projectId: src.projectId,
          branch: src.branch,
          ...(src.display === undefined ? {} : { display: src.display }),
        },
      }),
    );

  /** The entry of `key`, served for `projectId` (recorded and pinned while it is open). */
  const hit = (key: string, projectId: string): Effect.Effect<EntryMeta | undefined, FinError> =>
    Effect.gen(function* () {
      const cache = deps.cache();
      if (cache === undefined) return undefined;
      const meta = yield* attempt(() => cache.lookup(key));
      if (meta === null) return undefined;
      yield* attempt(() => cache.serve(key, [projectId]));
      return meta;
    });

  const submit = (owner: JobOwner, planned: Planned, priority?: "normal"): Effect.Effect<JobView, FinError> =>
    Effect.tap(deps.engine.submit(owner, { kind: MEDIA_PROXY, params: planned.params, project: planned.params.projectId!, ...(priority === undefined ? {} : { priority }) }), (job) =>
      Effect.sync(() => jobs.set(planned.key, job.id)),
    );

  const request = (owner: JobOwner, req: ProxyRequest): Effect.Effect<ProxyAnswer, FinError> =>
    Effect.gen(function* () {
      const src = yield* source(req.projectId, req.asset, req.branch);
      const report = yield* deps.capabilities(owner);
      const tier = req.tier ?? "540";
      const planned = yield* plan(report, src, tier, req.codec);
      const base = { asset: req.asset, tier, codec: planned.codec, key: planned.key };
      if ((yield* hit(planned.key, req.projectId)) !== undefined) return { ...base, state: "ready", cached: true };
      const job = yield* submit(owner, planned, "normal");
      if (req.wait === false) return { ...base, state: "making", job: job.id };
      const [done] = yield* deps.engine.wait([job.id], { timeoutMs: PROXY_WAIT_MS }).pipe(
        Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the proxy is still being made: fin media status <asset>") : e)),
      );
      const view = done as JobView;
      if (view.state !== "succeeded") return yield* Effect.fail(errorOf(view));
      const result = resultOf(view, src.fingerprint);
      return { ...base, state: "ready", job: job.id, ...(result === undefined ? {} : { result }) };
    });

  /** One tier's state: an entry in either codec, else the job of the preferred codec. */
  const tierStatus = (report: unknown, src: Source, tier: ProxyTier): Effect.Effect<ProxyTierStatus, FinError> =>
    Effect.gen(function* () {
      const preferred = proxyCodec(report);
      const order: ProxyCodec[] = preferred === "h264" ? ["h264", "av1"] : ["av1", "h264"];
      const planned: Planned[] = [];
      for (const codec of order) planned.push(yield* plan(report, src, tier, codec));
      for (const p of planned) {
        const cache = deps.cache();
        const meta = cache === undefined ? null : yield* attempt(() => cache.lookup(p.key));
        if (meta !== null) return { tier, state: "ready", key: p.key, codec: p.codec };
      }
      for (const p of planned) {
        const id = jobs.get(p.key);
        if (id === undefined) continue;
        const view = yield* Effect.result(deps.engine.get(id));
        if (view._tag === "Failure") continue;
        const j = view.success;
        if (MAKING.has(j.state)) return { tier, state: "making", key: p.key, codec: p.codec, job: id };
        if (j.state === "failed") return { tier, state: "failed", key: p.key, codec: p.codec, job: id, code: j.error?.code ?? "JOB_FAILED" };
      }
      return { tier, state: "none", key: planned[0]!.key, codec: planned[0]!.codec };
    });

  const status = (owner: JobOwner, req: Pick<ProxyRequest, "projectId" | "asset" | "branch">): Effect.Effect<ProxyStatus, FinError> =>
    Effect.gen(function* () {
      const src = yield* source(req.projectId, req.asset, req.branch);
      const report = yield* deps.capabilities(owner);
      const tiers: ProxyTierStatus[] = [];
      for (const tier of TIERS) tiers.push(yield* tierStatus(report, src, tier));
      return { asset: req.asset, fingerprint: src.fingerprint, tiers };
    });

  const ensure = (owner: JobOwner, req: Pick<ProxyRequest, "projectId" | "asset" | "branch" | "tier">): Effect.Effect<EntryMeta, FinError> =>
    Effect.gen(function* () {
      const tier = req.tier ?? "1080";
      const src = yield* source(req.projectId, req.asset, req.branch);
      const report = yield* deps.capabilities(owner);
      const planned = yield* plan(report, src, tier, undefined);
      const meta = yield* hit(planned.key, req.projectId);
      if (meta !== undefined) return meta;
      // The job making it (dedupe answers the running one).
      const job = yield* submit(owner, planned);
      return yield* Effect.fail(proxyUnavailable(req.asset, tier, job.id));
    });

  const usedHere = (projectId: string): void => {
    void Effect.runPromise(
      Effect.gen(function* () {
        const snap = yield* deps.registry.snapshot(projectId, "main");
        const assets = ((snap.project as { assets?: Record<string, AssetJson> } | null)?.assets ?? {}) as Record<string, AssetJson>;
        for (const [id, a] of Object.entries(assets)) {
          if (a.kind !== "media" || !isObj(a.video) || typeof a.fingerprint !== "string") continue;
          const seen = `${projectId}\0${a.fingerprint}`;
          if (used.has(seen)) continue;
          used.add(seen);
          const asked = yield* Effect.result(
            Effect.gen(function* () {
              const src = yield* source(projectId, id, "main");
              const report = yield* deps.capabilities(MEDIA_SYSTEM_OWNER);
              const planned = yield* plan(report, src, "1080", undefined);
              if ((yield* hit(planned.key, projectId)) !== undefined) return;
              yield* submit(MEDIA_SYSTEM_OWNER, planned);
            }),
          );
          if (asked._tag === "Failure") log(`finisherd: media proxy: no 1080 proxy of ${id} in ${projectId} (${asked.failure.code})`);
        }
      }),
    ).catch(() => undefined);
  };

  return { request, status, ensure, used: usedHere };
}
