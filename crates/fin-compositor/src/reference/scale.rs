//! Scaling for proxies and thumbnails (plan 6B-1 Task 14), the oracle of `Compositor::scale_nv12`:
//! ingest (the source's matrix, range, siting, gamut and HDR tone mapping, as for a layer), the
//! box mip chain down to level `⌊L⌋` ([`crate::scale::scale_level`]), bilinear from that level
//! to the output size (texel centres, clamped to the edge), then NV12 ([`crate::nv12`]).

use fin_media::Size;

use super::ingest::ingest;
use super::mipchain::chain;
use crate::nv12::{self, Nv12Frame};
use crate::scale::scale_level;
use crate::source::{SourceFrame, readable};

/// The R′G′B′ of `src` scaled to `out`, row-major. Panics on a source a compositor cannot read
/// (`readable`), as the GPU path refuses it with `UNSUPPORTED_PIXEL_FORMAT`.
pub fn scale_rgb(src: &SourceFrame, out: Size) -> Vec<[f32; 3]> {
    readable(src).expect("a readable source");
    let level = scale_level(src.size, out).floor();
    let levels = chain(ingest(src), level);
    let img = &levels[(level as usize).min(levels.len() - 1)];
    let (w, h) = (out.width as usize, out.height as usize);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let v = (y as f64 + 0.5) / h as f64;
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64;
            px.push(img.sample(u, v));
        }
    }
    px
}

/// `src` scaled to `out` as NV12.
pub fn scale_nv12(src: &SourceFrame, out: Size) -> Nv12Frame {
    nv12::from_rgb(&scale_rgb(src, out), out)
}
