//! Scaling a source picture to a proxy or thumbnail size (plan 6B-1 Task 14; spec §5.1's mip
//! chain): ingest into the working space (tone-mapped and gamut-converted as a layer is), the
//! exact 2×2 box mip chain down to level `⌊L⌋` with `L = log2(1/s)` for the larger axis scale
//! `s` (so no axis is ever shrunk below the output by the boxes), then bilinear to the output
//! size, then NV12 BT.709 limited range ([`crate::nv12`]). The CPU reference is
//! `reference::scale`; the GPU path (`Compositor::scale_nv12`) agrees with it within T1.

use fin_media::Size;

/// The mip level the boxes stop below: `max(0, log2(1/s))`, `s` the larger of the two axis
/// scales (output pixels per source pixel); 0 when nothing shrinks.
pub fn scale_level(src: Size, out: Size) -> f64 {
    let sx = out.width as f64 / src.width.max(1) as f64;
    let sy = out.height as f64 / src.height.max(1) as f64;
    let s = sx.max(sy);
    if !s.is_finite() || s <= 0.0 {
        return 0.0;
    }
    libm::log2(1.0 / s).max(0.0)
}

/// An 8-bit code of an R′G′B′ (or alpha) value: clamped to 0…1, scaled to 255, rounded half to
/// even (the CPU reference and the GPU readback of thumbnails agree on it).
pub fn code8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}

#[cfg(not(target_os = "wasi"))]
pub(crate) mod gpu;
