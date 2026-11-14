//! `media.probeDeep` v1 (plan 6B-1 Task 11; sub-project 6 spec §6.3): the decode-level probe of
//! one asset. The worker opens the original the manifest names, decodes its first frame with
//! the decoder `open_decoder` picks, and reads in-band HDR SEI when the container has no HDR
//! boxes. It answers what it found; nothing is written to the project.

use fin_core::events::payloads::JobMedia;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::backend::VendorClass;
use crate::format::{PixelFormat, VideoCodec};
use crate::info::{HdrInfo, Size};

/// `media.probeDeep` v1: probe `asset`, whose content is `fingerprint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeDeepParams {
    pub asset: String,
    /// The asset's fingerprint as the submitter saw it. The manifest's must equal it, else the
    /// job fails `ASSET_CHANGED` (the model changed between the request and the run).
    pub fingerprint: String,
    /// The project the asset is in. Only the host reads it (`prepare` writes the manifest
    /// from it); the worker works from the job directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch whose head names the asset (`main` when absent). Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    // The source's size as the model records it, for the kind's memory rule (plan Decision
    // 18). Host only. (A plain comment: a doc comment would inline a copy of `Size`.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Size")]
    pub display: Option<Size>,
    /// The worker's `RENDER_VERSION` and a digest of its capability report, which with the
    /// fingerprint make the job's identity (`dedupe`): another decoder set probes again.
    /// Host only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps_digest: Option<String>,
}

/// Where [`ProbeDeepResult::hdr`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HdrSource {
    /// The sample entry's `mdcv`/`clli`/`dvcC` boxes.
    Container,
    /// H.264/HEVC SEI in the first sample (no `mdcv`/`clli` box).
    Sei,
    /// Both, field by field: the boxes' fields, and from SEI the mastering display or content
    /// light level the boxes lack (a `dvcC` without `mdcv`/`clli`, or `mdcv` without `clli`).
    ContainerAndSei,
}

/// The first decoded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirstFrame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

/// Why the first frame did not decode: the decoder's error code and message (a codec nothing
/// on this machine decodes is `CODEC_UNAVAILABLE`; a bitstream the decoder refused is
/// `DECODE_FAILED` or `MEDIA_CORRUPT`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeFailure {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

/// What `media.probeDeep` v1 answers. `backend`, `class`, `hw` and `firstFrame` are present
/// exactly when `decodable`; otherwise `failure` says why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeDeepResult {
    pub asset: String,
    pub fingerprint: String,
    /// The video track's codec.
    pub codec: VideoCodec,
    /// The first frame decoded.
    pub decodable: bool,
    /// The decoder's backend (`dav1d`, `videotoolbox`, …), a `fin_media::backend::BACKENDS`
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VendorClass")]
    pub class: Option<VendorClass>,
    /// The frame was decoded on hardware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub hw: Option<bool>,
    // HDR metadata: the container's, else in-band SEI's; absent when neither has any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HdrInfo")]
    pub hdr: Option<HdrInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HdrSource")]
    pub hdr_source: Option<HdrSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "FirstFrame")]
    pub first_frame: Option<FirstFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProbeFailure")]
    pub failure: Option<ProbeFailure>,
    // The `media` object of the job's `job.done` (6B-1 Task 19): present when diagnostics were
    // on at the worker; numbers and table names only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "JobMedia")]
    pub media: Option<JobMedia>,
}

/// The boxes' HDR metadata, with the mastering display and content light level they lack taken
/// from SEI (read only when one is missing), field by field, and where the result came from.
pub fn merge_hdr(
    boxes: Option<HdrInfo>,
    sei: impl FnOnce() -> Option<HdrInfo>,
) -> (Option<HdrInfo>, Option<HdrSource>) {
    let boxes = boxes.filter(|h| !h.is_empty());
    let complete = boxes.is_some_and(|h| h.mastering.is_some() && h.content_light.is_some());
    let sei = if complete { None } else { sei() };
    let Some(sei) = sei.filter(|h| h.mastering.is_some() || h.content_light.is_some()) else {
        return (boxes, boxes.map(|_| HdrSource::Container));
    };
    let Some(mut out) = boxes else {
        return (Some(sei), Some(HdrSource::Sei));
    };
    let filled = (out.mastering.is_none() && sei.mastering.is_some())
        || (out.content_light.is_none() && sei.content_light.is_some());
    out.mastering = out.mastering.or(sei.mastering);
    out.content_light = out.content_light.or(sei.content_light);
    let source = if filled {
        HdrSource::ContainerAndSei
    } else {
        HdrSource::Container
    };
    (Some(out), Some(source))
}
