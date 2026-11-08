//! Source pixels for a layer.

use fin_core::Rational;
use fin_core::eval::{FramePlan, ItemKind, Layer};
use fin_media::{ColourTags, PixelFormat, Rotation, Size};
use serde_json::{Value, json};

use crate::output::Skipped;

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

/// Whether a compositor can read `src`: 8-bit 4:2:0 (6A), a non-empty picture, planes as long
/// as their strides and rows say. Otherwise the details of `UNSUPPORTED_PIXEL_FORMAT`.
pub(crate) fn readable(src: &SourceFrame) -> Result<(), Value> {
    let (w, h) = (src.size.width as usize, src.size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fits = |k: usize, pw: usize, ph: usize| {
        src.strides[k] >= pw
            && ph
                .checked_mul(src.strides[k])
                .is_some_and(|n| src.planes[k].len() >= n)
    };
    if src.format != PixelFormat::I420 {
        return Err(json!({ "format": src.format, "reason": "the compositor reads 8-bit 4:2:0" }));
    }
    if w == 0 || h == 0 || !fits(0, w, h) || !fits(1, cw, ch) || !fits(2, cw, ch) {
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
