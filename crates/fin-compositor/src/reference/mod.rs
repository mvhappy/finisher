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

use std::convert::Infallible;

use fin_core::eval::FramePlan;
use fin_media::Size;

use crate::output::RenderOutput;
use crate::source::{SourceProvider, walk};

/// Renders `plan` at `out` (the plan's resolution scaled to `out`).
pub fn render(plan: &FramePlan, src: &mut dyn SourceProvider, out: Size) -> RenderOutput {
    let (w, h) = (out.width as usize, out.height as usize);
    let mut acc = vec![[0.0f32; 3]; w * h];
    let skipped = walk(plan, src, |l, frame| {
        layer::draw(&mut acc, out, plan.resolution, l, frame);
        Ok::<(), Infallible>(())
    })
    .unwrap_or_else(|never| match never {});
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
        skipped,
    }
}

/// Clamped to 0…1, scaled to 255, rounded half to even.
fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}
