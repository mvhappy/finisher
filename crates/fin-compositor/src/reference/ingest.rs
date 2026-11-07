//! Ingest (spec §5.1 step 2): Y′CbCr code values to display-encoded R′G′B′ `f32`: bilinear
//! chroma upsampling at the source's siting, range expansion, the source matrix, and the
//! wide-gamut conversion to Rec.709 (spec §10.1).

use fin_media::colour::convert::{convert_gamut, expand, primaries_to_709, to_rgb};
use fin_media::{ChromaSiting, PixelFormat};

use super::mipchain::Image;
use crate::source::SourceFrame;

fn at(plane: &[u8], stride: usize, w: usize, h: usize, x: isize, y: isize) -> f32 {
    let x = x.clamp(0, w as isize - 1) as usize;
    let y = y.clamp(0, h as isize - 1) as usize;
    plane[y * stride + x] as f32
}

/// Bilinear at chroma coordinates `(fx, fy)`, clamped to the edge.
fn bilinear(plane: &[u8], stride: usize, w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let a = at(plane, stride, w, h, x0, y0);
    let b = at(plane, stride, w, h, x0 + 1, y0);
    let c = at(plane, stride, w, h, x0, y0 + 1);
    let d = at(plane, stride, w, h, x0 + 1, y0 + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

/// The source as R′G′B′ in its coded orientation. 6A reads 8-bit 4:2:0 only (others are
/// refused by the provider).
pub(crate) fn ingest(src: &SourceFrame) -> Image {
    debug_assert_eq!(src.format, PixelFormat::I420);
    let (w, h) = (src.size.width as usize, src.size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let tags = src.colour;
    // Chroma sample k sits at luma coordinate 2k + offset.
    let (ox, oy) = match tags.siting {
        ChromaSiting::Center => (0.5, 0.5),
        ChromaSiting::TopLeft => (0.0, 0.0),
        ChromaSiting::Left | ChromaSiting::Unknown => (0.0, 0.5),
    };
    let gamut = primaries_to_709(tags.primaries);
    let [py, pu, pv] = src.planes;
    let [sy, su, sv] = src.strides;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let fy = (y as f32 - oy) / 2.0;
        for x in 0..w {
            let fx = (x as f32 - ox) / 2.0;
            let yv = py[y * sy + x] as f32;
            let cb = bilinear(pu, su, cw, ch, fx, fy);
            let cr = bilinear(pv, sv, cw, ch, fx, fy);
            let (l, pb, pr) = expand(yv, cb, cr, tags.range, 8);
            let rgb = to_rgb(l, pb, pr, tags.matrix);
            px.push(match &gamut {
                Some(m) => convert_gamut(rgb, m),
                None => rgb,
            });
        }
    }
    Image {
        width: w,
        height: h,
        px,
    }
}
