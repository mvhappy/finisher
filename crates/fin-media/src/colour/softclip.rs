//! The gamut soft clip (spec §10.1): linear-light Rec.709 channels after a wide-gamut
//! conversion (Display P3, BT.2020 SDR, tone-mapped HDR) roll off smoothly above [`KNEE`]
//! toward 1.0 instead of being hard-clipped (6A clipped to 0…1).
//!
//! Above the knee, with `t = (x − k) / (1 − k)`, the channel is `k + (1 − k) · t / √(1 + t²)`
//! (computed as `k + (1 − k) / √(1 + 1/t²)`):
//! closed form, equal to `x` with slope 1 and curvature 0 at the knee (C²), strictly
//! increasing, and below 1.0 over the matrix range (`x` up to ≈ 1.66; past `x` ≈ 360 the
//! f32 result rounds to 1.0). In-gamut highlights compress too: 1.0 maps to
//! `0.8 + 0.2 / √2 ≈ 0.9414`, BT.2020 pure red (1.6605 after the matrix) to ≈ 0.9948. Values
//! below the knee, negatives included, are unchanged: the roll-off is one-sided, and the
//! BT.1886 encode still hard-clips negatives (out-of-gamut colours below 0) to 0.
//! Scalar `f32`, `libm` only (spec §13.4); `shaders/tonemap.wgsl` ports it line for line.

/// Where the roll-off starts, per channel.
pub const KNEE: f32 = 0.8;

/// One linear-light channel through the soft clip.
pub fn softclip(x: f32) -> f32 {
    if x <= KNEE {
        return x;
    }
    // t / √(1 + t²) written as 1 / √(1 + 1/t²): a chain of correctly rounded steps, each
    // monotonic in `x`, so the f32 result never decreases (t appearing twice would let rounding
    // step backwards once the quotient nears 1).
    let t = (x - KNEE) / (1.0 - KNEE);
    KNEE + (1.0 - KNEE) / libm::sqrtf(1.0 + 1.0 / (t * t))
}
