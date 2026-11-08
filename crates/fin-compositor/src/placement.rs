//! Where one clip layer lands in the output (spec §5.1), shared by both compositors so they take
//! the same samples: the cropped quad, the inverse matrix, the explicit sampling level, the
//! pixels it may touch and the transition weight. All `f64` (`f32` for the weight), computed
//! once per layer in Rust; the GPU path only converts them to `f32` uniforms.

use fin_core::eval::Layer;
use fin_core::{Curve, Rational, Resolution};
use fin_media::{Rotation, Size, display_size};

use crate::geometry::{base_box, crop_quad, invert, layer_matrix};
use crate::mips::mip_level;

/// One layer's geometry in output pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The cropped item's corners in output pixels, in order around the quad.
    pub quad: [[f64; 2]; 4],
    /// Output pixels to coded-normalised source coordinates.
    pub inverse: [f64; 9],
    /// The explicit sampling level `L` (0 for magnification).
    pub level: f64,
    /// The output pixels the layer may touch, `[x0, y0, x1, y1)`, clipped to the output (empty
    /// when the layer is off-frame): the quad's bounds widened by one pixel.
    pub bounds: [i64; 4],
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

/// The placement of clip layer `l` whose source is `coded` pixels with display `rotation` and
/// `pixel_aspect`; `None` when the crop leaves nothing or the matrix is singular.
pub fn place(
    l: &Layer,
    coded: Size,
    rotation: Rotation,
    pixel_aspect: Rational,
    frame: Resolution,
    out: Size,
) -> Option<Placement> {
    let display = display_size(coded, rotation, pixel_aspect);
    let base = base_box(display, frame);
    let mut quad = crop_quad(&l.transform, base, frame, &l.crop)?;
    let (sx, sy) = (
        out.width as f64 / frame.width.max(1) as f64,
        out.height as f64 / frame.height.max(1) as f64,
    );
    for p in &mut quad {
        *p = [p[0] * sx, p[1] * sy];
    }
    let mut m = layer_matrix(&l.transform, base, frame, rotation);
    scale_to(out, frame, &mut m);
    let inverse = invert(&m)?;
    let level = mip_level(&m, coded);
    let (w, h) = (out.width as i64, out.height as i64);
    let lo = |k: usize| {
        quad.iter()
            .map(|p| p[k])
            .fold(f64::INFINITY, f64::min)
            .floor() as i64
            - 1
    };
    let hi = |k: usize| {
        quad.iter()
            .map(|p| p[k])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil() as i64
            + 1
    };
    let bounds = [lo(0).max(0), lo(1).max(0), hi(0).min(w), hi(1).min(h)];
    Some(Placement {
        quad,
        inverse,
        level,
        bounds,
    })
}

impl Placement {
    /// Whether no output pixel can be touched.
    pub fn is_empty(&self) -> bool {
        self.bounds[0] >= self.bounds[2] || self.bounds[1] >= self.bounds[3]
    }
}

/// A transition's incoming layer weight `curve(progress)`: linear, or equal power (`sqrt`).
pub fn weight(curve: Curve, progress: f64) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    (match curve {
        Curve::Linear => p,
        Curve::EqualPower => p.sqrt(),
    }) as f32
}
