//! Blend modes on the GPU (spec §5.1 step 6). `normal`, `screen` and `add` are fixed-function
//! blend states on premultiplied colour, fused with the layer pass: `normal` src + dst·(1 − αs),
//! `screen` src + dst·(1 − src), `add` src + dst; the frame's alpha stays 1. `multiply`,
//! `overlay`, `darken` and `lighten`, and every mode under a dissolve (which needs the backdrop
//! in linear light), go through `blend.wgsl` and a ping-pong target. A crossfade folds its
//! weight into the layer's alpha: `dst + w·(blended − dst)` is the blend at alpha `w·a`.

use fin_core::BlendMode;

/// The modes by [`index`].
pub(crate) const MODES: [BlendMode; 7] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Add,
    BlendMode::Darken,
    BlendMode::Lighten,
];

/// The `MODE` override of `blend.wgsl`.
pub(crate) fn index(m: BlendMode) -> u32 {
    match m {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Overlay => 3,
        BlendMode::Add => 4,
        BlendMode::Darken => 5,
        BlendMode::Lighten => 6,
    }
}

/// The fixed-function blend state of a fused mode; `None` for the shader modes.
pub(crate) fn fused(m: BlendMode) -> Option<wgpu::BlendState> {
    use wgpu::BlendFactor as F;
    let dst = match m {
        BlendMode::Normal => F::OneMinusSrcAlpha,
        BlendMode::Screen => F::OneMinusSrc,
        BlendMode::Add => F::One,
        _ => return None,
    };
    Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: F::One,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        },
        // The frame is opaque: its alpha is kept.
        alpha: wgpu::BlendComponent {
            src_factor: F::Zero,
            dst_factor: F::One,
            operation: wgpu::BlendOperation::Add,
        },
    })
}
