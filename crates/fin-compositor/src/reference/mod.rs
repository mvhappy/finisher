//! The CPU reference compositor (spec §5.1, §13.2): the oracle every GPU render is compared
//! with, and itself pinned by BLAKE3 hashes (T0). Scalar `f32` in a fixed order, `libm` for
//! transcendentals, the shared `geometry`, `coverage` and `mips` maths and `fin-media`'s colour
//! conversion; nothing else is shared with the GPU path.
//!
//! The frame is opaque: the accumulation starts black and keeps an implicit alpha of 1, so
//! each blend is the W3C formula with an opaque backdrop, `cb·(1 − αs) + αs·B(cb, Cs)`, with
//! the source's straight colour `Cs` (6A sources are opaque pictures) and `αs` its coverage ×
//! opacity. A transition's incoming layer is mixed with the composite below by
//! `curve(progress)`: `crossfade` in display-encoded values, `dissolve` in linear light.

mod blend;
mod ingest;
mod layer;
mod mipchain;

use fin_core::eval::{FramePlan, ItemKind};
use fin_media::Size;
use serde_json::json;

use crate::output::{RenderOutput, Skipped};
use crate::source::SourceProvider;

fn skipped(item: &str, kind: ItemKind, code: &str, details: serde_json::Value) -> Skipped {
    Skipped {
        item: item.to_string(),
        kind,
        code: code.to_string(),
        details,
    }
}

/// Renders `plan` at `out` (the plan's resolution scaled to `out`).
pub fn render(plan: &FramePlan, src: &mut dyn SourceProvider, out: Size) -> RenderOutput {
    let (w, h) = (out.width as usize, out.height as usize);
    let mut acc = vec![[0.0f32; 3]; w * h];
    let mut skips = Vec::new();
    for l in &plan.layers {
        if l.kind != ItemKind::Clip || l.children.is_some() {
            skips.push(skipped(
                &l.item,
                l.kind,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet" }),
            ));
            continue;
        }
        for e in &l.effects {
            skips.push(skipped(
                &l.item,
                l.kind,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet", "effect": e.id, "effectKind": e.kind }),
            ));
        }
        if let Some(m) = &l.mask {
            skips.push(skipped(
                &l.item,
                l.kind,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet", "mask": m.matte }),
            ));
        }
        if l.source.is_none() {
            skips.push(skipped(
                &l.item,
                l.kind,
                "LAYER_UNAVAILABLE",
                json!({ "reason": "noSource" }),
            ));
            continue;
        }
        match src.frame(l) {
            Ok(frame) => match ingest::readable(&frame) {
                Ok(()) => layer::draw(&mut acc, out, plan.resolution, l, &frame),
                Err(details) => skips.push(skipped(
                    &l.item,
                    l.kind,
                    "UNSUPPORTED_PIXEL_FORMAT",
                    details,
                )),
            },
            Err(issue) => skips.push(skipped(&l.item, l.kind, &issue.code, issue.details)),
        }
    }
    debug_assert!(
        acc.iter().all(|p| p.iter().all(|c| c.is_finite())),
        "the reference produced a non-finite value"
    );
    let mut rgba8 = Vec::with_capacity(w * h * 4);
    for p in &acc {
        for c in p {
            rgba8.push(to_u8(*c));
        }
        rgba8.push(255);
    }
    RenderOutput {
        size: out,
        rgba8,
        skipped: skips,
    }
}

/// Clamped to 0…1, scaled to 255, rounded half to even.
fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}
