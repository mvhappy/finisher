//! `mediaPcmPlan` (plan 6C Task 18, Decision 7): the cache key a `media.pcm` job of a source will
//! commit under, computed by the host before it submits (the worker computes it again from the
//! decoder it opens and the source's channels: `PLAN_MISMATCH {reason: cacheKey}` when they
//! differ). The decoder is the first backend of `fin_media::backend_audio::AUDIO_BACKENDS` that
//! decodes the source's audio codec (symphonia for a source the probe names no codec for).

use fin_core::{CoreError, Rational};
use fin_media::AudioCodec;
use fin_media::backend::VendorClass;
use fin_media::jobs::{PcmLayout, PcmRecipe, check_pcm_duration, pcm_plan_key};
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
    /// The source's probed duration (rational seconds), when known: past 24 h is `MEDIA_LIMIT`.
    #[serde(default)]
    duration: Option<Rational>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    key: String,
    decoder: String,
    decoder_class: VendorClass,
    layout: PcmLayout,
}

/// `{fingerprint, recipe, sourceCodec?, channels, duration?}` JSON → `{key, decoder,
/// decoderClass, layout}` JSON; rejects `MEDIA_LIMIT {limit: maxSeconds}` for a duration past
/// 24 h (before any job), `CODEC_UNAVAILABLE` when no backend of this build decodes the codec,
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
    if let Some(d) = req.duration {
        check_pcm_duration(d).map_err(reject)?;
    }
    let p = pcm_plan_key(&req.fingerprint, req.recipe, req.source_codec, req.channels)
        .map_err(reject)?;
    encode(&Plan {
        key: p.key.to_string(),
        decoder: p.decoder.to_string(),
        decoder_class: p.decoder_class,
        layout: p.layout,
    })
}
