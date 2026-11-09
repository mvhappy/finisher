//! Ingest (spec §5.1 step 2): Y′CbCr code values to display-encoded R′G′B′ `f32`: bilinear
//! chroma upsampling at the source's siting, range expansion, the source matrix, and the
//! conversion to the Rec.709 SDR working space: the wide-gamut matrix and soft clip, or the HDR
//! tone mapping (`fin_media::colour::tonemap`, spec §10.1).

use fin_media::colour::convert::decode;
use fin_media::colour::tonemap::Conversion;
use fin_media::{ChromaSiting, PixelFormat};

use super::mipchain::Image;
use crate::source::SourceFrame;

/// One component of a plane: sample `(x, y)` is the `step`-th part at `offset` of each texel
/// (1, 0 for a planar plane; 2, 0 or 2, 1 for U or V of interleaved UV), one byte or (`wide`)
/// a P010 word whose 10-bit value is the high bits.
#[derive(Clone, Copy)]
struct Plane<'a> {
    data: &'a [u8],
    stride: usize,
    step: usize,
    offset: usize,
    wide: bool,
}

impl Plane<'_> {
    fn code(&self, x: usize, y: usize) -> f32 {
        let i = x * self.step + self.offset;
        if self.wide {
            let at = y * self.stride + 2 * i;
            (u16::from_le_bytes([self.data[at], self.data[at + 1]]) >> 6) as f32
        } else {
            self.data[y * self.stride + i] as f32
        }
    }
}

fn at(plane: &Plane, w: usize, h: usize, x: isize, y: isize) -> f32 {
    let x = x.clamp(0, w as isize - 1) as usize;
    let y = y.clamp(0, h as isize - 1) as usize;
    plane.code(x, y)
}

/// Bilinear at chroma coordinates `(fx, fy)`, clamped to the edge.
fn bilinear(plane: &Plane, w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let a = at(plane, w, h, x0, y0);
    let b = at(plane, w, h, x0 + 1, y0);
    let c = at(plane, w, h, x0, y0 + 1);
    let d = at(plane, w, h, x0 + 1, y0 + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

/// The Y, Cb and Cr planes of `src` and their bit depth.
fn planes<'a>(src: &SourceFrame<'a>) -> ([Plane<'a>; 3], u32) {
    let plane = |k: usize, step, offset, wide| Plane {
        data: src.planes[k],
        stride: src.strides[k],
        step,
        offset,
        wide,
    };
    match src.format {
        PixelFormat::Nv12 => (
            [
                plane(0, 1, 0, false),
                plane(1, 2, 0, false),
                plane(1, 2, 1, false),
            ],
            8,
        ),
        PixelFormat::P010 => (
            [
                plane(0, 1, 0, true),
                plane(1, 2, 0, true),
                plane(1, 2, 1, true),
            ],
            10,
        ),
        _ => (
            [
                plane(0, 1, 0, false),
                plane(1, 1, 0, false),
                plane(2, 1, 0, false),
            ],
            8,
        ),
    }
}

/// The source as R′G′B′ in its coded orientation; `source::readable` has checked it.
pub(crate) fn ingest(src: &SourceFrame) -> Image {
    let (w, h) = (src.size.width as usize, src.size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let tags = src.colour;
    // Chroma sample k sits at luma coordinate 2k + offset.
    let (ox, oy) = match tags.siting {
        ChromaSiting::Center => (0.5, 0.5),
        ChromaSiting::TopLeft => (0.0, 0.0),
        ChromaSiting::Left | ChromaSiting::Unknown => (0.0, 0.5),
    };
    let conversion = Conversion::for_source(&tags, src.hdr.as_ref());
    let ([py, pu, pv], bits) = planes(src);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let fy = (y as f32 - oy) / 2.0;
        for x in 0..w {
            let fx = (x as f32 - ox) / 2.0;
            let yv = py.code(x, y);
            let cb = bilinear(&pu, cw, ch, fx, fy);
            let cr = bilinear(&pv, cw, ch, fx, fy);
            let rgb = decode(yv, cb, cr, tags.range, bits, tags.matrix);
            px.push(conversion.apply(rgb));
        }
    }
    Image {
        width: w,
        height: h,
        px,
    }
}
