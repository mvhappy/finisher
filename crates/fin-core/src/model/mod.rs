//! The canonical project model (spec §6.2). Rational positions, short readable ids.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::time::{Rational, frame_duration};

/// Version of the serialized model and op format.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    // Timeline frame rate in frames per second (a doc comment here would flatten the
    // Rational $ref to a plain string in the generated TS).
    pub rate: Rational,
    pub timeline: Timeline,
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub id: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    /// Sorted by `start`, non-overlapping.
    pub items: Vec<Clip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub asset: String,
    // Position on the timeline, in seconds.
    pub start: Rational,
    // Offset into the asset, in seconds.
    pub source_in: Rational,
    pub duration: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub path: String,
    pub duration: Rational,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub rate: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A new project with timeline `tl1`, video track `t1` and audio track `t2`.
/// Fails with `INVALID_RANGE` when `rate` is not positive.
pub fn new_project(id: String, name: String, rate: Rational) -> Result<Project, CoreError> {
    frame_duration(rate)?;
    let track = |id: &str, kind| Track {
        id: id.to_string(),
        kind,
        items: Vec::new(),
    };
    Ok(Project {
        id,
        name,
        schema_version: SCHEMA_VERSION,
        rate,
        timeline: Timeline {
            id: "tl1".to_string(),
            tracks: vec![track("t1", TrackKind::Video), track("t2", TrackKind::Audio)],
        },
        assets: BTreeMap::new(),
    })
}
