//! Assets: imported files, by kind, with what probing found.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::model::fonts::FontFormat;
use crate::time::Rational;

/// What an asset is: `media` (video and/or audio; every pre-4C asset), `image`, `svg`,
/// `lottieTemplate` (a dotLottie template with replaceable slots), `font`, or `lut` (a `.cube`
/// colour lookup table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Media,
    Image,
    Svg,
    LottieTemplate,
    Font,
    Lut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub path: String,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub bin: Option<String>,
    // Length in seconds (media, animated templates); absent for stills, fonts and LUTs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub duration: Option<Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VideoInfo")]
    pub video: Option<VideoInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AudioInfo")]
    pub audio: Option<AudioInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ImageInfo")]
    pub image: Option<ImageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "LottieInfo")]
    pub lottie: Option<LottieInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "FontFile")]
    pub font: Option<FontFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "LutInfo")]
    pub lut: Option<LutInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    /// The coded width, in px.
    pub width: u32,
    /// The coded height, in px.
    pub height: u32,
    pub rate: Rational,
    // Schema 5 (sub-project 6 spec §6.3); each absent means unknown (an asset probed before
    // schema 5). No doc comments: a schema `$ref` with a sibling `description` would be
    // inlined in the generated TypeScript instead of naming its type.
    // `display`: the coded size after the pixel aspect and the rotation
    // (`fin_media::display_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DisplaySize")]
    pub display: Option<DisplaySize>,
    // The clockwise display rotation from the track matrix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rotation")]
    pub rotation: Option<Rotation>,
    // Horizontal over vertical sample spacing (`pasp`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub pixel_aspect: Option<Rational>,
}

impl VideoInfo {
    /// A video stream of the given coded size and rate whose display geometry is unknown.
    pub fn coded(width: u32, height: u32, rate: Rational) -> VideoInfo {
        VideoInfo {
            width,
            height,
            rate,
            display: None,
            rotation: None,
            pixel_aspect: None,
        }
    }
}

/// A picture's display size in px (`VideoInfo.display`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySize {
    pub width: u32,
    pub height: u32,
}

/// A clockwise display rotation from the track matrix; serialised as degrees. `fin_media`
/// re-exports it (its `MediaInfo` uses the same type).
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// The widest and tallest `image` or `svg` asset, in px (`asset.add` refuses larger).
pub const MAX_IMAGE_DIMENSION: u32 = 32_768;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
}

/// A dotLottie template: its canvas, timing, replaceable slots and the font families it names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LottieInfo {
    pub width: u32,
    pub height: u32,
    pub rate: Rational,
    pub duration: Rational,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SlotSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<String>,
}

/// One replaceable slot of a template: its id and value kind (as the template declares it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlotSpec {
    pub id: String,
    pub kind: String,
}

/// A `.cube` LUT: its edge size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LutInfo {
    pub size: u32,
}

/// An imported font file (plan 4C Decision 37): its whole-file SHA-256, size, format and faces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontFile {
    pub sha256: String,
    pub size: u64,
    pub format: FontFormat,
    pub faces: Vec<FontFace>,
}

/// One face of a font file, as `fin_text::font_info` reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontFace {
    pub index: u32,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// ISO 15924 script codes the face covers.
    pub scripts: Vec<String>,
}
