// Thumbnails and filmstrips of project assets (6B-1 plan Task 15, Decision 17; sub-project 6 spec
// §3.2 item 6): the `media.thumbs` RPC.
// - **The cache first** (spec §7.2: a cached answer needs no original). The asset's content comes
//   from the branch's snapshot (nothing read), then its current `1080` proxy entry (`currentProxy`,
//   the one the job's manifest will list). With one, the key (Decision 11) is planned from what the
//   entry's `meta.json` records of the original (its decoder class and duration): the key the worker
//   computes from the same entry. Without one, the original is probed (the proxy service's
//   `source`, its path kept out of any error) and the key planned with the capability report's
//   decoders for its codec. Either way `mediaThumbsPlan` (fin-media's `plan_thumbs`).
// - **A cache hit** answers at once (`serve` records that the entry serves the project and pins it
//   while the project is open); otherwise a job is submitted (dedupe: the key) at `normal` priority
//   and, unless `wait: false`, waited for. A job that already ended (the poisoned job dedupe
//   answers) is answered with its error. The job's `prepare` lists the proxy in its manifest and
//   pins it for the job's life (`attachProxies`, Decision 12).
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { EntryMeta, ThumbsParams, ThumbsPlan, VideoCodec } from "@finisher/core-types";
import { type MediaCache, mediaThumbsPlan } from "@finisher/fin-node";
import { Effect } from "effect";
import { attempt, finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_THUMBS } from "../jobs/kinds/media.thumbs.ts";
import { decodersOf } from "../jobs/kinds/media.proxy.ts";
import type { CapabilitiesCache } from "./caps.ts";
import { PROXY_WAIT_MS, type ProxyService } from "./proxy.ts";

/** The most thumbnails one request asks for (fin-media's `MAX_THUMBS`). */
export const MAX_THUMBS = 64;
/** The defaults: a thumbnail's longest side, a tile's height. */
export const DEFAULT_MAX_SIDE = 256;
export const DEFAULT_TILE_HEIGHT = 90;

export interface ThumbsRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
  /** Source times (rational seconds): one thumbnail each. */
  readonly at?: ReadonlyArray<string> | undefined;
  /** The filmstrip. */
  readonly strip?: boolean | undefined;
  readonly maxSide?: number | undefined;
  readonly tileHeight?: number | undefined;
  /** Default true: wait up to `PROXY_WAIT_MS` for the job. */
  readonly wait?: boolean | undefined;
}

/** What `media.thumbs` answers. */
export interface ThumbsAnswer {
  readonly asset: string;
  readonly mode: "thumbs" | "strip";
  readonly key: string;
  /** `ready`: in the cache (`cached`: it was before this call); `making`: `job` makes it. */
  readonly state: "ready" | "making";
  readonly cached?: boolean;
  readonly job?: string;
  /** The entry's files, when it is ready (`media.url` makes a URL of one). */
  readonly files?: ReadonlyArray<{ readonly name: string; readonly bytes: number }>;
}

export interface ThumbsServiceDeps {
  readonly engine: Pick<JobEngine, "submit" | "wait" | "get">;
  readonly capabilities: (owner: JobOwner) => Effect.Effect<unknown, FinError>;
  readonly proxies: Pick<ProxyService, "source" | "asset">;
  readonly cache: () => MediaCache | undefined;
}

export interface ThumbsService {
  readonly request: (owner: JobOwner, req: ThumbsRequest) => Effect.Effect<ThumbsAnswer, FinError>;
}

const MAKING = new Set(["queued", "running", "paused"]);

const bad = (field: string, why: string): FinError => finError("INVALID_REQUEST", `media.thumbs: ${field}: ${why}`, { field }, "fin media thumbs <asset> --at <t>[,<t>…] | --strip");

function errorOf(job: JobView): FinError {
  const e = job.error;
  return e === undefined || e === null ? finError("JOB_FAILED", `the thumbnails job ended ${job.state}`, { job: job.id, state: job.state }) : finError(e.code, e.message, e.details, e.fix);
}

export function makeThumbsService(deps: ThumbsServiceDeps): ThumbsService {
  const filesOf = (meta: EntryMeta): ReadonlyArray<{ readonly name: string; readonly bytes: number }> => meta.files.map((f) => ({ name: f.name, bytes: f.bytes }));

  const request = (owner: JobOwner, req: ThumbsRequest): Effect.Effect<ThumbsAnswer, FinError> =>
    Effect.gen(function* () {
      const strip = req.strip === true;
      if (strip === (req.at !== undefined)) return yield* Effect.fail(bad("at", "give `at` (times) or `strip`, not both and not neither"));
      if (strip && req.maxSide !== undefined) return yield* Effect.fail(bad("maxSide", "goes with `at`"));
      if (!strip && req.tileHeight !== undefined) return yield* Effect.fail(bad("tileHeight", "goes with `strip`"));
      if (req.at !== undefined && (req.at.length === 0 || req.at.length > MAX_THUMBS)) return yield* Effect.fail(bad("at", `1 to ${MAX_THUMBS} times`));
      const src = yield* deps.proxies.asset(req.projectId, req.asset, req.branch);
      const mode = strip ? { strip: { tileHeight: req.tileHeight ?? DEFAULT_TILE_HEIGHT } } : { thumbs: { times: [...(req.at ?? [])], maxSide: req.maxSide ?? DEFAULT_MAX_SIDE } };
      const cache = deps.cache();
      const proxy = cache === undefined ? null : yield* attempt(() => cache.currentProxy(src.fingerprint, [req.projectId]));
      const recorded = proxy?.media;
      const planned: ThumbsPlan =
        proxy == null || recorded?.video == null
          ? yield* Effect.gen(function* () {
              const original = yield* deps.proxies.source(req.projectId, req.asset, req.branch);
              const report = yield* deps.capabilities(owner);
              const { duration } = original;
              return yield* attempt(async () => mediaThumbsPlan({ fingerprint: src.fingerprint, mode, sourceCodec: original.sourceCodec as VideoCodec, decoders: decodersOf(report, original.sourceCodec), ...(duration === undefined ? {} : { duration }) }));
            })
          : yield* attempt(async () => mediaThumbsPlan({ fingerprint: src.fingerprint, mode, decoderClass: proxy.decoderClass, duration: recorded.duration }));
      const base = { asset: req.asset, mode: strip ? ("strip" as const) : ("thumbs" as const), key: planned.key };
      if (cache !== undefined) {
        const meta = yield* attempt(() => cache.lookup(planned.key));
        if (meta !== null) {
          yield* attempt(() => cache.serve(planned.key, [req.projectId]));
          return { ...base, state: "ready", cached: true, files: filesOf(meta) };
        }
      }
      const params: ThumbsParams = {
        fingerprint: src.fingerprint,
        asset: req.asset,
        mode: planned.mode,
        decoderClass: planned.decoderClass,
        key: planned.key,
        projectId: req.projectId,
        branch: src.branch,
        ...(src.display === undefined ? {} : { display: src.display }),
      };
      const job = yield* deps.engine.submit(owner, { kind: MEDIA_THUMBS, params, project: req.projectId, priority: "normal" });
      if (!MAKING.has(job.state) && job.state !== "succeeded") return yield* Effect.fail(errorOf(job));
      if (req.wait === false) return { ...base, state: "making", job: job.id };
      const [done] = yield* deps.engine.wait([job.id], { timeoutMs: PROXY_WAIT_MS }).pipe(
        Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the thumbnails are still being made: fin job wait <job>") : e)),
      );
      const view = done as JobView;
      if (view.state !== "succeeded") return yield* Effect.fail(errorOf(view));
      const meta = cache === undefined ? null : yield* attempt(() => cache.lookup(planned.key));
      return { ...base, state: "ready", job: job.id, ...(meta === null ? {} : { files: filesOf(meta) }) };
    });

  return { request };
}
