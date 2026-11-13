//! Scaling to proxy and thumbnail sizes, and NV12 out (plan 6B-1 Task 14): the CPU reference's
//! own properties (exact box levels, known codes, frame ids that survive), and the GPU path
//! within T1 of it for I420 and NV12 sources, SDR and HDR (`FIN_GPU`, `FIN_REQUIRE_GPU` as in
//! `gpu.rs`). NV12 planes are compared as grey RGBA images (each plane on its own).

use fin_compositor::nv12::{self, Nv12Frame};
use fin_compositor::reference::scale::{scale_nv12, scale_rgb};
use fin_compositor::{SourceFrame, scale_level};
use fin_core::Rational;
use fin_media::colour::{Primaries, Transfer};
use fin_media::testing::{Yuv420, pattern, read_id_y};
use fin_media::{ColourTags, PixelFormat, Rotation, Size};

fn source(p: &Yuv420, colour: ColourTags) -> SourceFrame<'_> {
    let cw = p.width.div_ceil(2) as usize;
    SourceFrame {
        format: PixelFormat::I420,
        size: Size::new(p.width, p.height),
        planes: [&p.y, &p.u, &p.v],
        strides: [p.width as usize, cw, cw],
        colour,
        hdr: None,
        rotation: Rotation::R0,
        pixel_aspect: Rational::from_int(1),
    }
}

fn flat(width: u32, height: u32, y: u8, u: u8, v: u8) -> Yuv420 {
    let (cw, ch) = (width.div_ceil(2) as usize, height.div_ceil(2) as usize);
    Yuv420 {
        width,
        height,
        y: vec![y; (width * height) as usize],
        u: vec![u; cw * ch],
        v: vec![v; cw * ch],
    }
}

const PQ: ColourTags = ColourTags {
    primaries: Primaries::Bt2020,
    transfer: Transfer::Pq,
    matrix: fin_media::colour::Matrix::Bt2020Ncl,
    range: fin_media::colour::Range::Limited,
    siting: fin_media::colour::ChromaSiting::Left,
};

#[test]
fn the_level_is_the_halvings_the_larger_scale_allows() {
    let l = |w, h, ow, oh| scale_level(Size::new(w, h), Size::new(ow, oh));
    assert_eq!(l(1920, 1080, 960, 540), 1.0);
    assert_eq!(l(3840, 2160, 960, 540), 2.0);
    assert_eq!(l(320, 180, 320, 180), 0.0);
    // Magnification reads level 0.
    assert_eq!(l(100, 100, 400, 400), 0.0);
    // The larger axis scale decides: no axis is boxed below the output.
    assert_eq!(l(4096, 2160, 960, 506).floor(), 2.0);
    assert_eq!(l(1920, 1080, 958, 540).floor(), 1.0);
}

#[test]
fn flat_pictures_keep_their_codes() {
    // Mid grey, black, white and an in-gamut colour survive ingest, scaling and NV12 within
    // one code.
    for (y, u, v) in [
        (128, 128, 128),
        (16, 128, 128),
        (235, 128, 128),
        (126, 100, 160),
    ] {
        let p = flat(64, 36, y, u, v);
        let out = scale_nv12(&source(&p, ColourTags::BT709), Size::new(32, 18));
        assert!(
            out.y.iter().all(|c| c.abs_diff(y) <= 1),
            "{y}: {:?}",
            &out.y[..4]
        );
        for pair in out.uv.chunks(2) {
            assert!(
                pair[0].abs_diff(u) <= 1 && pair[1].abs_diff(v) <= 1,
                "{u},{v}: {pair:?}"
            );
        }
        assert_eq!(out.uv.len(), 2 * 16 * 9);
    }
}

#[test]
fn an_exact_halving_is_the_box_average() {
    let p = pattern(3, 320, 180, 7);
    let src = source(&p, ColourTags::BT709);
    let full = scale_rgb(&src, Size::new(320, 180));
    let half = scale_rgb(&src, Size::new(160, 90));
    for y in 0..90 {
        for x in 0..160 {
            let at = |dx: usize, dy: usize| full[(2 * y + dy) * 320 + 2 * x + dx];
            let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
            for k in 0..3 {
                let want = ((a[k] + b[k]) + (c[k] + d[k])) * 0.25;
                assert!((half[y * 160 + x][k] - want).abs() < 1e-6, "({x},{y})");
            }
        }
    }
}

#[test]
fn frame_ids_survive_every_tier_scale() {
    for (w, h, ow, oh) in [
        (1920, 1080, 960, 540),
        (3840, 2160, 960, 540),
        (4096, 2160, 960, 506),
        (1280, 720, 960, 540),
        (1080, 1920, 540, 960),
        (320, 180, 320, 180),
    ] {
        let id = 0xABCDE;
        let p = pattern(id, w, h, 3);
        let out = scale_nv12(&source(&p, ColourTags::BT709), Size::new(ow, oh));
        assert_eq!(out.y.len(), (ow * oh) as usize);
        assert_eq!(read_id_y(&out.y, ow, oh), Some(id), "{w}x{h} → {ow}x{oh}");
    }
}

#[test]
fn hdr_sources_are_tone_mapped_before_nv12() {
    // PQ code 235 (10,000 nits) lands on SDR white, 16 on black: the ids still read.
    let p = pattern(42, 640, 360, 1);
    let out = scale_nv12(&source(&p, PQ), Size::new(320, 180));
    assert_eq!(read_id_y(&out.y, 320, 180), Some(42));
    // The same picture tagged BT.709 gives other codes: the mapping ran.
    let sdr = scale_nv12(&source(&p, ColourTags::BT709), Size::new(320, 180));
    assert_ne!(out.y, sdr.y);
}

#[test]
fn nv12_from_rgb_matches_the_definitions() {
    let size = Size::new(3, 3);
    let rgb: Vec<[f32; 3]> = (0..9)
        .map(|i| [i as f32 / 8.0, 1.0 - i as f32 / 8.0, 0.5])
        .collect();
    let f = nv12::from_rgb(&rgb, size);
    assert_eq!(f.chroma_size(), (2, 2));
    for (i, c) in rgb.iter().enumerate() {
        assert_eq!(f.y[i], nv12::y_code(*c));
    }
    // Chroma (1, 1) covers column 2 (its left neighbour 1, itself twice: clamped right) of row 2
    // (with itself: the odd last row).
    let get = |x: usize, y: usize| rgb[y * 3 + x];
    let want = nv12::uv_codes(nv12::chroma_rgb(get, 3, 3, 1, 1));
    assert_eq!((f.uv[6], f.uv[7]), want);
    // Out-of-range values are clamped before the matrix.
    assert_eq!(nv12::y_code([2.0, 2.0, 2.0]), 235);
    assert_eq!(nv12::y_code([-1.0, -1.0, -1.0]), 16);
    assert_eq!(nv12::uv_codes([1.0, 1.0, 1.0]), (128, 128));
}

/// A plane as a grey RGBA image, for T1.
fn grey(plane: &[u8]) -> Vec<u8> {
    plane.iter().flat_map(|&c| [c, c, c, 255]).collect()
}

fn as_nv12(p: &Yuv420) -> (Vec<u8>, Vec<u8>) {
    let uv = p.u.iter().zip(&p.v).flat_map(|(u, v)| [*u, *v]).collect();
    (p.y.clone(), uv)
}

#[cfg(not(target_os = "wasi"))]
mod gpu {
    use super::*;
    use fin_compositor::compare::{passes, t1};
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

    fn within_t1(name: &str, want: &Nv12Frame, got: &Nv12Frame) {
        assert_eq!(got.size, want.size, "{name}");
        for (plane, a, b) in [("y", &want.y, &got.y), ("uv", &want.uv, &got.uv)] {
            let m = t1(&grey(a), &grey(b));
            eprintln!(
                "{name} {plane}: max |Δ| {}, PSNR {:.1} dB",
                m.max_abs, m.psnr
            );
            assert!(passes(&m), "{name} {plane}: {m:?}");
        }
    }

    #[test]
    fn gpu_scale_nv12_within_t1() {
        let Some(gpu) = gpu() else { return };
        eprintln!("gpu: {:?}", gpu.info());
        let mut c = Compositor::new(&gpu);
        for (w, h, ow, oh, colour) in [
            (1920, 1080, 960, 540, ColourTags::BT709),
            (1280, 720, 960, 540, ColourTags::BT709),
            (4096, 2160, 960, 506, ColourTags::BT709),
            (1080, 1920, 540, 960, ColourTags::BT709),
            (321, 181, 320, 180, ColourTags::BT709),
            (640, 360, 320, 180, PQ),
        ] {
            let p = pattern(7, w, h, 11);
            let src = source(&p, colour);
            let name = format!("{w}x{h}→{ow}x{oh} {:?}", colour.transfer);
            let want = scale_nv12(&src, Size::new(ow, oh));
            let got = c.scale_nv12(&gpu, &src, Size::new(ow, oh)).unwrap();
            within_t1(&name, &want, &got);
            assert_eq!(read_id_y(&got.y, ow, oh), Some(7), "{name}");
            // NV12 in gives the same as the I420 it came from.
            let (y, uv) = as_nv12(&p);
            let cw = w.div_ceil(2) as usize;
            let nv = SourceFrame {
                format: PixelFormat::Nv12,
                planes: [&y, &uv, &[]],
                strides: [w as usize, 2 * cw, 0],
                ..src
            };
            let from_nv12 = c.scale_nv12(&gpu, &nv, Size::new(ow, oh)).unwrap();
            assert_eq!(from_nv12, got, "{name} nv12");
        }
        // A picture the compositor cannot read is refused, not drawn.
        let p = flat(64, 64, 128, 128, 128);
        let bad = SourceFrame {
            format: PixelFormat::I444,
            ..source(&p, ColourTags::BT709)
        };
        let e = c.scale_nv12(&gpu, &bad, Size::new(32, 32)).unwrap_err();
        assert_eq!(e.code, "UNSUPPORTED_PIXEL_FORMAT");
    }
}
