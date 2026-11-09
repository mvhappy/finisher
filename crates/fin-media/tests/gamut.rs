//! Gamut compression to Rec.709 (spec §10.1, owner decision 2026-11-09, "match professional
//! editors"): colours Rec.709 can show pass through unchanged, only the saturated edge is
//! compressed toward the boundary (ACES 1.3 RGC's curve), smoothly and monotonically.

use fin_media::Primaries;
use fin_media::colour::convert::{
    Gamut, IDENTITY, convert_gamut, from_linear, gamut_to_709, linear_to_709, primaries_to_709,
    to_linear,
};
use fin_media::colour::softclip::{
    BAND, NONE, POWER, compress, compress_distance, encode_signed, reach, scale,
};

const WIDE: [Primaries; 2] = [Primaries::DisplayP3, Primaries::Bt2020];

fn gamut(p: Primaries) -> Gamut {
    gamut_to_709(p).unwrap()
}

/// 8-bit full-range code values of a display-encoded value.
fn code(v: [f32; 3]) -> [u8; 3] {
    v.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The inverse of a 3×3 matrix (f64, for building test colours).
fn inverse(m: [[f32; 3]; 3]) -> [[f64; 3]; 3] {
    let a = m.map(|r| r.map(f64::from));
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let mut inv = [[0.0; 3]; 3];
    for (i, row) in inv.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            let (r0, r1) = ((j + 1) % 3, (j + 2) % 3);
            let (c0, c1) = ((i + 1) % 3, (i + 2) % 3);
            *v = (a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0]) / det;
        }
    }
    inv
}

/// A display-encoded Rec.709 colour as the same colour in primaries `p` (display-encoded).
fn in_primaries(rec709: [f32; 3], p: Primaries) -> [f32; 3] {
    let inv = inverse(primaries_to_709(p).unwrap());
    let l = rec709.map(|v| f64::from(to_linear(v)));
    let mut out = [0.0f32; 3];
    for (k, row) in inv.iter().enumerate() {
        let v = row[0] * l[0] + row[1] * l[1] + row[2] * l[2];
        out[k] = from_linear(v as f32);
    }
    out
}

#[test]
fn wide_gamut_white_is_rec709_white() {
    for p in WIDE {
        let w = convert_gamut([1.0; 3], &gamut(p));
        assert_eq!(code(w), [255; 3], "{p:?} white: {w:?}");
        assert!(w.iter().all(|&v| (v - 1.0).abs() < 1e-6), "{p:?}: {w:?}");
        let g = convert_gamut([0.5; 3], &gamut(p));
        assert!(
            g.iter().all(|&v| (v - 0.5).abs() < 1e-6),
            "{p:?} grey: {g:?}"
        );
    }
    assert_eq!(convert_gamut([0.0; 3], &gamut(Primaries::Bt2020)), [0.0; 3]);
}

/// Every colour Rec.709 can show, sent in a wider gamut, comes back as it was, to the 8-bit
/// code value, unless it lies in the narrow band next to Rec.709's boundary that makes room for
/// the compressed edge (`d` above the channel's threshold).
#[test]
fn rec709_representable_colours_pass_through() {
    // Skin, sky, foliage, a saturated-but-inside orange, greys.
    let samples = [
        [0.85, 0.65, 0.55],
        [0.40, 0.60, 0.85],
        [0.25, 0.45, 0.20],
        [0.95, 0.55, 0.20],
        [0.20, 0.20, 0.20],
        [0.72, 0.72, 0.72],
    ];
    for p in WIDE {
        let g = gamut(p);
        for c in samples {
            let src = in_primaries(c, p);
            let out = convert_gamut(src, &g);
            assert_eq!(code(out), code(c), "{p:?} {c:?} → {out:?}");
            assert!(
                (0..3).all(|k| (out[k] - c[k]).abs() < 2e-5),
                "{p:?} {c:?} → {out:?}"
            );
        }
        // A dense sweep of the protected core: every Rec.709 colour whose distances stay at or
        // under the thresholds is unchanged at 8 bits.
        let t = g.compression.threshold;
        let mut checked = 0;
        for r in 0..=17 {
            for gg in 0..=17 {
                for b in 0..=17 {
                    let c = [r as f32 / 17.0, gg as f32 / 17.0, b as f32 / 17.0];
                    let a = c[0].max(c[1]).max(c[2]);
                    if a <= 0.0 || (0..3).any(|k| (a - c[k]) / a > t[k] - 1e-3) {
                        continue;
                    }
                    let out = convert_gamut(in_primaries(c, p), &g);
                    assert_eq!(code(out), code(c), "{p:?} {c:?} → {out:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 3000, "{p:?}: {checked}");
        // On the boundary itself (Rec.709's primaries and secondaries, sent in the wider
        // gamut) the zero channels lift by a few code values at most (BT.2020's red: 8.7).
        for c in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0],
        ] {
            let out = code(convert_gamut(in_primaries(c, p), &g));
            let want = code(c);
            let worst = (0..3).map(|k| out[k].abs_diff(want[k])).max().unwrap();
            assert!(worst <= 9, "{p:?} {c:?} → {out:?}");
        }
    }
}

#[test]
fn constants_follow_the_source_reach() {
    // Rec.709 itself and the BT.601 primaries reach nowhere: no compression at all.
    assert_eq!(reach(Primaries::Bt709), [1.0; 3]);
    assert!(gamut_to_709(Primaries::Bt709).is_none());
    assert_eq!(IDENTITY.compression, NONE);
    for p in WIDE {
        let c = gamut(p).compression;
        for (k, &lim) in reach(p).iter().enumerate() {
            // t = 1 − (limit − 1)/10: Display P3 R 0.947 207, G 0.975 459, B 0.961 965;
            // BT.2020 R 0.919 516, G 0.963 761, B 0.959 159.
            assert!((c.threshold[k] - (1.0 - (lim - 1.0) * BAND)).abs() < 1e-6);
            assert_eq!(c.scale[k], scale(c.threshold[k], lim));
            // The limit lands on the boundary.
            let at = compress_distance(lim, c.threshold[k], c.scale[k]);
            assert!((at - 1.0).abs() < 1e-5, "{p:?} channel {k}: {at}");
        }
    }
    assert_eq!(BAND, 0.1);
    let c = gamut(Primaries::DisplayP3).compression;
    assert!((c.threshold[1] - 0.975_459).abs() < 1e-6);
    let c = gamut(Primaries::Bt2020).compression;
    assert!((c.threshold[0] - 0.919_516).abs() < 1e-6);
}

/// Each source gamut's edge lands inside Rec.709: no channel negative (nothing left for the
/// encode to clip), the furthest colours on the boundary, the brightest channel kept.
#[test]
fn out_of_gamut_primaries_land_inside() {
    let corners = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
    ];
    for p in WIDE {
        let g = gamut(p);
        for c in corners {
            let l = c.map(to_linear);
            let m = g.matrix;
            let v: [f32; 3] =
                std::array::from_fn(|k| m[k][0] * l[0] + m[k][1] * l[1] + m[k][2] * l[2]);
            let out = compress(v.map(encode_signed), &g.compression);
            assert!(
                out.iter().all(|&x| x >= -1e-6),
                "{p:?} {c:?}: {v:?} → {out:?}"
            );
            let a = encode_signed(v[0].max(v[1]).max(v[2]));
            assert_eq!(
                out[0].max(out[1]).max(out[2]),
                a,
                "{p:?} {c:?}: the max moved"
            );
            // Saturation is kept as far as it fits: the compressed colour is still clearly
            // coloured (some channel well below the max).
            assert!(out.iter().any(|&x| x < 0.5 * a), "{p:?} {c:?}: {out:?}");
        }
    }
    // BT.2020 green reaches furthest past Rec.709's red side of the gamut and is held at the
    // boundary, not clipped: R lands just above 0 where 6A's hard clip gave 0 at a lower hue.
    let g = gamut(Primaries::Bt2020);
    let out = convert_gamut([0.0, 1.0, 0.0], &g);
    assert!(out[0] > 0.0 && out[2] > 0.0, "{out:?}");
}

#[test]
fn compression_curve_is_continuous_and_monotonic() {
    for p in WIDE {
        let c = gamut(p).compression;
        for k in 0..3 {
            let (t, s) = (c.threshold[k], c.scale[k]);
            assert_eq!(compress_distance(t, t, s), t);
            // Slope 1 at the threshold.
            let h = 1e-3f32;
            assert!(((compress_distance(t + h, t, s) - t) / h - 1.0).abs() < 2e-2);
            let mut last = compress_distance(0.0, t, s);
            let mut d = 0.0f32;
            while d < 4.0 {
                let next = d + 1.0 / 4096.0;
                let v = compress_distance(next, t, s);
                assert!(v >= last, "{p:?} {k}: decreasing at {next}");
                assert!(
                    v - last <= (next - d) * 1.000_1,
                    "{p:?} {k}: steeper than 1"
                );
                assert!(v < t + s, "{p:?} {k}: past the asymptote");
                if next > t && next <= reach(p)[k] {
                    assert!(v > last, "{p:?} {k}: flat at {next}");
                }
                last = v;
                d = next;
            }
        }
    }
    assert_eq!(POWER, 1.2);
    // A saturation ramp from grey to BT.2020's pure green: every output channel moves one way.
    let g = gamut(Primaries::Bt2020);
    let mut prev = convert_gamut([0.5, 0.5, 0.5], &g);
    for i in 1..=512 {
        let f = i as f32 / 512.0;
        let o = convert_gamut([0.5 * (1.0 - f), 0.5 + 0.5 * f, 0.5 * (1.0 - f)], &g);
        assert!(
            o[0] <= prev[0] + 1e-6 && o[2] <= prev[2] + 1e-6,
            "{f}: {o:?} {prev:?}"
        );
        assert!(o[1] >= prev[1] - 1e-6, "{f}: {o:?}");
        prev = o;
    }
}

#[test]
fn identity_gamut_changes_nothing() {
    for c in [
        [0.3, 0.6, 0.9],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.9, 0.9, 0.1],
    ] {
        let out = linear_to_709(c, &IDENTITY);
        assert_eq!(out, c.map(from_linear), "{c:?}");
    }
    // Above 1.0 a channel clips at the encode (the HDR path keeps max(R, G, B) ≤ 1 anyway).
    assert_eq!(linear_to_709([1.5, 0.2, 0.0], &IDENTITY)[0], 1.0);
}

/// The hard-coded reach is each gamut's true furthest encoded distance: a sweep of the three
/// faces of the source's unit cube (where `max` = 1; the distance does not change with scale)
/// never exceeds it and comes within the grid's 1e-3 of it. Most maxima lie inside a face, not at a
/// corner (Display P3's G at about (0.955, 0, 1)).
#[test]
fn reach_bounds_every_source_colour() {
    const N: u32 = 200;
    for p in WIDE {
        let m = primaries_to_709(p).unwrap();
        let mut worst = [0.0f32; 3];
        for face in 0..3 {
            for i in 0..=N {
                for j in 0..=N {
                    let mut s = [0.0f32; 3];
                    let (u, w) = ((face + 1) % 3, (face + 2) % 3);
                    s[face] = 1.0;
                    s[u] = i as f32 / N as f32;
                    s[w] = j as f32 / N as f32;
                    let l = s.map(to_linear);
                    let e: [f32; 3] = std::array::from_fn(|k| {
                        encode_signed(m[k][0] * l[0] + m[k][1] * l[1] + m[k][2] * l[2])
                    });
                    let a = e[0].max(e[1]).max(e[2]);
                    for k in 0..3 {
                        worst[k] = worst[k].max((a - e[k]) / a);
                    }
                }
            }
        }
        let r = reach(p);
        for k in 0..3 {
            assert!(
                worst[k] <= r[k],
                "{p:?} channel {k}: {} > {}",
                worst[k],
                r[k]
            );
            assert!(
                r[k] - worst[k] < 1e-4,
                "{p:?} channel {k}: {} vs {}",
                worst[k],
                r[k]
            );
        }
    }
}
