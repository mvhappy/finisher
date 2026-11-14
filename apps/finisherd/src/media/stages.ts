// The media library stages (6B-1 plan Task 14; sub-project 6 spec §7.1 "When", cp §7.3): 5D's
// indexer makes a `540` proxy of every video file of a library, by `media.proxy` jobs of one
// fingerprint each.
// - `appliesTo`: a `media` file whose stored probe has a video track this machine decodes (the
//   capability report lists a decoder for its codec); audio-only files, images and codecs nothing
//   here decodes are skipped.
// - `version`: `<recipe version>.<RENDER_VERSION>`, so a new recipe or render version makes
//   every proxy again (their keys change too).
// - `isDone`: the cache holds a `proxy-540` entry of the fingerprint made at this build's recipe
//   and render versions (adopted entries included); an older one is made again.
// - `params`: the file as the catalogue knows it (raw chain and `Expect`), the codec of
//   Decision 5 and the key planned from the report (`mediaProxyPlan`).
import type { ProxyParams } from "@finisher/core-types";
import { type MediaCache, mediaProxyPlan, mediaVersions } from "@finisher/fin-node";
import { Effect } from "effect";
import { MEDIA_PROXY, decodersOf, proxyCodec } from "../jobs/kinds/media.proxy.ts";
import type { StageFile, StageSpec } from "../library/api.ts";
import { sourceAudioOf } from "./proxy.ts";

export const PROXY540_STAGE = "proxy540";
export const PROXY540_RECIPE = "proxy-540";

export interface MediaStageDeps {
  /** The cached capability report (the stage is registered once there is one). */
  readonly report: () => unknown;
  readonly cache: () => MediaCache | undefined;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** The stored probe's video track (`probe.media.video`), when the file has one. */
function videoOf(f: StageFile): { readonly codec: string; readonly display?: { width: number; height: number } } | undefined {
  const media = isObj(f.probe) ? f.probe.media : undefined;
  const video = isObj(media) ? media.video : undefined;
  if (!isObj(video) || typeof video.codec !== "string") return undefined;
  const d = video.display;
  const display = isObj(d) && typeof d.width === "number" && typeof d.height === "number" ? { width: d.width, height: d.height } : undefined;
  return { codec: video.codec, ...(display === undefined ? {} : { display }) };
}

/** `media.proxy` params for `f` at `540`; throws `CODEC_UNAVAILABLE` when nothing decodes it. */
function paramsOf(report: unknown, f: StageFile): ProxyParams {
  const video = videoOf(f);
  if (video === undefined) throw new Error("the file has no video track");
  const codec = proxyCodec(report);
  const audio = sourceAudioOf(isObj(f.probe) ? f.probe.media : undefined);
  const plan = mediaProxyPlan({ fingerprint: f.fp, tier: "540", codec, sourceCodec: video.codec, decoders: decodersOf(report, video.codec), ...(audio === undefined ? {} : { sourceAudio: audio }) });
  return {
    fingerprint: f.fp,
    tier: "540",
    codec,
    decoderClass: plan.decoderClass,
    audio: plan.audio,
    key: plan.key,
    file: { library: f.library, root: f.root, rel: f.rel, expect: f.expect, hydrate: f.hydrate },
    ...(video.display === undefined ? {} : { display: video.display }),
  };
}

/** The `proxy540` stage. */
export function proxy540Stage(deps: MediaStageDeps): StageSpec {
  const v = mediaVersions();
  const recipeVersion = v.recipes[PROXY540_RECIPE] ?? 0;
  return {
    name: PROXY540_STAGE,
    version: `${recipeVersion}.${v.renderVersion}`,
    jobKind: MEDIA_PROXY,
    batch: 1,
    maxInFlight: 1,
    appliesTo: (f) => {
      if (f.kind !== "media" || videoOf(f) === undefined) return false;
      try {
        paramsOf(deps.report(), f);
        return true;
      } catch {
        return false;
      }
    },
    params: (files) => paramsOf(deps.report(), files[0]!),
    isDone: (fp) =>
      Effect.promise(async () => {
        const cache = deps.cache();
        if (cache === undefined) return false;
        try {
          return (await cache.entriesFor({ fingerprint: fp })).some((e) => e.recipe === PROXY540_RECIPE && e.recipeVersion === recipeVersion && e.renderVersion === v.renderVersion);
        } catch {
          return false;
        }
      }),
  };
}
