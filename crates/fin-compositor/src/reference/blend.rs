//! Blend modes over an opaque backdrop (W3C Compositing, separable modes), and the transition
//! mixes. `cb` is the backdrop, `cs` the source's straight colour, `a` its alpha.

use fin_core::BlendMode;
use fin_media::colour::convert::{from_linear, to_linear};

fn mode(m: BlendMode, cb: f32, cs: f32) -> f32 {
    match m {
        BlendMode::Normal => cs,
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => {
            if cb <= 0.5 {
                2.0 * cb * cs
            } else {
                1.0 - 2.0 * (1.0 - cb) * (1.0 - cs)
            }
        }
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Add => cb + cs,
    }
}

/// `cb·(1 − a) + a·B(cb, cs)` per channel. `add` is `cb + a·cs` (no clamp until the output).
pub(crate) fn blend(m: BlendMode, dst: [f32; 3], cs: [f32; 3], a: f32) -> [f32; 3] {
    [0, 1, 2].map(|k| match m {
        BlendMode::Add => dst[k] + a * cs[k],
        _ => dst[k] * (1.0 - a) + a * mode(m, dst[k], cs[k]),
    })
}

/// A crossfade's mix: `dst + w·(blended − dst)` in display-encoded values.
pub(crate) fn mix_encoded(dst: [f32; 3], blended: [f32; 3], w: f32) -> [f32; 3] {
    [0, 1, 2].map(|k| dst[k] + w * (blended[k] - dst[k]))
}

/// A dissolve's mix: the same in linear light (BT.1886), re-encoded.
pub(crate) fn mix_linear(dst: [f32; 3], blended: [f32; 3], w: f32) -> [f32; 3] {
    [0, 1, 2].map(|k| {
        let (d, b) = (to_linear(dst[k]), to_linear(blended[k]));
        from_linear(d + w * (b - d))
    })
}
