//! The sampling level (spec §5.1): `L = max(0, log2(1 / s))` for the layer's smaller axis
//! scale `s` in output pixels per source pixel (constant per layer: transforms are affine).

use fin_media::Size;

/// `matrix` maps coded-normalised source coordinates to frame pixels; `source` is the coded
/// size. 0 for magnification.
pub fn mip_level(matrix: &[f64; 9], source: Size) -> f64 {
    let (w, h) = (source.width.max(1) as f64, source.height.max(1) as f64);
    let sx = (matrix[0] * matrix[0] + matrix[3] * matrix[3]).sqrt() / w;
    let sy = (matrix[1] * matrix[1] + matrix[4] * matrix[4]).sqrt() / h;
    let s = sx.min(sy);
    if s.is_nan() || s <= 0.0 || !s.is_finite() {
        return 0.0;
    }
    libm::log2(1.0 / s).max(0.0)
}
