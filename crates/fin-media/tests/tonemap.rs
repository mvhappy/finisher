//! HDR tone mapping and the BT.2408 reference white (spec §10.1) against hand-computed values.
//!
//! The PQ constants (SMPTE ST 2084): m1 = 2610/16384 = 0.159 301 76, m2 = 2523/4096 × 128 =
//! 78.843 75, c1 = 3424/4096 = 0.835 937 5, c2 = 2413/4096 × 32 = 18.851 562 5,
//! c3 = 2392/4096 × 32 = 18.6875. PQ(L) = ((c1 + c2·Y^m1) / (1 + c3·Y^m1))^m2 with Y = L/10 000.

use fin_media::colour::convert::{IDENTITY, from_linear, gamut_to_709};
use fin_media::colour::softclip::NONE;
use fin_media::colour::tonemap::{
    Conversion, DEFAULT_PEAK, Eetf, HLG_PEAK, Hdr, SDR_PEAK, ToneMap, hlg_inverse_oetf, hlg_ootf,
    pq_eotf, pq_inverse_eotf, source_peak, to_sdr,
};
use fin_media::{
    ChromaSiting, ColourTags, ContentLight, HdrInfo, MasteringDisplay, Matrix, Primaries, Range,
    Transfer,
};

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

#[test]
fn pq_curve_matches_st2084() {
    // PQ(0) = c1^m2 = 0.835 937 5^78.843 75 = e^(78.843 75 × −0.179 2…) = 7.31e-7.
    assert!(close(pq_inverse_eotf(0.0), 7.309_56e-7, 1e-9));
    // 100 nits: Y^m1 = 0.01^0.159 30 = 0.480 08; (0.835 94 + 18.851 56 × 0.480 08) /
    // (1 + 18.6875 × 0.480 08) = 9.886 0 / 9.971 6 = 0.991 41; ^78.84 = 0.508 08.
    // (Within 5e-6: f32 through x^78.84 keeps about five and a half digits.)
    assert!(close(pq_inverse_eotf(100.0), 0.508_078, 5e-6));
    assert!(close(pq_inverse_eotf(1000.0), 0.751_827, 5e-6));
    assert!(close(pq_inverse_eotf(4000.0), 0.902_572, 5e-6));
    assert_eq!(pq_inverse_eotf(10_000.0), 1.0);
    assert_eq!(pq_eotf(1.0), 10_000.0);
    assert_eq!(pq_eotf(0.0), 0.0);
    assert_eq!(pq_eotf(-0.5), 0.0, "below the curve is black");
    for nits in [0.5f32, 10.0, 100.0, 1000.0, 4000.0] {
        let back = pq_eotf(pq_inverse_eotf(nits));
        assert!(close(back, nits, nits * 2e-4), "{nits} → {back}");
    }
}

/// BT.2390 §5.4 for a 1,000-nit and a 4,000-nit source on a 100-nit target (black 0 both):
/// `E1 = (PQ(L) − PQ(0)) / (PQ(Lw) − PQ(0))`, `maxLum = (PQ(100) − PQ(0)) / (PQ(Lw) − PQ(0))`,
/// `KS = 1.5 · maxLum − 0.5`; above KS, `T = (E1 − KS) / (1 − KS)` and
/// `E2 = (2T³ − 3T² + 1)·KS + (T³ − 2T² + T)·(1 − KS) + (−2T³ + 3T²)·maxLum`; the result is
/// `PQ⁻¹(E2 · (PQ(Lw) − PQ(0)) + PQ(0))`.
#[test]
fn eetf_matches_bt2390_table() {
    // Lw = 1000: range = 0.751 827 − 0.000 001 = 0.751 826; maxLum = 0.508 077 / 0.751 826 =
    // 0.675 791; KS = 1.5 × 0.675 791 − 0.5 = 0.513 687.
    let e = Eetf::new(1000.0, SDR_PEAK);
    assert!(close(e.range, 0.751_826, 5e-6));
    assert!(close(e.max_lum, 0.675_791, 5e-6));
    assert!(close(e.knee, 0.513_687, 8e-6));
    assert_eq!(e.min_lum, 0.0, "SDR black is 0: no black lift");
    // Below the knee the signal is kept: 10 nits has E1 = 0.299 698 / 0.751 826 = 0.398 627.
    assert!(close(e.apply(1.0), 1.0, 1e-4));
    assert!(close(e.apply(10.0), 10.0, 2e-3));
    // 100 nits: E1 = 0.508 077 / 0.751 826 = 0.675 791; T = (0.675 791 − 0.513 687) /
    // 0.486 313 = 0.333 333; E2 = (2/27 − 3/9 + 1) × 0.513 687 + (1/27 − 2/9 + 1/3) × 0.486 313
    // + (−2/27 + 3/9) × 0.675 791 = 0.380 509 + 0.072 046 + 0.175 205 = 0.627 760;
    // E4 = 0.627 760 × 0.751 826 + 0.000 001 = 0.471 968 → 69.454 nits.
    assert!(close(e.apply(100.0), 69.454, 0.02));
    // 400 nits: E1 = 0.867 990, T = 0.728 515, E2 = 0.672 549 → E4 = 0.505 641 → 97.598 nits.
    assert!(close(e.apply(400.0), 97.598, 0.03));
    // The source peak lands on the target peak (T = 1, E2 = maxLum), and brighter than the
    // declared peak clips there (E1 clamped to 1).
    assert!(close(e.apply(1000.0), 100.0, 0.02));
    assert!(close(e.apply(4000.0), 100.0, 0.02));

    // Lw = 4000: range = 0.902 572; maxLum = 0.562 922; KS = 0.344 383.
    let e = Eetf::new(4000.0, SDR_PEAK);
    assert!(close(e.max_lum, 0.562_922, 5e-6));
    assert!(close(e.knee, 0.344_383, 8e-6));
    assert!(close(e.apply(10.0), 10.0, 2e-3));
    // 100 nits: E1 = 0.562 922, T = 0.333 333 (again), E2 = 0.498 170 → 55.146 nits.
    assert!(close(e.apply(100.0), 55.146, 0.02));
    // 1000 nits: E1 = 0.832 982, T = 0.745 382, E2 = 0.559 309 → 96.800 nits.
    assert!(close(e.apply(1000.0), 96.800, 0.03));
    assert!(close(e.apply(4000.0), 100.0, 0.02));

    // A source no brighter than the target needs no roll-off (KS ≥ 1).
    let e = Eetf::new(SDR_PEAK, SDR_PEAK);
    assert!(e.knee >= 1.0);
    assert!(close(e.apply(80.0), 80.0, 0.02));

    // Monotonic over the whole PQ range.
    let e = Eetf::new(1000.0, SDR_PEAK);
    let mut last = 0.0;
    for i in 0..=4000 {
        let v = e.apply(i as f32 * 2.5);
        assert!(v >= last, "{} nits", i as f32 * 2.5);
        last = v;
    }
}

#[test]
fn hlg_ootf_at_1000_nits() {
    // Inverse OETF (BT.2100): E′ ≤ ½ gives E′²/3; above, (e^((E′ − c)/a) + b) / 12 with
    // a = 0.178 832 77, b = 1 − 4a = 0.284 668 92, c = ½ − a·ln 4a = 0.559 910 73.
    assert_eq!(hlg_inverse_oetf(0.0), 0.0);
    assert!(close(hlg_inverse_oetf(0.5), 1.0 / 12.0, 1e-7));
    // e^((1 − 0.559 910 73) / 0.178 832 77) = e^2.460 9 = 11.715 33; (11.715 33 + 0.284 67) / 12 = 1.
    assert!(close(hlg_inverse_oetf(1.0), 1.0, 1e-6));
    // E′ = 0.75: (0.75 − 0.559 911) / 0.178 833 = 1.062 94, e^1.062 94 = 2.894 88;
    // (2.894 88 + 0.284 67) / 12 = 0.264 963.
    assert!(close(hlg_inverse_oetf(0.75), 0.264_963, 1e-6));
    // OOTF at a 1,000-nit nominal peak: γ = 1.2, Fd = 1000 · Ys^0.2 · E with
    // Ys = 0.2627 R + 0.6780 G + 0.0593 B (scene linear).
    assert_eq!(HLG_PEAK, 1000.0);
    let white = hlg_ootf([1.0, 1.0, 1.0]);
    assert!(white.iter().all(|&v| close(v, 1000.0, 0.01)), "{white:?}");
    // Grey at E′ = ½: Ys = 1/12, 12^−0.2 = 0.608 364, × 1000 / 12 = 50.697 nits.
    let grey = hlg_ootf([1.0 / 12.0; 3]);
    assert!(grey.iter().all(|&v| close(v, 50.697, 0.005)), "{grey:?}");
    // Pure red: Ys = 0.2627, 0.2627^0.2 = 0.765 406 → 765.406 nits on R only.
    let red = hlg_ootf([1.0, 0.0, 0.0]);
    assert!(
        close(red[0], 765.406, 0.01) && red[1] == 0.0 && red[2] == 0.0,
        "{red:?}"
    );
    assert_eq!(hlg_ootf([0.0; 3]), [0.0; 3]);
}

#[test]
fn to_sdr_keeps_hue_and_lands_on_the_target() {
    let p = ToneMap {
        eetf: Eetf::new(1000.0, SDR_PEAK),
        target_peak: SDR_PEAK,
        gamut: gamut_to_709(Primaries::Bt2020).unwrap(),
    };
    // 1000-nit white → 100 nits → 1.0 relative: SDR white, kept by the gamut compression.
    let w = to_sdr([1000.0; 3], &p);
    assert!(w.iter().all(|&v| close(v, 1.0, 2e-5)), "{w:?}");
    assert_eq!(to_sdr([0.0; 3], &p), [0.0; 3]);
    // The roll-off scales all three channels by max(R, G, B)'s ratio: a dim grey below the knee
    // keeps its level (10 nits → 0.1 → 0.1^(1/2.4)).
    let g = to_sdr([10.0; 3], &p);
    assert!(g.iter().all(|&v| close(v, from_linear(0.1), 2e-4)), "{g:?}");
    // A 100-nit orange keeps its R:G ratio (69.454 / 100 for both channels; R ends at 0.694 54,
    // and no gamut to compress).
    let identity = ToneMap {
        gamut: IDENTITY,
        ..p
    };
    let o = to_sdr([100.0, 50.0, 0.0], &identity);
    let lin = |v: f32| libm::powf(v, 2.4);
    assert!(close(lin(o[0]), 0.694_54, 1e-3), "{o:?}");
    assert!(close(lin(o[1]) / lin(o[0]), 0.5, 1e-3), "{o:?}");
    assert_eq!(o[2], 0.0);
}

#[test]
fn conversion_follows_the_tags_and_metadata() {
    let tags = |primaries, transfer| ColourTags {
        primaries,
        transfer,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
        siting: ChromaSiting::TopLeft,
    };
    assert_eq!(
        Conversion::for_source(&ColourTags::BT709, None),
        Conversion::None
    );
    assert!(matches!(
        Conversion::for_source(&tags(Primaries::DisplayP3, Transfer::Bt709), None),
        Conversion::Gamut(_)
    ));
    let Conversion::Hdr(Hdr::Pq, p) =
        Conversion::for_source(&tags(Primaries::Bt2020, Transfer::Pq), None)
    else {
        panic!("PQ is tone-mapped")
    };
    assert_eq!(p.eetf, Eetf::new(DEFAULT_PEAK, SDR_PEAK));
    assert_eq!(Some(p.gamut), gamut_to_709(Primaries::Bt2020));
    let Conversion::Hdr(Hdr::Hlg, p) =
        Conversion::for_source(&tags(Primaries::Bt2020, Transfer::Hlg), None)
    else {
        panic!("HLG is tone-mapped")
    };
    assert_eq!(p.eetf, Eetf::new(HLG_PEAK, SDR_PEAK));

    // The source peak: clli's MaxCLL, else mdcv's maximum (0.0001 cd/m² units), else 1000,
    // kept within 100…10 000; HLG is always its 1,000-nit nominal peak.
    let mdcv = |nits: u32| MasteringDisplay {
        primaries: [[8500, 39850], [6550, 2300], [35400, 14600]],
        white_point: [15635, 16450],
        max_luminance: nits * 10_000,
        min_luminance: 50,
    };
    let info = |mastering, content_light| HdrInfo {
        mastering,
        content_light,
        dolby_vision: None,
    };
    assert_eq!(source_peak(Transfer::Pq, None), 1000.0);
    assert_eq!(
        source_peak(Transfer::Pq, Some(&info(Some(mdcv(4000)), None))),
        4000.0
    );
    let clli = ContentLight {
        max_cll: 1600,
        max_fall: 400,
    };
    assert_eq!(
        source_peak(Transfer::Pq, Some(&info(Some(mdcv(4000)), Some(clli)))),
        1600.0
    );
    let dark = ContentLight {
        max_cll: 40,
        max_fall: 10,
    };
    assert_eq!(
        source_peak(Transfer::Pq, Some(&info(None, Some(dark)))),
        SDR_PEAK
    );
    assert_eq!(
        source_peak(Transfer::Hlg, Some(&info(Some(mdcv(4000)), None))),
        1000.0
    );
    let Conversion::Hdr(_, p) = Conversion::for_source(
        &tags(Primaries::Bt2020, Transfer::Pq),
        Some(&info(None, Some(clli))),
    ) else {
        panic!()
    };
    assert_eq!(p.eetf, Eetf::new(1600.0, SDR_PEAK));
}

#[test]
fn hdr_signal_to_sdr_end_to_end() {
    // PQ code value 1.0 is 10 000 nits: past the 1000-nit peak, it lands on the target white.
    let c = Conversion::for_source(
        &ColourTags {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Pq,
            ..ColourTags::BT709
        },
        None,
    );
    let w = c.apply([1.0; 3]);
    let want = 1.0;
    assert!(w.iter().all(|&v| close(v, want, 2e-5)), "{w:?}");
    // HLG E′ = 1 white: 1000 nits from the OOTF, the same white.
    let c = Conversion::for_source(
        &ColourTags {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Hlg,
            ..ColourTags::BT709
        },
        None,
    );
    let w = c.apply([1.0; 3]);
    assert!(w.iter().all(|&v| close(v, want, 1e-4)), "{w:?}");
    // Darker signals are darker, all the way down (the EETF is flat at its peak, T = 1).
    let mut last = -1.0;
    for i in 0..=100 {
        let v = c.apply([i as f32 / 100.0; 3])[1];
        assert!(if i <= 95 { v > last } else { v >= last }, "{i}");
        last = v;
    }
}

/// HDR reference white (BT.2408: 203 cd/m², 75 % HLG, 58 % PQ) through the BT.2390 EETF on a
/// 1,000-nit source lands at 0.949 signal (242 of 255), BT.2446 Method C's "approximately 95 %
/// SDR signal"; a brighter master keeps more highlight range below it (4,000 nits: 0.875).
/// Neutrals are never touched by the gamut compression, so these are the EETF's alone.
#[test]
fn hdr_reference_white_lands_near_sdr_white() {
    let tags = |primaries, transfer| ColourTags {
        primaries,
        transfer,
        ..ColourTags::BT709
    };
    let code = |v: f32| (v * 255.0).round() as u8;
    // HLG 75 %: the inverse OETF gives 0.264 963 scene light, the OOTF 1000 · 0.264 963^1.2 =
    // 202.9 cd/m²; EETF(1000 → 100) of that is 88.24 cd/m², 0.8824^(1/2.4) = 0.9492.
    let hlg = Conversion::for_source(&tags(Primaries::Bt2020, Transfer::Hlg), None);
    let w = hlg.apply([0.75; 3]);
    assert!(w.iter().all(|&v| close(v, 0.949_2, 1e-3)), "{w:?}");
    assert_eq!(code(w[1]), 242);
    // PQ at 203 cd/m² (58 %), default 1,000-nit peak: the same level.
    let pq = Conversion::for_source(&tags(Primaries::Bt2020, Transfer::Pq), None);
    let w = pq.apply([pq_inverse_eotf(203.0); 3]);
    assert!(w.iter().all(|&v| close(v, 0.949_2, 1e-3)), "{w:?}");
    assert_eq!(code(w[1]), 242);
    // Rec.709 primaries: the same white, and no gamut compression at all.
    let Conversion::Hdr(_, p) = Conversion::for_source(&tags(Primaries::Bt709, Transfer::Pq), None)
    else {
        panic!()
    };
    assert_eq!(p.gamut.compression, NONE);
    assert!(close(to_sdr([203.0; 3], &p)[0], 0.949_2, 1e-3));
    // A 4,000-nit master (mdcv) keeps more highlight range below SDR white.
    let bright = HdrInfo {
        mastering: None,
        content_light: Some(ContentLight {
            max_cll: 4000,
            max_fall: 400,
        }),
        dolby_vision: None,
    };
    let pq4k = Conversion::for_source(&tags(Primaries::Bt2020, Transfer::Pq), Some(&bright));
    let w = pq4k.apply([pq_inverse_eotf(203.0); 3]);
    assert!(close(w[1], 0.875_2, 2e-3), "{w:?}");
    // The source peak reaches SDR white exactly.
    assert!(
        pq4k.apply([pq_inverse_eotf(4000.0); 3])
            .iter()
            .all(|&v| close(v, 1.0, 1e-4))
    );
}
