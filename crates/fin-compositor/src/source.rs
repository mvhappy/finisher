//! Source pixels for a layer.

use fin_core::Rational;
use fin_core::eval::{FramePlan, ItemKind, Layer};
use fin_media::{ColourTags, HdrInfo, PixelFormat, Rotation, Size};
use serde_json::{Value, json};

use crate::output::Skipped;

/// A decoded source picture as a compositor reads it: coded orientation, planes with strides
/// (in bytes).
///
/// - `I420` (dav1d): `planes` are Y, U and V, one byte per sample.
/// - `Nv12` (platform decoders): `planes[0]` is Y, `planes[1]` interleaved UV (U then V, one
///   byte each, so a row holds 2 × chroma width bytes); `planes[2]` is unused (empty, stride 0).
/// - `P010` (platform decoders): as `Nv12` in little-endian 16-bit words, the 10-bit value in
///   the high bits (`word >> 6`).
///
/// Strides are whole texels: a UV pair for interleaved chroma, a word for P010.
#[derive(Debug, Clone, Copy)]
pub struct SourceFrame<'a> {
    /// `I420`, `Nv12` or `P010`.
    pub format: PixelFormat,
    pub size: Size,
    pub planes: [&'a [u8]; 3],
    pub strides: [usize; 3],
    /// Resolved tags (the untagged rule applied).
    pub colour: ColourTags,
    /// The source's HDR metadata (`clli`, `mdcv`): a PQ source's peak for the tone mapping
    /// (spec §10.1); `None` for SDR and for HDR without it (1,000 nits).
    pub hdr: Option<HdrInfo>,
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

/// Whether a compositor can read `src`: 4:2:0 as `I420`, `Nv12` or `P010` (see
/// [`SourceFrame`]), a non-empty picture, planes as long as their strides and rows say, strides
/// whole texels (an NV12 or P010 UV pair, a P010 word: the GPU copies texel rows). Otherwise the details of `UNSUPPORTED_PIXEL_FORMAT`.
pub(crate) fn readable(src: &SourceFrame) -> Result<(), Value> {
    let (w, h) = (src.size.width as usize, src.size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    // A plane `k` of `ph` rows of `pw` bytes, its stride a multiple of `unit` bytes.
    let fits = |k: usize, pw: usize, ph: usize, unit: usize| {
        src.strides[k] >= pw
            && src.strides[k].is_multiple_of(unit)
            && ph
                .checked_mul(src.strides[k])
                .is_some_and(|n| src.planes[k].len() >= n)
    };
    let ok = match src.format {
        PixelFormat::I420 => fits(0, w, h, 1) && fits(1, cw, ch, 1) && fits(2, cw, ch, 1),
        PixelFormat::Nv12 => fits(0, w, h, 1) && fits(1, 2 * cw, ch, 2),
        PixelFormat::P010 => fits(0, 2 * w, h, 2) && fits(1, 4 * cw, ch, 4),
        _ => {
            return Err(json!({
                "format": src.format,
                "reason": "the compositor reads 4:2:0 as I420, NV12 or P010"
            }));
        }
    };
    if w == 0 || h == 0 || !ok {
        return Err(
            json!({ "format": src.format, "reason": "the planes are smaller than the picture" }),
        );
    }
    Ok(())
}

fn skipped(l: &Layer, code: &str, details: Value) -> Skipped {
    Skipped {
        item: l.item.clone(),
        kind: l.kind,
        code: code.to_string(),
        details,
    }
}

/// The layer walk both compositors share (spec §2.7): bottom layer first, `draw` is called for
/// each clip layer with a readable source picture; everything left out is returned in order.
/// Layers 6A cannot draw (non-clip kinds, nested `children`) and the effects and masks of a
/// drawn clip are `LAYER_UNAVAILABLE` (`reason: notYet`); a clip without a source is
/// `noSource`; the provider's issues keep their codes; unreadable pictures are
/// `UNSUPPORTED_PIXEL_FORMAT`.
pub(crate) fn walk<E>(
    plan: &FramePlan,
    src: &mut dyn SourceProvider,
    mut draw: impl FnMut(&Layer, &SourceFrame) -> Result<(), E>,
) -> Result<Vec<Skipped>, E> {
    let mut skips = Vec::new();
    for l in &plan.layers {
        if l.kind != ItemKind::Clip || l.children.is_some() {
            skips.push(skipped(
                l,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet" }),
            ));
            continue;
        }
        for e in &l.effects {
            skips.push(skipped(
                l,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet", "effect": e.id, "effectKind": e.kind }),
            ));
        }
        if let Some(m) = &l.mask {
            skips.push(skipped(
                l,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet", "mask": m.matte }),
            ));
        }
        if l.source.is_none() {
            skips.push(skipped(
                l,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "noSource" }),
            ));
            continue;
        }
        match src.frame(l) {
            Ok(frame) => match readable(&frame) {
                Ok(()) => draw(l, &frame)?,
                Err(details) => skips.push(skipped(l, "UNSUPPORTED_PIXEL_FORMAT", details)),
            },
            Err(issue) => skips.push(skipped(l, &issue.code, issue.details)),
        }
    }
    Ok(skips)
}
