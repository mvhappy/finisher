//! Where a source lands in the frame (spec §5.1), in `f64`, `libm` for `sin`/`cos`; shared by
//! both compositors so they take the same samples.
//!
//! - **Base box:** the source's display size fitted inside the frame, centred ("contain").
//! - **Transform:** the item is the base box with item-normalised coordinates `(u, v)` in
//!   0…1. Its `anchor` point is placed at the frame centre plus `position` (pixels), and the
//!   item is scaled by `scale` and rotated `rotation` degrees clockwise about that point. The
//!   defaults (anchor ½, ½) leave the base box where it is.
//! - **Matrices** are row-major 3×3 affine, frame pixels with the origin top-left, y down.

use fin_core::Resolution;
use fin_core::eval::{PlanCrop, PlanTransform};
use fin_media::{Rotation, Size};

/// A rectangle in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF64 {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Spec §5.1: the source's display size fitted inside the frame, centred ("contain").
pub fn base_box(display: Size, frame: Resolution) -> RectF64 {
    let (dw, dh) = (display.width.max(1) as f64, display.height.max(1) as f64);
    let (fw, fh) = (frame.width as f64, frame.height as f64);
    let s = (fw / dw).min(fh / dh);
    let (w, h) = (dw * s, dh * s);
    RectF64 {
        x: (fw - w) / 2.0,
        y: (fh - h) / 2.0,
        width: w,
        height: h,
    }
}

fn mul(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// `(sin, cos)` of `degrees`, exact at multiples of 90°.
fn sin_cos(degrees: f64) -> (f64, f64) {
    let quarter = degrees / 90.0;
    if quarter == quarter.floor() {
        return match (quarter as i64).rem_euclid(4) {
            0 => (0.0, 1.0),
            1 => (1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (-1.0, 0.0),
        };
    }
    let rad = degrees * (core::f64::consts::PI / 180.0);
    (libm::sin(rad), libm::cos(rad))
}

/// The map from coded-normalised source coordinates to display-normalised ones for a
/// clockwise display rotation.
fn orientation(r: Rotation) -> [f64; 9] {
    match r {
        Rotation::R0 => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        // Coded (uc, vc) shows at display (1 − vc, uc).
        Rotation::R90 => [0.0, -1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        Rotation::R180 => [-1.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 1.0],
        Rotation::R270 => [0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 1.0],
    }
}

/// Source → frame matrix: coded-normalised source coordinates (0…1 over the coded picture) to
/// frame pixels, through the display `rotation`, the base box and the transform.
pub fn layer_matrix(
    t: &PlanTransform,
    base: RectF64,
    frame: Resolution,
    rotation: Rotation,
) -> [f64; 9] {
    let (bw, bh) = (base.width, base.height);
    let (s, c) = sin_cos(t.rotation);
    let (sx, sy) = (t.scale[0], t.scale[1]);
    // R · S · diag(bw, bh): display-normalised (u, v) to pixels about the anchor.
    let (a, b) = (c * sx * bw, -s * sy * bh);
    let (d, e) = (s * sx * bw, c * sy * bh);
    let (ax, ay) = (t.anchor[0], t.anchor[1]);
    let (px, py) = (
        frame.width as f64 / 2.0 + t.position[0],
        frame.height as f64 / 2.0 + t.position[1],
    );
    let display = [
        a,
        b,
        px - (a * ax + b * ay),
        d,
        e,
        py - (d * ax + e * ay),
        0.0,
        0.0,
        1.0,
    ];
    mul(&display, &orientation(rotation))
}

pub fn transform_point(m: &[f64; 9], p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2],
        m[3] * p[0] + m[4] * p[1] + m[5],
    ]
}

/// The inverse of an affine matrix; `None` when it is singular (a zero scale).
pub fn invert(m: &[f64; 9]) -> Option<[f64; 9]> {
    let det = m[0] * m[4] - m[1] * m[3];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    Some([
        a,
        b,
        -(a * m[2] + b * m[5]),
        c,
        d,
        -(c * m[2] + d * m[5]),
        0.0,
        0.0,
        1.0,
    ])
}

/// The frame-pixel corners of the item after its crop (fractions of the base box cut from each
/// side), in order around the quad; `None` when the crop leaves nothing.
pub fn crop_quad(
    t: &PlanTransform,
    base: RectF64,
    frame: Resolution,
    crop: &PlanCrop,
) -> Option<[[f64; 2]; 4]> {
    let (u0, u1) = (crop.left, 1.0 - crop.right);
    let (v0, v1) = (crop.top, 1.0 - crop.bottom);
    if u1 <= u0 || v1 <= v0 {
        return None;
    }
    let m = layer_matrix(t, base, frame, Rotation::R0);
    Some([[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|p| transform_point(&m, p)))
}
