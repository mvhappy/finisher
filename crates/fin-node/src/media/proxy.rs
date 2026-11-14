//! `mediaProxyPlan` (plan 6B-1 Task 14, Decision 11): the cache key a `media.proxy` job of a
//! source will commit under, computed by the host before it submits (the worker computes it
//! again from the decoder it opens, `PLAN_MISMATCH {reason: cacheKey}` when they differ). The
//! decoder is the first backend of the capability report's list for the source's codec that
//! `fin_media::backend::BACKENDS` says decodes it; its vendor class is part of the key.

use fin_core::CoreError;
use fin_media::VideoCodec;
use fin_media::backend::VendorClass;
use fin_media::jobs::{ProxyCodec, ProxyTier, plan_key};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::{encode, reject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRequest {
    fingerprint: String,
    tier: ProxyTier,
    codec: ProxyCodec,
    /// The source's video codec (the asset's `video.codec`).
    source_codec: VideoCodec,
    /// The report's backends for that codec, in the order they are tried.
    decoders: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    key: String,
    decoder: String,
    decoder_class: VendorClass,
}

fn plan(req: &PlanRequest) -> std::result::Result<Plan, CoreError> {
    let (key, b) = plan_key(
        &req.fingerprint,
        req.tier,
        req.codec,
        req.source_codec,
        &req.decoders,
    )?;
    Ok(Plan {
        key: key.to_string(),
        decoder: b.name.to_string(),
        decoder_class: b.class,
    })
}

/// `{fingerprint, tier, codec, sourceCodec, decoders}` JSON → `{key, decoder, decoderClass}`
/// JSON; rejects `CODEC_UNAVAILABLE` when no listed backend decodes the source's codec and
/// `INVALID_REQUEST` for a request that does not parse.
#[napi(catch_unwind)]
pub fn media_proxy_plan(request_json: String) -> Result<String> {
    let req: PlanRequest = serde_json::from_str(&request_json).map_err(|e| {
        reject(CoreError::new(
            "INVALID_REQUEST",
            format!("mediaProxyPlan: {e}"),
        ))
    })?;
    encode(&plan(&req).map_err(reject)?)
}

/// `{renderVersion, audioVersion, recipes: {<name>: <version>}}` JSON: this build's
/// `RENDER_VERSION`, `AUDIO_VERSION` (6C Task 18) and cache recipes (a library stage's version is
/// made of them, 6B-1 Task 14).
#[napi(catch_unwind)]
pub fn media_versions() -> Result<String> {
    let recipes: serde_json::Map<String, serde_json::Value> = fin_media::cache::RECIPES
        .iter()
        .map(|r| (r.name.to_string(), serde_json::json!(r.version)))
        .collect();
    encode(&serde_json::json!({
        "renderVersion": fin_media::RENDER_VERSION,
        "audioVersion": fin_media::AUDIO_VERSION,
        "recipes": recipes,
    }))
}
