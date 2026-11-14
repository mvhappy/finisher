//! `mediaPcmPlan` (plan 6C Task 18, Decision 7): the cache key a `media.pcm` job of a source will
//! commit under, computed by the host before it submits (the worker computes it again from the
//! decoder it opens and the source's channels: `PLAN_MISMATCH {reason: cacheKey}` when they
//! differ). The decoder is the first backend of `fin_media::backend_audio::AUDIO_BACKENDS` that
//! decodes the source's audio codec (symphonia for a source the probe names no codec for).

use fin_core::CoreError;
use fin_media::AudioCodec;
use fin_media::backend::VendorClass;
use fin_media::jobs::{PcmLayout, PcmRecipe, pcm_plan_key};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::{encode, reject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    fingerprint: String,
    recipe: PcmRecipe,
    /// The source's audio codec (the probe's `media.audio.codec`), when the probe names one.
    #[serde(default)]
    source_codec: Option<AudioCodec>,
    /// The source's channel count (the asset's `audio.channels`).
    channels: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    key: String,
    decoder: String,
    decoder_class: VendorClass,
    layout: PcmLayout,
}

/// `{fingerprint, recipe, sourceCodec?, channels}` JSON → `{key, decoder, decoderClass, layout}`
/// JSON; rejects `CODEC_UNAVAILABLE` when no backend of this build decodes the codec,
/// `ASSET_TYPE_UNSUPPORTED` for no channel and `INVALID_REQUEST` for a request that does not
/// parse.
#[napi(catch_unwind)]
pub fn media_pcm_plan(request_json: String) -> Result<String> {
    let req: PlanRequest = serde_json::from_str(&request_json).map_err(|e| {
        reject(CoreError::new(
            "INVALID_REQUEST",
            format!("mediaPcmPlan: {e}"),
        ))
    })?;
    let p = pcm_plan_key(&req.fingerprint, req.recipe, req.source_codec, req.channels)
        .map_err(reject)?;
    encode(&Plan {
        key: p.key.to_string(),
        decoder: p.decoder.to_string(),
        decoder_class: p.decoder_class,
        layout: p.layout,
    })
}
