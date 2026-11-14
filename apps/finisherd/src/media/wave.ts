// Waveform peaks of audio (6C plan Task 19, Decisions 7, 19 and 20; sub-project 6 spec §7.2): the
// `media.waveform` RPC and the background waveform that follows an asset's first-use `pcm48`.
// - **The key** follows the asset's `pcm48` plan (`PcmService.planFeed`): the same fingerprint and
//   decoder class (the waveform is of the same decode), `mediaWavePlan`.
// - **A cache hit** answers at once; otherwise a job is submitted (dedupe: the key) and, unless
//   `wait: false`, waited for. When the cache holds the asset's `pcm48` entry the job reads it
//   (`pcmKey`) instead of decoding the original; the bytes are the same.
// - **After the commit** of a first-use `pcm48` (`PcmServiceDeps.afterPcm48`), the waveform is
//   submitted in the background, from that entry, once the entry is in the cache.
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { WaveParams, WaveResult } from "@finisher/core-types";
import { type MediaCache, mediaWavePlan } from "@finisher/fin-node";
import { Effect } from "effect";
import { attempt, finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_WAVEFORM } from "../jobs/kinds/media.waveform.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { PCM_WAIT_MS, type PcmService, endedWithout, errorOf } from "./pcm.ts";
import { MEDIA_SYSTEM_OWNER } from "./proxy.ts";

/** How long `media.waveform` waits for its job. */
export const WAVE_WAIT_MS = PCM_WAIT_MS;

export interface WaveRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
  /** Default true: wait up to `WAVE_WAIT_MS` for the job. */
  readonly wait?: boolean | undefined;
}

/** What `media.waveform` answers. */
export interface WaveAnswer {
  readonly asset: string;
  readonly key: string;
  /** Samples per peak of each level of `peaks.bin`, finest first. */
  readonly levels: ReadonlyArray<number>;
  readonly state: "ready" | "making";
  readonly cached?: boolean;
  readonly job?: string;
  readonly result?: WaveResult;
}

export interface WaveServiceDeps {
  readonly registry: Pick<ProjectRegistryShape, "ensure" | "resolveBranch">;
  readonly pcm: Pick<PcmService, "planFeed">;
  readonly engine: Pick<JobEngine, "submit" | "wait">;
  readonly cache: () => MediaCache | undefined;
  readonly log?: (message: string) => void;
  /** How long one wait for the `pcm48` job lasts before it is waited for again (default `WAVE_WAIT_MS`). */
  readonly waitMs?: number;
}

export interface WaveService {
  readonly request: (owner: JobOwner, req: WaveRequest) => Effect.Effect<WaveAnswer, FinError>;
  /** The `pcm48` of `p` is in the cache, or is being made as `job` (`PcmServiceDeps.afterPcm48`): make its waveform in the background. */
  readonly afterPcm48: (p: { readonly fingerprint: string; readonly decoderClass: WaveParams["decoderClass"]; readonly key: string; readonly asset?: string | null; readonly projectId?: string | null; readonly branch?: string | null }, job?: string) => void;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function resultOf(job: JobView, fingerprint: string): WaveResult | undefined {
  const items = isObj(job.result) && isObj(job.result.items) ? job.result.items : {};
  const r = items[fingerprint];
  return isObj(r) ? (r as unknown as WaveResult) : undefined;
}

export function makeWaveService(deps: WaveServiceDeps): WaveService {
  const log = deps.log ?? (() => {});

  /** `media.waveform` params for the asset whose `pcm48` plan is `pcm`, reading its entry when the cache holds it. */
  const paramsOf = (pcm: { fingerprint: string; decoderClass: WaveParams["decoderClass"]; key: string }, asset: string, projectId: string, branch: string): Effect.Effect<{ readonly p: WaveParams; readonly levels: number[] }, FinError> =>
    Effect.gen(function* () {
      const w = yield* attempt(async () => mediaWavePlan({ fingerprint: pcm.fingerprint, decoderClass: pcm.decoderClass }));
      const cache = deps.cache();
      const held = cache === undefined ? null : yield* attempt(() => cache.lookup(pcm.key));
      const p: WaveParams = { fingerprint: pcm.fingerprint, decoderClass: pcm.decoderClass, key: w.key, asset, ...(held === null ? {} : { pcmKey: pcm.key }), projectId, branch };
      return { p, levels: w.levels };
    });

  /** The entry of `key`, served for `projectId` (recorded and pinned while it is open). */
  const hit = (key: string, projectId: string): Effect.Effect<boolean, FinError> =>
    Effect.gen(function* () {
      const cache = deps.cache();
      if (cache === undefined) return false;
      if ((yield* attempt(() => cache.lookup(key))) === null) return false;
      yield* attempt(() => cache.serve(key, [projectId]));
      return true;
    });

  /** Submits `p` (dedupe: its key); a job that already ended (a poisoned one) fails with its error. */
  const submit = (owner: JobOwner, p: WaveParams, priority?: "normal"): Effect.Effect<JobView, FinError> =>
    Effect.flatMap(deps.engine.submit(owner, { kind: MEDIA_WAVEFORM, params: p, project: p.projectId!, ...(priority === undefined ? {} : { priority }) }), (job) =>
      endedWithout(job) ? Effect.fail(errorOf(job)) : Effect.succeed(job),
    );

  const request = (owner: JobOwner, req: WaveRequest): Effect.Effect<WaveAnswer, FinError> =>
    Effect.gen(function* () {
      yield* deps.registry.ensure(req.projectId);
      const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
      const pcm = yield* deps.pcm.planFeed(req.projectId, branch, req.asset);
      const { p, levels } = yield* paramsOf(pcm, req.asset, req.projectId, branch);
      const base = { asset: req.asset, key: p.key, levels };
      if (yield* hit(p.key, req.projectId)) return { ...base, state: "ready", cached: true };
      const job = yield* submit(owner, p, "normal");
      if (req.wait === false) return { ...base, state: "making", job: job.id };
      const [done] = yield* deps.engine.wait([job.id], { timeoutMs: WAVE_WAIT_MS }).pipe(
        Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the waveform is still being made: fin job wait <job>") : e)),
      );
      const view = done as JobView;
      if (view.state !== "succeeded") return yield* Effect.fail(errorOf(view));
      const result = resultOf(view, p.fingerprint);
      return { ...base, state: "ready", job: job.id, ...(result === undefined ? {} : { result }) };
    });

  const afterPcm48: WaveService["afterPcm48"] = (pcm, job) => {
    const { asset, projectId } = pcm;
    if (asset == null || projectId == null || deps.cache() === undefined) return;
    void Effect.runPromise(
      Effect.gen(function* () {
        if (job !== undefined) {
          // pcm48 runs at background priority and may outlast any one wait: wait again while it is
          // still queued or running (a wait that ends otherwise says how the job ended).
          for (;;) {
            const waited = yield* Effect.result(deps.engine.wait([job], { timeoutMs: deps.waitMs ?? WAVE_WAIT_MS }));
            if (waited._tag === "Success") {
              if ((waited.success[0] as JobView).state !== "succeeded") return;
              break;
            }
            if (waited.failure.code !== "WAIT_TIMEOUT") return yield* Effect.fail(waited.failure);
          }
        }
        const { p } = yield* paramsOf(pcm, asset, projectId, pcm.branch ?? "main");
        // Only from an entry that is there (the commit follows the job's success); never twice.
        if (p.pcmKey === undefined || (yield* hit(p.key, projectId))) return;
        yield* submit(MEDIA_SYSTEM_OWNER, p);
      }),
    ).catch((e: unknown) => log(`finisherd: media waveform: none for ${asset} in ${projectId} (${(e as { code?: string }).code ?? "error"})`));
  };

  return { request, afterPcm48 };
}
