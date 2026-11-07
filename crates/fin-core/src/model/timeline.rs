//! Timelines, their tracks, markers and transitions, and bins.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::item::Item;
use crate::model::{is_default, is_false, is_zero};
use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub id: String,
    pub name: String,
    // Frame rate in frames per second.
    pub rate: Rational,
    pub resolution: Resolution,
    /// Sorted by `at`. Markers never ripple.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Compositing order, bottom first.
    pub tracks: Vec<Track>,
}

/// Frame size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    pub id: String,
    // Timeline position in seconds.
    pub at: Rational,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub duration: Option<Rational>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
    Caption,
}

impl TrackKind {
    /// The letter of the default track names (`V1`, `A1`, `C1`).
    pub const fn letter(self) -> char {
        match self {
            TrackKind::Video => 'V',
            TrackKind::Audio => 'A',
            TrackKind::Caption => 'C',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mute: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub lock: bool,
    /// Decibels.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain: f64,
    /// Sorted by `start`, non-overlapping.
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
}

impl Track {
    /// An empty track with every flag off and 0 dB gain.
    pub fn new(id: impl Into<String>, kind: TrackKind, name: impl Into<String>) -> Track {
        Track {
            id: id.into(),
            kind,
            name: name.into(),
            mute: false,
            solo: false,
            lock: false,
            gain: 0.0,
            items: Vec::new(),
            transitions: Vec::new(),
        }
    }
}

/// A transition at the cut between two touching items of one track, centred on the cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    /// The outgoing item, then the incoming one.
    pub between: [String; 2],
    pub kind: TransitionKind,
    // Seconds.
    pub duration: Rational,
    #[serde(default, skip_serializing_if = "is_default")]
    pub curve: Curve,
}

/// How a transition mixes its two items (sub-project 6 spec §4.3): both mix by
/// `curve(progress)`; they differ in the space they mix in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TransitionKind {
    /// Mixes display-encoded (BT.1886) values.
    Crossfade,
    /// Mixes linear light.
    Dissolve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Curve {
    #[default]
    Linear,
    EqualPower,
}

/// A folder of assets; `parent` nests it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Bin {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub parent: Option<String>,
}
