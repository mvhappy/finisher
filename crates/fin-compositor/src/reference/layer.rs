//! One clip layer (spec §5.1 steps 4–8): crop, matrix, explicit-level sample, coverage,
//! opacity, blend, transition weight.

use fin_core::eval::{Layer, Role};
use fin_core::{Curve, Resolution, TransitionKind};
use fin_media::{Size, display_size};

use super::blend::{blend, mix_encoded, mix_linear};
use super::ingest::ingest;
use super::mipchain::{chain, sample};
use crate::coverage::coverage;
use crate::geometry::{base_box, crop_quad, invert, layer_matrix, transform_point};
use crate::mips::mip_level;
use crate::source::SourceFrame;

/// The incoming layer's weight `curve(progress)`: linear, or equal power (`sqrt`).
fn weight(curve: Curve, progress: f64) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    (match curve {
        Curve::Linear => p,
        Curve::EqualPower => p.sqrt(),
    }) as f32
}

/// Frame (plan resolution) to output pixels.
fn scale_to(out: Size, frame: Resolution, m: &mut [f64; 9]) {
    let (sx, sy) = (
        out.width as f64 / frame.width.max(1) as f64,
        out.height as f64 / frame.height.max(1) as f64,
    );
    for c in 0..3 {
        m[c] *= sx;
        m[3 + c] *= sy;
    }
}

pub(crate) fn draw(
    acc: &mut [[f32; 3]],
    out: Size,
    frame: Resolution,
    l: &Layer,
    src: &SourceFrame,
) {
    let display = display_size(src.size, src.rotation, src.pixel_aspect);
    let base = base_box(display, frame);
    let Some(mut quad) = crop_quad(&l.transform, base, frame, &l.crop) else {
        return;
    };
    let (sx, sy) = (
        out.width as f64 / frame.width.max(1) as f64,
        out.height as f64 / frame.height.max(1) as f64,
    );
    for p in &mut quad {
        *p = [p[0] * sx, p[1] * sy];
    }
    let mut m = layer_matrix(&l.transform, base, frame, src.rotation);
    scale_to(out, frame, &mut m);
    let Some(inv) = invert(&m) else { return };
    let level = mip_level(&m, src.size);
    let levels = chain(ingest(src), level);
    let opacity = l.opacity.clamp(0.0, 1.0) as f32;
    let incoming = l.transition.as_ref().filter(|t| t.role == Role::In);
    let (w, h) = (out.width as i64, out.height as i64);
    let min_x = quad
        .iter()
        .map(|p| p[0])
        .fold(f64::INFINITY, f64::min)
        .floor() as i64
        - 1;
    let max_x = quad
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil() as i64
        + 1;
    let min_y = quad
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min)
        .floor() as i64
        - 1;
    let max_y = quad
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil() as i64
        + 1;
    for y in min_y.max(0)..max_y.min(h) {
        for x in min_x.max(0)..max_x.min(w) {
            let centre = [x as f64 + 0.5, y as f64 + 0.5];
            let cov = coverage(&quad, centre) as f32;
            if cov == 0.0 {
                continue;
            }
            let [u, v] = transform_point(&inv, centre);
            let cs = sample(&levels, level, u, v);
            let a = cov * opacity;
            let i = (y * w + x) as usize;
            let dst = acc[i];
            let blended = blend(l.blend, dst, cs, a);
            acc[i] = match incoming {
                None => blended,
                Some(t) => {
                    let wt = weight(t.curve, t.progress);
                    match t.kind {
                        TransitionKind::Crossfade => mix_encoded(dst, blended, wt),
                        TransitionKind::Dissolve => mix_linear(dst, blended, wt),
                    }
                }
            };
        }
    }
}
