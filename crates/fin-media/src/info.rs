//! What a media file is (spec §6.3): `MediaInfo`, read from the container by `fin-container`
//! and answered by `asset.probe` (never stored in an `asset.add` record, spec §4.4).

use std::borrow::Cow;

use fin_core::Rational;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::colour::ColourTags;
use crate::format::{AudioCodec, Chroma, ContainerKind, VideoCodec};

/// A clockwise display rotation from the track matrix; serialised as degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(into = "u16", try_from = "u16")]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn degrees(self) -> u16 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    /// Whether width and height swap on display.
    pub fn swaps(self) -> bool {
        matches!(self, Rotation::R90 | Rotation::R270)
    }
}

impl From<Rotation> for u16 {
    fn from(r: Rotation) -> u16 {
        r.degrees()
    }
}

impl TryFrom<u16> for Rotation {
    type Error = String;
    fn try_from(d: u16) -> Result<Rotation, String> {
        match d {
            0 => Ok(Rotation::R0),
            90 => Ok(Rotation::R90),
            180 => Ok(Rotation::R180),
            270 => Ok(Rotation::R270),
            _ => Err(format!("rotation must be 0, 90, 180 or 270, not {d}")),
        }
    }
}

impl JsonSchema for Rotation {
    fn schema_name() -> Cow<'static, str> {
        "Rotation".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "enum": [0, 90, 180, 270],
            "description": "Clockwise display rotation in degrees, from the track matrix."
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Size {
        Size { width, height }
    }
}

/// The display size of a picture coded at `coded` (spec §6.3): the width scaled by the pixel
/// aspect ratio (exact, rounded to the nearest even number, halves up, at least 2 when the
/// width is not 0), then width and height swapped for a 90° or 270° rotation. A pixel aspect
/// that is not positive counts as square.
pub fn display_size(coded: Size, rotation: Rotation, pixel_aspect: Rational) -> Size {
    let width = if pixel_aspect.is_positive() && pixel_aspect != Rational::from_int(1) {
        let (n, d) = (pixel_aspect.num() as i128, pixel_aspect.den() as i128);
        let halves = (coded.width as i128 * n + d) / (2 * d);
        let w = (2 * halves).clamp(if coded.width == 0 { 0 } else { 2 }, u32::MAX as i128 - 1);
        w as u32
    } else {
        coded.width
    };
    match rotation.swaps() {
        true => Size::new(coded.height, width),
        false => Size::new(width, coded.height),
    }
}

/// SMPTE ST 2086 mastering display colour volume (`mdcv`), in the box's own units:
/// chromaticities in 0.00002, luminance in 0.0001 cd/m².
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MasteringDisplay {
    /// `[x, y]` for green, blue, red (the box's order).
    pub primaries: [[u16; 2]; 3],
    pub white_point: [u16; 2],
    pub max_luminance: u32,
    pub min_luminance: u32,
}

/// CTA-861.3 content light level (`clli`), in cd/m².
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContentLight {
    pub max_cll: u16,
    pub max_fall: u16,
}

/// A Dolby Vision configuration record (`dvcC`/`dvvC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DolbyVision {
    pub profile: u8,
    pub level: u8,
    pub rpu: bool,
    pub enhancement_layer: bool,
    pub base_layer: bool,
    /// The base layer's signal compatibility id (8.1 HDR10, 8.4 HLG, …).
    pub compatibility: u8,
}

/// HDR metadata a source carries (kept for HDR export later, spec §10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HdrInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "MasteringDisplay")]
    pub mastering: Option<MasteringDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ContentLight")]
    pub content_light: Option<ContentLight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DolbyVision")]
    pub dolby_vision: Option<DolbyVision>,
}

impl HdrInfo {
    pub fn is_empty(&self) -> bool {
        self.mastering.is_none() && self.content_light.is_none() && self.dolby_vision.is_none()
    }
}

/// The first video track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VideoTrackInfo {
    pub codec: VideoCodec,
    /// The sample entry type (`av01`, `hvc1`, …).
    pub fourcc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u8")]
    pub profile: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u8")]
    pub level: Option<u8>,
    pub bit_depth: u8,
    pub chroma: Chroma,
    /// The sample entry's coded size.
    pub coded: Size,
    /// `coded` after the pixel aspect and the rotation ([`display_size`]).
    pub display: Size,
    pub rotation: Rotation,
    /// Horizontal over vertical spacing (`pasp`); 1 when absent.
    pub pixel_aspect: Rational,
    pub colour: ColourTags,
    /// An ICC profile was given instead of code points (`colr` `prof`/`rICC`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub icc: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HdrInfo")]
    pub hdr: Option<HdrInfo>,
    /// Frames per second of the most common frame duration.
    pub nominal_rate: Rational,
    /// Frame durations differ by more than 0.1 % (the last frame ignored).
    pub vfr: bool,
    /// Frames presented, after edit lists.
    pub frames: u64,
}

/// The first audio track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackInfo {
    pub codec: AudioCodec,
    pub fourcc: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Samples to drop at the start (encoder delay: edit-list media time, `roll`, Opus
    /// pre-skip).
    pub priming: u32,
    /// Samples to drop at the end.
    pub padding: u32,
}

/// A track `fin-container` does not interpret (a second video or audio track, timecode, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IgnoredTrack {
    pub id: u32,
    /// The `hdlr` handler type (`vide`, `soun`, `tmcd`, …).
    pub handler: String,
}

/// What a media file is (spec §6.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub container: ContainerKind,
    /// Seconds: the longest interpreted track after edit lists.
    pub duration: Rational,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fragmented: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VideoTrackInfo")]
    pub video: Option<VideoTrackInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AudioTrackInfo")]
    pub audio: Option<AudioTrackInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<IgnoredTrack>,
}
