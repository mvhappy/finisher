//! Source pixels for a layer.

use fin_core::Rational;
use fin_core::eval::Layer;
use fin_media::{ColourTags, PixelFormat, Rotation, Size};
use serde_json::Value;

/// A decoded source picture as a compositor reads it: coded orientation, planes with strides.
#[derive(Debug, Clone, Copy)]
pub struct SourceFrame<'a> {
    /// `I420` in 6A.
    pub format: PixelFormat,
    pub size: Size,
    pub planes: [&'a [u8]; 3],
    pub strides: [usize; 3],
    /// Resolved tags (the untagged rule applied).
    pub colour: ColourTags,
    pub rotation: Rotation,
    pub pixel_aspect: Rational,
}

/// Why a layer has no source picture (`CODEC_UNAVAILABLE`, `ORIGINALS_MISSING`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerIssue {
    pub code: String,
    pub details: Value,
}

/// Gives each clip layer its source picture at the layer's source time (the nearest frame).
pub trait SourceProvider {
    fn frame(&mut self, layer: &Layer) -> Result<SourceFrame<'_>, LayerIssue>;
}
