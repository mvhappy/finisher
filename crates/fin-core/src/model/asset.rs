//! Assets: imported files, by kind, with what probing found.

use schemars::JsonSchema;
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
