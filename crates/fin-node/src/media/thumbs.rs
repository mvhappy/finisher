//! `mediaThumbsPlan` (plan 6B-1 Task 15, Decision 11): the cache key a `media.thumbs` job of a
//! source will commit under, and its items, computed by the host before it submits (the worker
//! computes the key again from the original's decoder class, `PLAN_MISMATCH {reason: cacheKey}`
//! when they differ). fin-media's `plan_thumbs` over the generated `ThumbsPlanRequest` and
//! `ThumbsPlan`.

use fin_core::CoreError;
use fin_media::jobs::{ThumbsPlanRequest, plan_thumbs};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{encode, reject};

/// `ThumbsPlanRequest` JSON → `ThumbsPlan` JSON; rejects `CODEC_UNAVAILABLE` when no listed
/// backend decodes the source's codec and `INVALID_REQUEST` for a request that does not parse.
#[napi(catch_unwind)]
pub fn media_thumbs_plan(request_json: String) -> Result<String> {
    let req: ThumbsPlanRequest = serde_json::from_str(&request_json).map_err(|e| {
        reject(CoreError::new(
            "INVALID_REQUEST",
            format!("mediaThumbsPlan: {e}"),
        ))
    })?;
    encode(&plan_thumbs(req).map_err(reject)?)
}
