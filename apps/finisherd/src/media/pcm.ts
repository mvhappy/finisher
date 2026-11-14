// PCM feeds of audio (6C plan Task 18, Decisions 7, 8 and 20; sub-project 6 spec §7.2): the
// `media.pcm` RPC, the background `pcm48` of an asset's first use on an audio track of an open
// project, and the `pcm16m` library stage. `projects.ts` and `main.ts` stay call-site only: the
// hooks are bound in `wiring.ts`.
// - **The key** (Decision 7) is planned before anything is submitted: the asset's channel count
//   (the snapshot's `audio.channels`), its audio codec (fin-node's `probe` of the file, kept per
//   path and fingerprint; none for WAV, read by symphonia) and `mediaPcmPlan`. The worker
//   computes it again from the decoder it opens.
// - **A cache hit** answers at once (`serve` records that the entry serves the project and pins it
//   while the project is open); otherwise a job is submitted (dedupe: the key) and, unless
//   `wait: false`, waited for. An explicit request runs at `normal` priority, first use at the
//   kind's `background`.
// - **First use** scans `main` only: the assets of the items on audio tracks when a project opens
//   here (or was open before the service started), the items a commit adds or moves otherwise. An
//   asset counts as asked for once its feed was submitted or found, or it failed for good; a
//   transient failure is tried again at the next open or commit.
// - **After the commit** (6C Task 19) the waveform of the asset is made in the background (`afterPcm48`,
//   `media/wave.ts`), from the entry just committed.
// - **The `pcm16m` stage** (after `probe`): every library media file with an audio track this
//   build decodes; its version is `<recipe version>.<RENDER_VERSION>.<AUDIO_VERSION>`, so either
//   version moving makes every feed again (their keys move too).
import { isAbsolute, join } from "node:path";
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import type { CommittedTx, EntryMeta, PcmParams, PcmResult } from "@finisher/core-types";
import { type MediaCache, mediaPcmPlan, mediaVersions, probe as probeFile } from "@finisher/fin-node";
import { Effect } from "effect";
import { attempt, finError } from "../errors.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_PCM } from "../jobs/kinds/media.pcm.ts";
import type { StageFile, StageSpec } from "../library/api.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { CONTENT_FAILURES, MEDIA_SYSTEM_OWNER, scrubPath } from "./proxy.ts";

/** How long `media.pcm` waits for its job. */
export const PCM_WAIT_MS = 10 * 60_000;
export const PCM16M_STAGE = "pcm16m";

export type PcmRecipe = "pcm48" | "pcm16m";

export interface PcmRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
  readonly recipe?: PcmRecipe | undefined;
  /** Default true: wait up to `PCM_WAIT_MS` for the job. */
  readonly wait?: boolean | undefined;
}

/** What `media.pcm` answers. */
export interface PcmAnswer {
  readonly asset: string;
  readonly recipe: PcmRecipe;
  readonly key: string;
  readonly state: "ready" | "making";
  readonly cached?: boolean;
  readonly job?: string;
  readonly result?: PcmResult;
}

export interface PcmServiceDeps {
  readonly registry: Pick<ProjectRegistryShape, "ensure" | "snapshot" | "resolveBranch" | "dirOf">;
  readonly engine: Pick<JobEngine, "submit" | "wait">;
  readonly cache: () => MediaCache | undefined;
  /** fin-node's `probe` (JSON); tests replace it. */
  readonly probe?: (path: string) => Promise<string>;
  readonly log?: (message: string) => void;
  /**
   * 6C Task 19: a `pcm48` feed of `p` is in the cache or was just submitted as `job` (first use);
   * called once it is there to read, which the waveform builds on (`media/wave.ts`). Never awaited.
   */
  readonly afterPcm48?: (p: PcmParams, job?: string) => void;
}

export interface PcmService {
  readonly request: (owner: JobOwner, req: PcmRequest) => Effect.Effect<PcmAnswer, FinError>;
  /**
   * A project opened (or, with `items`, a commit added or moved those items) here: a background
   * `pcm48` of each asset of `main` used on an audio track, not asked for yet this run.
   */
  readonly used: (projectId: string, items?: ReadonlyArray<string>) => Promise<void>;
  /** The `pcm48` plan of an asset (6C Task 19: the waveform's key and decoder class follow it). */
  readonly planFeed: (projectId: string, branch: string, asset: string) => Effect.Effect<PcmParams, FinError>;
}

interface AssetJson {
  readonly kind?: string;
  readonly path?: string;
  readonly fingerprint?: string;
  readonly duration?: string;
  readonly audio?: { readonly channels?: number } | null;
}

interface TrackJson {
  readonly kind?: string;
  readonly items?: ReadonlyArray<{ readonly id?: string; readonly asset?: unknown }>;
}

interface ProjectJson {
  readonly assets?: Record<string, AssetJson>;
  readonly timelines?: ReadonlyArray<{ readonly tracks?: ReadonlyArray<TrackJson> }>;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** The ids of the items a commit added, or moved to another track. */
export function itemsOf(committed: CommittedTx): string[] {
  return committed.diff.changes.filter((c) => c.kind === "item" && (c.change === "added" || (c.change === "updated" && c.track !== undefined))).map((c) => c.id);
}

/** The assets used by items on audio tracks of any timeline (`only`: those items alone), in first-use order. */
export function audioAssetsOf(project: ProjectJson, only?: ReadonlySet<string>): string[] {
  const out: string[] = [];
  for (const tl of project.timelines ?? []) {
    for (const tr of tl.tracks ?? []) {
      if (tr.kind !== "audio") continue;
      for (const it of tr.items ?? []) {
        if (typeof it.asset !== "string" || (only !== undefined && (it.id === undefined || !only.has(it.id)))) continue;
        if (!out.includes(it.asset)) out.push(it.asset);
      }
    }
  }
  return out;
}

export function errorOf(job: JobView): FinError {
  const e = job.error;
  return e === undefined || e === null ? finError("JOB_FAILED", `the PCM job ended ${job.state}`, { job: job.id, state: job.state }) : finError(e.code, e.message, e.details, e.fix);
}

const MAKING = new Set(["queued", "running", "paused"]);
export const endedWithout = (job: JobView): boolean => !MAKING.has(job.state) && job.state !== "succeeded";

/** The item's `PcmResult` of a succeeded job. */
function resultOf(job: JobView, fingerprint: string): PcmResult | undefined {
  const items = isObj(job.result) && isObj(job.result.items) ? job.result.items : {};
  const r = items[fingerprint];
  return isObj(r) ? (r as unknown as PcmResult) : undefined;
}

/** The probe's audio codec (`media.audio.codec`), when it names one. */
function codecOf(probeJson: unknown): string | undefined {
  const media = isObj(probeJson) ? probeJson.media : undefined;
  const audio = isObj(media) ? media.audio : undefined;
  return isObj(audio) && typeof audio.codec === "string" ? audio.codec : undefined;
}

export function makePcmService(deps: PcmServiceDeps): PcmService {
  const probe = deps.probe ?? probeFile;
  const log = deps.log ?? (() => {});
  /** The audio codec of a file, by path and fingerprint (`null`: the probe names none). */
  const codecs = new Map<string, string | null>();
  /** `<project>\0<fingerprint>` asked for at first use this run, or being asked for now. */
  const used = new Set<string>();
  const asking = new Set<string>();

  const assetsOf = (projectId: string, branch: string): Effect.Effect<ProjectJson, FinError> =>
    Effect.map(deps.registry.snapshot(projectId, branch), (snap) => ((snap.project as ProjectJson | null) ?? {}) as ProjectJson);

  const sourceCodec = (path: string, fingerprint: string, asset: string): Effect.Effect<string | undefined, FinError> => {
    const id = `${path}\0${fingerprint}`;
    const known = codecs.get(id);
    if (known !== undefined) return Effect.succeed(known ?? undefined);
    return Effect.map(attempt(() => probe(path)).pipe(Effect.mapError((e) => scrubPath(e, path, asset))), (json) => {
      const codec = codecOf(JSON.parse(json));
      codecs.set(id, codec ?? null);
      return codec;
    });
  };

  /** `media.pcm` params of asset `asset` (its JSON `a`) on `branch`. */
  const plan = (projectId: string, branch: string, asset: string, a: AssetJson | undefined, recipe: PcmRecipe): Effect.Effect<PcmParams, FinError> =>
    Effect.gen(function* () {
      if (a === undefined) return yield* Effect.fail(finError("ASSET_NOT_FOUND", `no asset ${asset} on ${branch}`, { asset, branch }, "use the id of an existing asset"));
      const channels = isObj(a.audio) && typeof a.audio.channels === "number" ? a.audio.channels : 0;
      if (a.kind !== "media" || typeof a.fingerprint !== "string" || typeof a.path !== "string" || channels < 1) {
        return yield* Effect.fail(finError("ASSET_TYPE_UNSUPPORTED", `asset ${asset} has no audio`, { asset, kind: a.kind ?? null }, "make PCM feeds of assets with an audio track"));
      }
      const dir = deps.registry.dirOf(projectId);
      const path = isAbsolute(a.path) || dir === undefined ? a.path : join(dir, a.path);
      const codec = yield* sourceCodec(path, a.fingerprint, asset);
      const fingerprint = a.fingerprint;
      const duration = typeof a.duration === "string" ? a.duration : undefined;
      const p = yield* attempt(async () => mediaPcmPlan({ fingerprint, recipe, sourceCodec: codec, channels, duration }));
      return { fingerprint, recipe, layout: p.layout, decoderClass: p.decoderClass, key: p.key, asset, projectId, branch };
    });

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

  /** Submits `p` (dedupe: its key); a job that already ended (a poisoned one) fails with its error. */
  const submit = (owner: JobOwner, p: PcmParams, priority?: "normal"): Effect.Effect<JobView, FinError> =>
    Effect.flatMap(deps.engine.submit(owner, { kind: MEDIA_PCM, params: p, project: p.projectId!, ...(priority === undefined ? {} : { priority }) }), (job) =>
      endedWithout(job) ? Effect.fail(errorOf(job)) : Effect.succeed(job),
    );

  const request = (owner: JobOwner, req: PcmRequest): Effect.Effect<PcmAnswer, FinError> =>
    Effect.gen(function* () {
      yield* deps.registry.ensure(req.projectId);
      const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
      const project = yield* assetsOf(req.projectId, branch);
      const assets = project.assets ?? {};
      const recipe = req.recipe ?? "pcm48";
      const p = yield* plan(req.projectId, branch, req.asset, Object.hasOwn(assets, req.asset) ? assets[req.asset] : undefined, recipe);
      const base = { asset: req.asset, recipe, key: p.key };
      if ((yield* hit(p.key, req.projectId)) !== undefined) return { ...base, state: "ready", cached: true };
      const job = yield* submit(owner, p, "normal");
      if (req.wait === false) return { ...base, state: "making", job: job.id };
      const [done] = yield* deps.engine.wait([job.id], { timeoutMs: PCM_WAIT_MS }).pipe(
        Effect.mapError((e) => (e.code === "WAIT_TIMEOUT" ? finError(e.code, e.message, { ...(e.details as object), job: job.id }, "the feed is still being made: fin job wait <job>") : e)),
      );
      const view = done as JobView;
      if (view.state !== "succeeded") return yield* Effect.fail(errorOf(view));
      const result = resultOf(view, p.fingerprint);
      return { ...base, state: "ready", job: job.id, ...(result === undefined ? {} : { result }) };
    });

  const usedHere = (projectId: string, items?: ReadonlyArray<string>): Promise<void> =>
    Effect.runPromise(
      Effect.gen(function* () {
        const project = yield* assetsOf(projectId, "main");
        const assets = project.assets ?? {};
        for (const id of audioAssetsOf(project, items === undefined ? undefined : new Set(items))) {
          const a = Object.hasOwn(assets, id) ? assets[id] : undefined;
          if (a === undefined || typeof a.fingerprint !== "string") continue;
          const seen = `${projectId}\0${a.fingerprint}`;
          if (used.has(seen) || asking.has(seen)) continue;
          asking.add(seen);
          const asked = yield* Effect.result(
            Effect.gen(function* () {
              const p = yield* plan(projectId, "main", id, a, "pcm48");
              if ((yield* hit(p.key, projectId)) !== undefined) return deps.afterPcm48?.(p);
              const job = yield* submit(MEDIA_SYSTEM_OWNER, p);
              deps.afterPcm48?.(p, job.id);
            }),
          );
          asking.delete(seen);
          // Asked for, or failed for good: not again this run. A transient failure is tried again.
          if (asked._tag === "Success" || CONTENT_FAILURES.has(asked.failure.code) || asked.failure.code === "MEDIA_LIMIT") used.add(seen);
          if (asked._tag === "Failure") log(`finisherd: media pcm: no pcm48 of ${id} in ${projectId} (${asked.failure.code})`);
        }
      }),
    ).catch(() => undefined);

  const planFeed = (projectId: string, branch: string, asset: string): Effect.Effect<PcmParams, FinError> =>
    Effect.gen(function* () {
      const assets = (yield* assetsOf(projectId, branch)).assets ?? {};
      return yield* plan(projectId, branch, asset, Object.hasOwn(assets, asset) ? assets[asset] : undefined, "pcm48");
    });

  return { request, used: usedHere, planFeed };
}

/** The stored probe's audio channels and codec (`probe.audio`, `probe.media.audio`), when the file has audio. */
function audioOf(f: StageFile): { readonly channels: number; readonly codec?: string; readonly duration?: string } | undefined {
  if (!isObj(f.probe) || !isObj(f.probe.audio) || typeof f.probe.audio.channels !== "number" || f.probe.audio.channels < 1) return undefined;
  const codec = codecOf(f.probe);
  const duration = typeof f.probe.duration === "string" ? f.probe.duration : undefined;
  return { channels: f.probe.audio.channels, ...(codec === undefined ? {} : { codec }), ...(duration === undefined ? {} : { duration }) };
}

/** `media.pcm` params for `f` as `pcm16m`; throws `CODEC_UNAVAILABLE` when nothing decodes it. */
function stageParams(f: StageFile): PcmParams {
  const audio = audioOf(f);
  if (audio === undefined) throw new Error("the file has no audio track");
  const p = mediaPcmPlan({ fingerprint: f.fp, recipe: "pcm16m", sourceCodec: audio.codec, channels: audio.channels, duration: audio.duration });
  return {
    fingerprint: f.fp,
    recipe: "pcm16m",
    layout: p.layout,
    decoderClass: p.decoderClass,
    key: p.key,
    file: { library: f.library, root: f.root, rel: f.rel, expect: f.expect, hydrate: f.hydrate },
  };
}

/** The `pcm16m` stage (Decision 20): sub-project 8's analysis feed of every library file with audio. */
export function pcm16mStage(deps: { readonly cache: () => MediaCache | undefined }): StageSpec {
  const v = mediaVersions();
  const recipeVersion = v.recipes[PCM16M_STAGE] ?? 0;
  return {
    name: PCM16M_STAGE,
    version: `${recipeVersion}.${v.renderVersion}.${v.audioVersion}`,
    dependsOn: ["probe"],
    jobKind: MEDIA_PCM,
    batch: 1,
    maxInFlight: 1,
    appliesTo: (f) => {
      if (f.kind !== "media" || audioOf(f) === undefined) return false;
      try {
        stageParams(f);
        return true;
      } catch {
        return false;
      }
    },
    params: (files) => stageParams(files[0]!),
    isDone: (fp) =>
      Effect.promise(async () => {
        const cache = deps.cache();
        if (cache === undefined) return false;
        try {
          return (await cache.entriesFor({ fingerprint: fp })).some(
            (e) => e.recipe === PCM16M_STAGE && e.recipeVersion === recipeVersion && e.renderVersion === v.renderVersion && (e.params as { audioVersion?: unknown } | null)?.audioVersion === v.audioVersion,
          );
        } catch {
          return false;
        }
      }),
  };
}
