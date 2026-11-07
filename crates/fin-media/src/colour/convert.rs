//! Colour conversion shared by both compositors (spec §5.1 ingest, §10.1): range expansion,
//! the BT.601/709/2020 Y′CbCr matrices, BT.1886 transfer, and the wide-gamut SDR primaries
//! conversion to the Rec.709 working space. Scalar `f32`, `libm` for powers (spec §13.4).

use super::{Matrix, Primaries, Range};

/// `(Kr, Kb)` of a Y′CbCr matrix; `None` for identity (GBR). Unknown reads as BT.709 (callers
/// resolve untagged sources first, `ColourTags::resolve_defaults`).
pub fn luma_coefficients(m: Matrix) -> Option<(f32, f32)> {
    match m {
        Matrix::Bt601 => Some((0.299, 0.114)),
        Matrix::Bt2020Ncl => Some((0.2627, 0.0593)),
        Matrix::Identity => None,
        Matrix::Bt709 | Matrix::Unknown => Some((0.2126, 0.0722)),
    }
}

/// Code values (`bits` deep) to Y′ in 0…1 and Pb, Pr in −½…½. Limited range is
/// 16–235 / 16–240 at 8 bits (scaled for more bits); full range is 0…2ⁿ−1 with chroma centred
/// at 2ⁿ⁻¹.
pub fn expand(y: f32, cb: f32, cr: f32, range: Range, bits: u32) -> (f32, f32, f32) {
    let scale = (1u32 << (bits - 8)) as f32;
    let mid = 128.0 * scale;
    match range {
        Range::Limited => (
            (y - 16.0 * scale) / (219.0 * scale),
            (cb - mid) / (224.0 * scale),
            (cr - mid) / (224.0 * scale),
        ),
        Range::Full => {
            let max = ((1u32 << bits) - 1) as f32;
            (y / max, (cb - mid) / max, (cr - mid) / max)
        }
    }
}

/// Y′PbPr to R′G′B′ (display-encoded) with matrix `m`.
pub fn to_rgb(y: f32, pb: f32, pr: f32, m: Matrix) -> [f32; 3] {
    match luma_coefficients(m) {
        None => [pr + 0.5, y, pb + 0.5],
        Some((kr, kb)) => {
            let r = y + 2.0 * (1.0 - kr) * pr;
            let b = y + 2.0 * (1.0 - kb) * pb;
            let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
            [r, g, b]
        }
    }
}

/// BT.1886 (γ 2.4, black 0) display-encoded value to linear light; negative inputs give 0.
pub fn to_linear(v: f32) -> f32 {
    if v > 0.0 { libm::powf(v, 2.4) } else { 0.0 }
}

/// Linear light to a BT.1886 display-encoded value; negative inputs give 0.
pub fn from_linear(v: f32) -> f32 {
    if v > 0.0 {
        libm::powf(v, 1.0 / 2.4)
    } else {
        0.0
    }
}

/// The linear-light 3×3 (row-major) from `p` to Rec.709 primaries, for the wide-gamut SDR
/// sources spec §10.1 converts (BT.2020, Display P3); `None` when no conversion applies
/// (Rec.709, the BT.601 primaries, which 6A treats as Rec.709, and unknown).
pub fn primaries_to_709(p: Primaries) -> Option<[[f32; 3]; 3]> {
    match p {
        Primaries::Bt2020 => Some([
            [1.660_491, -0.587_641, -0.072_850],
            [-0.124_550, 1.132_9, -0.008_349],
            [-0.018_151, -0.100_579, 1.118_73],
        ]),
        Primaries::DisplayP3 => Some([
            [1.224_94, -0.224_940, 0.0],
            [-0.042_057, 1.042_057, 0.0],
            [-0.019_638, -0.078_636, 1.098_274],
        ]),
        _ => None,
    }
}

/// A display-encoded R′G′B′ in primaries `m`'s gamut to Rec.709: linearised, converted,
/// clipped to 0…1 (6A clips; the soft clip of spec §10.1 is 6B's), re-encoded.
pub fn convert_gamut(rgb: [f32; 3], m: &[[f32; 3]; 3]) -> [f32; 3] {
    let l = [to_linear(rgb[0]), to_linear(rgb[1]), to_linear(rgb[2])];
    let mut out = [0.0f32; 3];
    for (k, row) in m.iter().enumerate() {
        let v = row[0] * l[0] + row[1] * l[1] + row[2] * l[2];
        out[k] = from_linear(v.clamp(0.0, 1.0));
    }
    out
}
