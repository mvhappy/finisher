//! Platform decoder layouts (sub-project 6B-1): NV12 (8-bit 4:2:0, Y then interleaved UV) and
//! P010 (10-bit 4:2:0 in little-endian 16-bit words, the value in the high 10 bits, Y then
//! interleaved UV), built from the 6A pattern's I420 planes. The reference reads NV12 exactly
//! as the I420 it came from and P010 with 8-bit-derived values within ±1 code of it (±8 for
//! full-range HDR, where scaling by 4 is inexact and the curves magnify it); the GPU
//! is within T1 of the reference for both (`FIN_GPU`, `FIN_REQUIRE_GPU` as in `gpu.rs`).

mod common;

use std::collections::HashMap;

use common::{Case, Patterns, cases};
use fin_compositor::compare::{passes, t1};
use fin_compositor::{LayerIssue, SourceFrame, SourceProvider, reference};
use fin_core::Rational;
use fin_core::eval::Layer;
use fin_media::{ColourTags, PixelFormat, Range, Rotation, Size, Transfer};

/// How P010 words are made from 8-bit codes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Depth {
    /// `code << 2 << 6`: the 8-bit value scaled to 10 bits.
    Scaled,
    /// Real 10-bit values: the scaled code plus a position-dependent 0…3 in the low two bits.
    Fine,
}

/// Bytes of padding past each row, so strides are not the row width.
const PAD: usize = 8;

struct Planes {
    y: Vec<u8>,
    uv: Vec<u8>,
    strides: [usize; 2],
}

fn word(code: u8, depth: Depth, x: usize, y: usize, k: usize) -> [u8; 2] {
    let fine = match depth {
        Depth::Scaled => 0,
        Depth::Fine => ((x * 7 + y * 3 + k) % 4) as u16,
    };
    (((u16::from(code) << 2) | fine) << 6).to_le_bytes()
}

/// `src` (I420) as NV12 (`depth` `None`) or P010.
fn convert(src: &SourceFrame, depth: Option<Depth>) -> Planes {
    let (w, h) = (src.size.width as usize, src.size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let bytes = if depth.is_some() { 2 } else { 1 };
    let (sy, suv) = (w * bytes + PAD, 2 * cw * bytes + PAD);
    let mut py = vec![0u8; sy * h];
    let mut puv = vec![0u8; suv * ch];
    for y in 0..h {
        for x in 0..w {
            let c = src.planes[0][y * src.strides[0] + x];
            match depth {
                None => py[y * sy + x] = c,
                Some(d) => py[y * sy + 2 * x..][..2].copy_from_slice(&word(c, d, x, y, 0)),
            }
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            for k in 0..2 {
                let c = src.planes[1 + k][y * src.strides[1 + k] + x];
                let at = y * suv + (2 * x + k) * bytes;
                match depth {
                    None => puv[at] = c,
                    Some(d) => puv[at..][..2].copy_from_slice(&word(c, d, x, y, 1 + k)),
                }
            }
        }
    }
    Planes {
        y: py,
        uv: puv,
        strides: [sy, suv],
    }
}

/// The pattern provider, its pictures converted to NV12 or P010.
struct Converted<'a> {
    inner: Patterns<'a>,
    depth: Option<Depth>,
    cache: HashMap<(String, u32), Planes>,
}

impl SourceProvider for Converted<'_> {
    fn frame(&mut self, l: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        let key = (
            l.source
                .as_ref()
                .map(|s| s.asset.clone())
                .unwrap_or_default(),
            self.inner.index(l).unwrap_or(0),
        );
        let depth = self.depth;
        let src = self.inner.frame(l)?;
        let (size, colour, hdr, rotation, pixel_aspect) = (
            src.size,
            src.colour,
            src.hdr,
            src.rotation,
            src.pixel_aspect,
        );
        if !self.cache.contains_key(&key) {
            let p = convert(&src, depth);
            self.cache.insert(key.clone(), p);
        }
        let p = &self.cache[&key];
        Ok(SourceFrame {
            format: if depth.is_some() {
                PixelFormat::P010
            } else {
                PixelFormat::Nv12
            },
            size,
            planes: [&p.y, &p.uv, &[]],
            strides: [p.strides[0], p.strides[1], 0],
            colour,
            hdr,
            rotation,
            pixel_aspect,
        })
    }
}

impl Case {
    fn converted(&self, depth: Option<Depth>) -> Converted<'_> {
        Converted {
            inner: self.patterns(),
            depth,
            cache: HashMap::new(),
        }
    }
}

fn max_abs(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn nv12_reads_as_the_i420_it_came_from_and_p010_within_one_code() {
    for case in cases() {
        let want = reference::render(&case.plan, &mut case.patterns(), case.size());
        let nv12 = reference::render(&case.plan, &mut case.converted(None), case.size());
        assert_eq!(nv12.skipped, want.skipped, "{}: nv12 skips", case.name);
        assert!(nv12.rgba8 == want.rgba8, "{}: nv12 equals i420", case.name);
        let p010 = reference::render(
            &case.plan,
            &mut case.converted(Some(Depth::Scaled)),
            case.size(),
        );
        assert_eq!(p010.skipped, want.skipped, "{}: p010 skips", case.name);
        let d = max_abs(&p010.rgba8, &want.rgba8);
        eprintln!("{:<32} p010 (scaled) vs i420 max {d}", case.name);
        // `code << 2` is exact for limited range but not for full range (4 · 255 = 1020 of
        // 1023, 0.3 % low at the top), which the HLG and PQ curves magnify near their peaks.
        let hdr_full = case.sources.values().any(|s| {
            s.colour.range == Range::Full
                && matches!(s.colour.transfer, Transfer::Pq | Transfer::Hlg)
        });
        let limit = if hdr_full { 8 } else { 1 };
        assert!(
            d <= limit,
            "{}: p010 within ±{limit} of i420, max {d}",
            case.name
        );
    }
}

#[test]
fn fine_p010_differs_from_scaled() {
    let case = cases()
        .into_iter()
        .find(|c| c.name == "single-clip")
        .unwrap();
    let a = reference::render(
        &case.plan,
        &mut case.converted(Some(Depth::Scaled)),
        case.size(),
    );
    let b = reference::render(
        &case.plan,
        &mut case.converted(Some(Depth::Fine)),
        case.size(),
    );
    assert!(a.skipped.is_empty() && b.skipped.is_empty());
    assert_ne!(a.rgba8, b.rgba8, "the low two bits are read");
}

/// One fixed two-plane picture.
struct Fixed {
    format: PixelFormat,
    planes: [Vec<u8>; 2],
    strides: [usize; 2],
}

impl SourceProvider for Fixed {
    fn frame(&mut self, _: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        Ok(SourceFrame {
            format: self.format,
            size: Size::new(320, 180),
            planes: [&self.planes[0], &self.planes[1], &[]],
            strides: [self.strides[0], self.strides[1], 0],
            colour: ColourTags::BT709,
            hdr: None,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
        })
    }
}

#[test]
fn short_or_misaligned_two_plane_pictures_are_unsupported() {
    let c = cases()
        .into_iter()
        .find(|c| c.name == "single-clip")
        .unwrap();
    let size = Size::new(320, 180);
    let ok = [
        (PixelFormat::Nv12, [320 * 180, 320 * 90], [320, 320]),
        (PixelFormat::P010, [640 * 180, 640 * 90], [640, 640]),
    ];
    let bad = [
        // NV12 UV rows are 2 × chroma width bytes.
        (PixelFormat::Nv12, [320 * 180, 160 * 90], [320, 160]),
        (PixelFormat::Nv12, [320 * 180, 320 * 89], [320, 320]),
        // P010 samples are two bytes.
        (PixelFormat::P010, [320 * 180, 640 * 90], [320, 640]),
        (PixelFormat::P010, [640 * 180, 320 * 90], [640, 320]),
        (PixelFormat::P010, [641 * 180, 640 * 90], [641, 640]),
        (PixelFormat::P010, [640 * 180, 0], [640, 640]),
    ];
    for (format, len, strides) in ok.into_iter().chain(bad) {
        let r = reference::render(
            &c.plan,
            &mut Fixed {
                format,
                planes: [vec![0; len[0]], vec![0; len[1]]],
                strides,
            },
            size,
        );
        let good = ok
            .iter()
            .any(|o| o.0 == format && o.1 == len && o.2 == strides);
        if good {
            assert!(
                r.skipped.is_empty(),
                "{format:?} {strides:?}: {:?}",
                r.skipped
            );
        } else {
            assert_eq!(r.skipped.len(), 1, "{format:?} {len:?} {strides:?}");
            assert_eq!(r.skipped[0].code, "UNSUPPORTED_PIXEL_FORMAT");
        }
    }
}

#[cfg(not(target_os = "wasi"))]
mod gpu {
    use super::*;
    use fin_compositor::{AdapterChoice, Compositor, Gpu};

    fn gpu() -> Option<Gpu> {
        let choice = AdapterChoice::from_env().expect("FIN_GPU");
        match Gpu::new(choice) {
            Ok(g) => Some(g),
            Err(e) => {
                assert!(
                    std::env::var("FIN_REQUIRE_GPU").as_deref() != Ok("1"),
                    "FIN_REQUIRE_GPU=1 but there is no adapter for {choice:?}: {e}"
                );
                eprintln!("notice: no GPU adapter for {choice:?}, skipping ({e})");
                None
            }
        }
    }

    #[test]
    fn nv12_and_p010_ingest_match_reference() {
        let Some(gpu) = gpu() else { return };
        eprintln!("gpu: {:?}", gpu.info());
        let mut c = Compositor::new(&gpu);
        let mut failed = Vec::new();
        for depth in [None, Some(Depth::Scaled), Some(Depth::Fine)] {
            let (mut worst, mut low) = (0u8, f64::INFINITY);
            for case in cases() {
                let want = reference::render(&case.plan, &mut case.converted(depth), case.size());
                let got = c
                    .render(&gpu, &case.plan, &mut case.converted(depth), case.size())
                    .unwrap();
                assert_eq!(got.skipped, want.skipped, "{} {depth:?}", case.name);
                let m = t1(&want.rgba8, &got.rgba8);
                worst = worst.max(m.max_abs);
                low = low.min(m.psnr);
                if !passes(&m) {
                    failed.push((case.name.clone(), depth, m));
                }
            }
            eprintln!("{depth:?}: max |Δ| {worst}, min PSNR {low:.2} dB");
        }
        assert!(failed.is_empty(), "outside T1: {failed:?}");
    }
}
