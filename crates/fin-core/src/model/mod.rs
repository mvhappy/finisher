//! The canonical project model (spec §6.2; sub-project 4 spec §5.1, schema version 2).
//! Rational positions, short readable ids. Fields at their defaults are omitted from the JSON
//! (plan 4C Decision 11); the collections 4B already had (`assets`, `tracks`, `items`) are
//! always written.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::time::{Rational, frame_duration};

mod asset;
mod caption;
mod fonts;
mod item;
mod timeline;

pub use asset::{
    Asset, AssetKind, AudioInfo, FontFace, FontFile, ImageInfo, LottieInfo, LutInfo,
    MAX_IMAGE_DIMENSION, SlotSpec, VideoInfo,
};
pub use caption::{
    Anchor, CaptionBlock, CaptionBox, CaptionPosition, CaptionSource, CaptionStyle, Stroke, Word,
};
pub use fonts::{FontFormat, FontPin, FontSource, Fonts, font_key};
pub use item::{
    AudioParams, BlendMode, Common, Crop, Effect, Interp, Item, ItemBody, Keyframe, Mask,
    ParamValue, Transform,
};
pub use timeline::{
    Bin, Curve, Marker, Resolution, Timeline, Track, TrackKind, Transition, TransitionKind,
};

/// Version of the serialized model and op format.
pub const SCHEMA_VERSION: u32 = 2;

/// The resolution of a new project's timeline, and of a pre-4C project's (owner decision,
/// plan 4C Decision 9): vertical 1080×1920.
pub const DEFAULT_RESOLUTION: Resolution = Resolution {
    width: 1080,
    height: 1920,
};

/// The name of a new project's timeline, and of a pre-4C project's.
pub const DEFAULT_TIMELINE_NAME: &str = "Timeline 1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    // The genesis frame rate in frames per second, and the default for new timelines (a doc
    // comment here would flatten the Rational $ref to a plain string in the generated TS).
    pub rate: Rational,
    /// The first is the main timeline.
    pub timelines: Vec<Timeline>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bins: BTreeMap<String, Bin>,
    pub assets: BTreeMap<String, Asset>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub styles: BTreeMap<String, CaptionStyle>,
    #[serde(default, skip_serializing_if = "Fonts::is_empty")]
    pub fonts: Fonts,
}

impl Project {
    /// Every id the project holds: timelines and their markers, tracks and their
    /// transitions, items, their effects and link groups, bins, assets and styles (font pins
    /// are keyed by face, not by id). In that order; a link group shared by several items is
    /// listed once per item.
    pub fn entity_ids(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for timeline in &self.timelines {
            out.push(timeline.id.as_str());
            out.extend(timeline.markers.iter().map(|m| m.id.as_str()));
            for track in &timeline.tracks {
                out.push(track.id.as_str());
                out.extend(track.transitions.iter().map(|t| t.id.as_str()));
                for item in &track.items {
                    out.push(item.id.as_str());
                    out.extend(item.common.effects.iter().map(|e| e.id.as_str()));
                    out.extend(item.common.link.as_deref());
                }
            }
        }
        out.extend(self.bins.keys().map(String::as_str));
        out.extend(self.assets.keys().map(String::as_str));
        out.extend(self.styles.keys().map(String::as_str));
        out
    }
}

/// A new project (plan 4C Decision 9): timeline `tl1` "Timeline 1" at `rate` and
/// [`DEFAULT_RESOLUTION`], with video track `t1` "V1" and audio track `t2` "A1"; no bins,
/// assets, styles or font pins. It equals the upcast of 4B's rev-0 project. Fails with
/// `INVALID_RANGE` when `rate` is not positive.
pub fn new_project(id: String, name: String, rate: Rational) -> Result<Project, CoreError> {
    frame_duration(rate)?;
    Ok(Project {
        id,
        name,
        schema_version: SCHEMA_VERSION,
        rate,
        timelines: vec![Timeline {
            id: "tl1".to_string(),
            name: DEFAULT_TIMELINE_NAME.to_string(),
            rate,
            resolution: DEFAULT_RESOLUTION,
            markers: Vec::new(),
            tracks: vec![
                Track::new("t1", TrackKind::Video, "V1"),
                Track::new("t2", TrackKind::Audio, "A1"),
            ],
        }],
        bins: BTreeMap::new(),
        assets: BTreeMap::new(),
        styles: BTreeMap::new(),
        fonts: Fonts::default(),
    })
}

// Default tests for `skip_serializing_if`. Floats compare bit for bit: `-0.0` is not the
// default `0.0`, so it is written and reads back as itself.

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

pub(crate) fn is_true(b: &bool) -> bool {
    *b
}

pub(crate) fn is_zero(x: &f64) -> bool {
    x.to_bits() == 0
}

pub(crate) fn is_one(x: &f64) -> bool {
    x.to_bits() == 1f64.to_bits()
}

pub(crate) fn is_zero_time(r: &Rational) -> bool {
    r.is_zero()
}

pub(crate) fn is_one_time(r: &Rational) -> bool {
    *r == Rational::from_int(1)
}

pub(crate) fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}
