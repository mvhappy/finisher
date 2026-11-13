//! `mediaThumbsPlan` (plan 6B-1 Task 15, Decision 11): the cache key a `media.thumbs` job of a
//! source will commit under, and its items, computed by the host before it submits (the worker
//! computes the key again from the original's decoder class, `PLAN_MISMATCH {reason: cacheKey}`
//! when they differ). A filmstrip's sprite count comes from the source's duration here.

use fin_core::{CoreError, Rational};
use fin_media::VideoCodec;
use fin_media::backend::VendorClass;
use fin_media::jobs::{ThumbsMode, plan_thumbs_key, strip_sprites};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::{encode, reject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    fingerprint: String,
    /// A filmstrip with `sprites: 0` (or none) is planned from `duration`.
    mode: ThumbsMode,
    /// The source's video codec (the asset's `video.codec`).
    source_codec: VideoCodec,
    /// The report's backends for that codec, in the order they are tried.
    decoders: Vec<String>,
    /// The source's duration in seconds (`MediaInfo.duration`; filmstrips).
    #[serde(default)]
    duration: Option<Rational>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    key: String,
    decoder: String,
    decoder_class: VendorClass,
    /// The mode with its sprite count filled in.
    mode: ThumbsMode,
    /// The job's items.
    items: Vec<String>,
}

fn plan(mut req: PlanRequest) -> std::result::Result<Plan, CoreError> {
    if let ThumbsMode::Strip { sprites, .. } = &mut req.mode
        && *sprites == 0
    {
        let duration = req.duration.ok_or_else(|| {
            CoreError::new(
                "INVALID_REQUEST",
                "mediaThumbsPlan: a filmstrip needs the source's duration",
            )
        })?;
        *sprites = strip_sprites(duration);
    }
    let (key, b) = plan_thumbs_key(&req.fingerprint, &req.mode, req.source_codec, &req.decoders)?;
    Ok(Plan {
        key: key.to_string(),
        decoder: b.name.to_string(),
        decoder_class: b.class,
        items: req.mode.items(),
        mode: req.mode,
    })
}

/// `{fingerprint, mode, sourceCodec, decoders, duration?}` JSON → `{key, decoder, decoderClass,
/// mode, items}` JSON; rejects `CODEC_UNAVAILABLE` when no listed backend decodes the source's
/// codec and `INVALID_REQUEST` for a request that does not parse.
#[napi(catch_unwind)]
pub fn media_thumbs_plan(request_json: String) -> Result<String> {
    let req: PlanRequest = serde_json::from_str(&request_json).map_err(|e| {
        reject(CoreError::new(
            "INVALID_REQUEST",
            format!("mediaThumbsPlan: {e}"),
        ))
    })?;
    encode(&plan(req).map_err(reject)?)
}
