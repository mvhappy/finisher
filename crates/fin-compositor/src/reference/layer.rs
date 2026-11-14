//! One clip layer (spec §5.1 steps 4–8): crop, matrix, explicit-level sample, coverage,
//! opacity, blend, transition weight.

use fin_core::Resolution;
use fin_core::TransitionKind;
use fin_core::eval::{Layer, Role};
use fin_media::Size;

use super::blend::{blend, mix_encoded, mix_linear};
use super::ingest::ingest;
use super::mipchain::{chain, sample};
use crate::coverage::coverage;
use crate::geometry::transform_point;
use crate::placement::{place, weight};
use crate::source::SourceFrame;

pub(crate) fn draw(
    acc: &mut [[f32; 3]],
    out: Size,
    frame: Resolution,
    l: &Layer,
    src: &SourceFrame,
) {
    let Some(p) = place(l, src.size, src.rotation, src.pixel_aspect, frame, out) else {
        return;
    };
    let levels = chain(ingest(src), p.level);
    let opacity = l.opacity.clamp(0.0, 1.0) as f32;
    let incoming = l.transition.as_ref().filter(|t| t.role == Role::In);
    let w = out.width as i64;
    let [x0, y0, x1, y1] = p.bounds;
    for y in y0..y1 {
        for x in x0..x1 {
            let centre = [x as f64 + 0.5, y as f64 + 0.5];
            let cov = coverage(&p.quad, centre) as f32;
            if cov == 0.0 {
                continue;
            }
            let [u, v] = transform_point(&p.inverse, centre);
            let cs = sample(&levels, p.level, u, v);
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
