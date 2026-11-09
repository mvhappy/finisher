//! Gamut compression to Rec.709 (spec §10.1, owner decision 2026-11-09, "match professional
//! editors"): after a wide-gamut conversion (Display P3, BT.2020 SDR, tone-mapped HDR) the
//! colours inside Rec.709 are kept and only the saturated edge is compressed toward the gamut
//! boundary, with the curve of the ACES 1.3 Reference Gamut Compression (RGC).
//!
//! It works on display-encoded values: the matrix output, BT.1886-encoded with its sign kept
//! (`±|v|^(1/2.4)`, so out-of-gamut negatives stay negative). Per pixel the achromatic value
//! is `a = max(R′, G′, B′)` and each channel's distance from it is `d = (a − c) / a`: 0 on the
//! neutral axis, 1 on the Rec.709 boundary (the channel is 0), above 1 outside it. Distances up
//! to the channel's threshold `t` are kept exactly; above it,
//! `d' = t + s·x / (1 + x^p)^(1/p)` with `x = (d − t) / s`, `p` = 1.2 and `s` chosen so the
//! source gamut's furthest reach (its limit, [`reach`]) lands on 1. The curve is continuous
//! with slope 1 at `t`, strictly increasing and bounded (`d' < t + s`). The channel becomes
//! `a − d'·a`; `a` is never changed, so the brightest channel and (to first order) the hue
//! hold and only saturation moves; the source's own boundary lands on Rec.709's.
//!
//! Encoded rather than linear distance (ACES works in linear ACEScg): BT.1886 spreads values
//! near 0 over many code values, so a band in linear light would reach well inside Rec.709
//! (an orange with a fifth of its red in blue). The threshold leaves a band a tenth as wide as
//! the source's reach (`t = 1 − (limit − 1)/10`): Display P3 keeps every colour whose smallest
//! encoded channel is at least 2.5 % (G), 3.8 % (B) or 5.3 % (R) of its largest, BT.2020 at
//! least 3.6 %, 4.1 % or 8.0 %. Only the most saturated Rec.709 colours, on or next to its
//! boundary, move, by up to a few code values. A source that reaches nowhere (Rec.709 or the
//! BT.601 primaries, PQ or HLG with Rec.709 primaries) is not compressed. A smooth compression
//! cannot keep the whole of Rec.709 exactly (only a hard clip does, BT.2407 §2); this keeps all
//! but that edge. A channel above 1.0 (brighter than Rec.709 can show, such as Display P3's
//! red) is clipped at 1.0 afterwards, as Rec.709 SDR clips.
//!
//! Scalar `f32`, `libm` only (spec §13.4); `shaders/tonemap.wgsl` ports [`compress`] and
//! [`compress_distance`] line for line, with the [`Compression`] constants computed here.

use super::Primaries;

/// The RGC power: how sharply the curve bends toward its asymptote (ACES 1.3 uses 1.2).
pub const POWER: f32 = 1.2;

/// One source gamut's compression, per Rec.709 channel: threshold `t` and scale `s` (`s` is 0
/// where the source does not reach past Rec.709, which leaves the channel alone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compression {
    pub threshold: [f32; 3],
    pub scale: [f32; 3],
}

/// No compression: every channel kept.
pub const NONE: Compression = Compression {
    threshold: [1.0; 3],
    scale: [0.0; 3],
};

/// The band inside the boundary, as a fraction of the reach past it.
pub const BAND: f32 = 0.1;

/// How far a source gamut reaches outside Rec.709, per Rec.709 channel (R: toward cyan, G:
/// magenta, B: yellow), in the encoded distance above, over the source's unit cube (found on its
/// faces to 1e-6 and rounded up). 1 for gamuts inside Rec.709.
pub fn reach(p: Primaries) -> [f32; 3] {
    match p {
        // Display P3: green (R), a red-magenta (G), yellow (B).
        Primaries::DisplayP3 => [1.527_93, 1.245_41, 1.380_35],
        Primaries::Bt2020 => [1.804_84, 1.362_39, 1.408_41],
        _ => [1.0; 3],
    }
}

impl Compression {
    /// The constants for a source with primaries `p`.
    pub fn for_primaries(p: Primaries) -> Compression {
        let mut c = NONE;
        for (k, &limit) in reach(p).iter().enumerate() {
            if limit > 1.0 {
                let t = 1.0 - (limit - 1.0) * BAND;
                c.threshold[k] = t;
                c.scale[k] = scale(t, limit);
            }
        }
        c
    }
}

/// The RGC scale that maps `limit` to 1 above threshold `t`:
/// `s = (limit − t) / (((1 − t)/(limit − t))^(−p) − 1)^(1/p)`.
pub fn scale(t: f32, limit: f32) -> f32 {
    let w = limit - t;
    let r = libm::powf((1.0 - t) / w, -POWER) - 1.0;
    w / libm::powf(r, 1.0 / POWER)
}

/// One distance through the curve (`s` > 0 and `d` > `t`; otherwise `d`).
pub fn compress_distance(d: f32, t: f32, s: f32) -> f32 {
    if s <= 0.0 || d <= t {
        return d;
    }
    let x = (d - t) / s;
    t + s * x / libm::powf(1.0 + libm::powf(x, POWER), 1.0 / POWER)
}

/// Signed-encoded Rec.709 R′G′B′ ([`encode_signed`]) through the compression. A pixel with no
/// positive channel is returned as it is (black, or all negative, which clips to black).
pub fn compress(rgb: [f32; 3], c: &Compression) -> [f32; 3] {
    let a = rgb[0].max(rgb[1]).max(rgb[2]);
    if a <= 0.0 {
        return rgb;
    }
    let mut out = rgb;
    for k in 0..3 {
        let d = (a - rgb[k]) / a;
        if c.scale[k] > 0.0 && d > c.threshold[k] {
            out[k] = a - compress_distance(d, c.threshold[k], c.scale[k]) * a;
        }
    }
    out
}

/// The BT.1886 encode with the sign kept: `±|v|^(1/2.4)`, 0 at 0.
pub fn encode_signed(v: f32) -> f32 {
    if v > 0.0 {
        libm::powf(v, 1.0 / 2.4)
    } else if v < 0.0 {
        -libm::powf(-v, 1.0 / 2.4)
    } else {
        0.0
    }
}
