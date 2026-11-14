//! The EQ (plan 6C Task 8; Decisions 13, 15, 16): eight RBJ biquad bands designed in `f64`,
//! run as transposed direct form II in `f32`, coefficients recomputed every 64 samples (on a
//! grid anchored at the item's first sample) from the parameters at the block's first sample.

use std::collections::BTreeMap;

use fin_audio::channels::Canonical;
use fin_audio::effects::biquad::{BandKind, Biquad, design};
use fin_audio::effects::params::ParamsAt;
use fin_audio::effects::{BLOCK, Chain, SkippedEffect};
use fin_audio::testing::{analyse::*, signals::*, source::MemSource};
use fin_audio::{Mixer, PcmSource, RATE, SourceLayout, Stereo, sample_at};
use fin_core::eval::{AudioEffectPlan, AudioItem, AudioPlan, GainSegment, PanSegment};
use fin_core::eval::{ParamSegment, PlanValue};
use fin_core::{Interp, Rational};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn s(n: i64) -> Rational {
    Rational::from_int(n)
}

fn constant(from: Rational, to: Rational, v: f64) -> ParamSegment {
    ParamSegment {
        from,
        to,
        v: [v, v],
        interp: Interp::Linear,
        u: [0.0, 1.0],
    }
}

/// One EQ band: `(type, freq, gain, q)`.
type Band = (&'static str, f64, f64, f64);

/// An `eq` over `[from, to)` with `bands` as bands 1…n (the rest off), every number constant.
fn eq(from: Rational, to: Rational, bands: &[Band]) -> AudioEffectPlan {
    let mut numbers = BTreeMap::new();
    let mut values = BTreeMap::new();
    let defaults = [80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10_000.0];
    for b in 1..=8 {
        let (ty, f, g, q) =
            bands
                .get(b - 1)
                .copied()
                .unwrap_or(("off", defaults[b - 1], 0.0, 0.707));
        values.insert(format!("band{b}Type"), PlanValue::Text(ty.to_string()));
        numbers.insert(format!("band{b}Freq"), vec![constant(from, to, f)]);
        numbers.insert(format!("band{b}Gain"), vec![constant(from, to, g)]);
        numbers.insert(format!("band{b}Q"), vec![constant(from, to, q)]);
    }
    AudioEffectPlan {
        id: "fx1".to_string(),
        kind: "eq".to_string(),
        numbers,
        values,
    }
}

/// An item at 0 dB, centre, playing `asset` from source time 0 at speed 1.
fn item(asset: &str, from: Rational, to: Rational, effects: Vec<AudioEffectPlan>) -> AudioItem {
    AudioItem {
        item: "i1".to_string(),
        track: "t1".to_string(),
        via: vec![],
        asset: asset.to_string(),
        from,
        to,
        source_from: s(0),
        speed: s(1),
        gain: vec![GainSegment {
            from,
            to,
            db: [0.0, 0.0],
            interp: Interp::Linear,
            u: [0.0, 1.0],
            shapes: vec![],
        }],
        pan: vec![PanSegment {
            from,
            to,
            pan: [0.0, 0.0],
            interp: Interp::Linear,
            u: [0.0, 1.0],
        }],
        effects,
    }
}

/// Runs `x` (mono, starting at the item's first sample) through `item`'s chain in calls of
/// `block` samples.
fn chain_mono(it: &AudioItem, x: &[f32], block: usize) -> Vec<f32> {
    let mut chain = Chain::new(it, SourceLayout::Mono).unwrap();
    let start = sample_at(it.from).unwrap();
    let mut out = Vec::with_capacity(x.len());
    for (k, piece) in x.chunks(block).enumerate() {
        let mut buf = Canonical::Mono(piece.to_vec());
        chain.process(start + (k * block) as i64, &mut buf);
        let Canonical::Mono(y) = buf else {
            panic!("the chain changed the layout")
        };
        out.extend_from_slice(&y);
    }
    out
}

/// The band's gain in dB at `hz`: a sine through one band, Goertzel over the last `measure`
/// seconds (whole periods) against the same window of the input.
fn band_gain(band: Band, hz: f64, secs: f64, measure: f64) -> f64 {
    let x = sine(hz, -12.0, secs, RATE);
    let it = item(
        "a",
        s(0),
        r((secs * 1000.0) as i64, 1000),
        vec![eq(s(0), s(10), &[band])],
    );
    let y = chain_mono(&it, &x, 1_000);
    let tail = x.len() - (measure * f64::from(RATE)) as usize;
    goertzel_db(&y[tail..], hz, RATE) - goertzel_db(&x[tail..], hz, RATE)
}

#[test]
fn peak_band_gain_at_centre() {
    let band = ("peak", 1_000.0, 6.0, 1.0);
    let centre = band_gain(band, 1_000.0, 1.5, 1.0);
    assert!(
        (centre - 6.0).abs() <= 0.05,
        "{centre} dB at 1 kHz (+6 ± 0.05)"
    );
    for hz in [100.0, 10_000.0] {
        let g = band_gain(band, hz, 1.5, 1.0);
        assert!(g.abs() <= 0.3, "{g} dB at {hz} Hz (0 ± 0.3)");
    }
}

#[test]
fn low_shelf_and_high_shelf_levels() {
    // Shelf slope 1: the shelf level a decade and more away from the corner, the flat side 0.
    let low = ("lowShelf", 200.0, 12.0, 0.707);
    let high = ("highShelf", 2_000.0, -12.0, 0.707);
    let cases = [
        (low, 20.0, 12.0),
        (low, 20_000.0, 0.0),
        (high, 20_000.0, -12.0),
        (high, 20.0, 0.0),
    ];
    for (band, hz, want) in cases {
        let g = band_gain(band, hz, 3.0, 2.0);
        assert!(
            (g - want).abs() <= 0.1,
            "{} at {hz} Hz: {g} dB ({want} ± 0.1)",
            band.0
        );
    }
}

#[test]
fn low_pass_minus_3db_at_cutoff() {
    let g = band_gain(("lowPass", 1_000.0, 0.0, 0.707), 1_000.0, 1.5, 1.0);
    assert!(
        (g + 3.0103).abs() <= 0.05,
        "{g} dB at the cutoff (−3.01 ± 0.05)"
    );
    let hp = band_gain(("highPass", 1_000.0, 0.0, 0.707), 1_000.0, 1.5, 1.0);
    assert!(
        (hp + 3.0103).abs() <= 0.05,
        "high pass: {hp} dB at the cutoff (−3.01 ± 0.05)"
    );
}

#[test]
fn high_pass_rejects_dc() {
    let x = vec![0.5f32; 3 * 48_000];
    let it = item(
        "a",
        s(0),
        s(3),
        vec![eq(s(0), s(3), &[("highPass", 80.0, 0.0, 0.707)])],
    );
    let y = chain_mono(&it, &x, 1_000);
    // The last second: below −100 dB relative to the 0.5 DC in.
    let worst = y[2 * 48_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        f64::from(worst) <= 0.5 * amp(-100.0),
        "DC leaks at {worst} (≤ 0.5 × −100 dB)"
    );
}

fn sources(asset: &str, c: Canonical) -> BTreeMap<String, Box<dyn PcmSource>> {
    BTreeMap::from([(asset.to_string(), MemSource::boxed(c))])
}

fn render(p: &AudioPlan, c: Canonical, frames: usize, block: usize) -> (Stereo, Mixer) {
    let mut m = Mixer::new(p, sources("a", c)).unwrap();
    let mut out = Stereo::default();
    while out.len() < frames {
        let mut b = Stereo::silence(block.min(frames - out.len()));
        m.render(&mut b).unwrap();
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
    }
    (out, m)
}

fn plan(from: Rational, to: Rational, items: Vec<AudioItem>) -> AudioPlan {
    AudioPlan {
        timeline: "tl1".to_string(),
        from,
        to,
        items,
    }
}

#[test]
fn all_bands_off_is_bit_exact_passthrough() {
    let (l, rr) = (pink(1, 1.0, RATE), pink(2, 1.0, RATE));
    let src = Canonical::Stereo(Stereo {
        l: l.clone(),
        r: rr.clone(),
    });
    // Every band off, whatever its numbers say.
    let mut fx = eq(s(0), s(1), &[]);
    for b in 1..=8 {
        fx.numbers
            .insert(format!("band{b}Gain"), vec![constant(s(0), s(1), 18.0)]);
    }
    let p = plan(s(0), s(1), vec![item("a", s(0), s(1), vec![fx])]);
    let (out, m) = render(&p, src, 48_000, 1_000);
    assert!(
        out.l == l && out.r == rr,
        "an all-off EQ changed the signal"
    );
    assert!(m.skipped_effects().is_empty());
}

/// A peak band at 1 kHz, Q 2, whose gain ramps linearly from −12 to +12 dB over the item.
fn ramped(from: Rational, to: Rational) -> AudioEffectPlan {
    let mut fx = eq(from, to, &[("peak", 1_000.0, 0.0, 2.0)]);
    fx.numbers.insert(
        "band1Gain".to_string(),
        vec![ParamSegment {
            from,
            to,
            v: [-12.0, 12.0],
            interp: Interp::Linear,
            u: [0.0, 1.0],
        }],
    );
    fx
}

#[test]
fn keyframed_gain_moves_once_per_64_samples() {
    // The item starts off the timeline's 64-grid: 1001/30000 s is sample 1602.
    let (from, to) = (r(1001, 30_000), r(31_001, 30_000));
    let it = item("a", from, to, vec![ramped(from, to)]);
    let (start, end) = (sample_at(from).unwrap(), sample_at(to).unwrap());
    assert_eq!(start, 1_602);
    let x = noise(9, 1.0, RATE);
    // Calls of 37 samples, never aligned with the 64-sample blocks.
    let got = chain_mono(&it, &x, 37);

    // The reference: one coefficient set per 64-sample block from the item's first sample,
    // designed from the parameters at the block's first sample.
    let params = ParamsAt::new(&it.effects[0], start, end).unwrap();
    let reference = |anchor: i64| {
        let mut st = Biquad::default();
        let mut y = x.clone();
        let mut i = 0usize;
        while i < y.len() {
            let n = start + i as i64;
            let stop = (i as i64 + BLOCK - (n - anchor).rem_euclid(BLOCK)) as usize;
            let stop = stop.min(y.len());
            let at = n - (n - anchor).rem_euclid(BLOCK);
            let c = design(
                BandKind::Peak,
                params.number("band1Freq", at, 0.0),
                params.number("band1Gain", at, 0.0),
                params.number("band1Q", at, 0.0),
                f64::from(RATE),
            );
            st.run(&c, &mut y[i..stop]);
            i = stop;
        }
        y
    };
    assert!(
        got == reference(start),
        "coefficients left the item's 64-sample grid"
    );
    // The ramp really moves within a block: anchoring the grid at timeline 0 differs.
    assert!(got != reference(0));
    // And the ramp is there: −12 dB at the start, +12 dB at the end.
    assert_eq!(params.number("band1Gain", start, 0.0), -12.0);
    assert!((params.number("band1Gain", end, 0.0) - 12.0).abs() < 1e-12);
}

#[test]
fn block_size_invariant() {
    let (from, to) = (r(1, 3), r(7, 3));
    let it = item("a", from, to, vec![ramped(from, to)]);
    let src = Canonical::Stereo(Stereo {
        l: pink(3, 3.0, RATE),
        r: noise(4, 3.0, RATE),
    });
    let p = plan(s(0), r(5, 2), vec![it]);
    let frames = 120_000;
    let (a, _) = render(&p, src.clone(), frames, 48_000);
    assert!(a.l.iter().any(|&v| v != 0.0));
    for block in [128, 1_000] {
        let (b, _) = render(&p, src.clone(), frames, block);
        assert!(a == b, "block {block} changed the output");
    }
}

#[test]
fn stable_at_extremes() {
    let x = noise(11, 10.0, RATE);
    for ty in ["lowShelf", "highShelf", "peak", "lowPass", "highPass"] {
        for f in [20.0, 20_000.0] {
            for g in [-24.0, 24.0] {
                let it = item("a", s(0), s(10), vec![eq(s(0), s(10), &[(ty, f, g, 18.0)])]);
                let y = chain_mono(&it, &x, 4_096);
                assert!(
                    y.iter().all(|v| v.is_finite()),
                    "{ty} at {f} Hz, {g} dB, Q 18 blew up"
                );
                let p = peak_dbfs(&y);
                assert!(p < 60.0, "{ty} at {f} Hz, {g} dB, Q 18: peak {p} dBFS");
            }
        }
    }
}

#[test]
fn unknown_kind_skipped_and_reported() {
    let x = pink(6, 1.0, RATE);
    let newer = AudioEffectPlan {
        id: "fx9".to_string(),
        kind: "spectralRepair".to_string(),
        numbers: BTreeMap::new(),
        values: BTreeMap::new(),
    };
    let p = plan(s(0), s(1), vec![item("a", s(0), s(1), vec![newer])]);
    let (out, m) = render(
        &p,
        Canonical::Stereo(Stereo {
            l: x.clone(),
            r: x.clone(),
        }),
        48_000,
        1_000,
    );
    assert!(
        out.l == x && out.r == x,
        "an unknown effect must not change the signal"
    );
    assert_eq!(
        m.skipped_effects(),
        &[SkippedEffect {
            item: "i1".to_string(),
            effect: "fx9".to_string(),
            kind: "spectralRepair".to_string(),
        }]
    );
}

#[test]
fn mono_and_stereo_channels_filtered_alike() {
    // A stereo source runs one state per channel; with L = R both sides equal the mono run.
    let x = pink(7, 1.0, RATE);
    let fx = eq(
        s(0),
        s(1),
        &[
            ("lowShelf", 150.0, 6.0, 0.707),
            ("peak", 3_000.0, -9.0, 4.0),
        ],
    );
    let it = item("a", s(0), s(1), vec![fx]);
    let mono = chain_mono(&it, &x, 1_000);
    let mut chain = Chain::new(&it, SourceLayout::Stereo).unwrap();
    let mut buf = Canonical::Stereo(Stereo {
        l: x.clone(),
        r: x.clone(),
    });
    chain.process(0, &mut buf);
    let Canonical::Stereo(st) = buf else { panic!() };
    assert!(st.l == mono && st.r == mono);
    assert!(mono != x);
}

/// Pinned: a change here moves an A0 hash and is re-blessed only with an `AUDIO_VERSION` bump.
#[test]
fn golden_hash() {
    let (from, to) = (r(1, 2), r(5, 2));
    let mut fx = eq(
        from,
        to,
        &[
            ("highPass", 40.0, 0.0, 0.707),
            ("lowShelf", 120.0, 4.5, 0.707),
            ("peak", 400.0, -6.0, 1.4),
            ("off", 800.0, 12.0, 1.0),
            ("peak", 2_500.0, 3.0, 0.9),
            ("highShelf", 8_000.0, -3.5, 0.707),
            ("lowPass", 18_000.0, 0.0, 0.707),
        ],
    );
    fx.numbers.insert(
        "band3Gain".to_string(),
        vec![
            ParamSegment {
                from,
                to: r(3, 2),
                v: [-6.0, 6.0],
                interp: Interp::Linear,
                u: [0.0, 1.0],
            },
            ParamSegment {
                from: r(3, 2),
                to,
                v: [6.0, 6.0],
                interp: Interp::Linear,
                u: [0.0, 1.0],
            },
        ],
    );
    let it = item("a", from, to, vec![fx]);
    let src = Canonical::Stereo(Stereo {
        l: pink(21, 3.0, RATE),
        r: sweep(30.0, 16_000.0, 3.0, RATE),
    });
    let (out, _) = render(&plan(s(0), s(3), vec![it]), src, 3 * 48_000, 1_000);
    let mut h = blake3::Hasher::new();
    for (l, r) in out.l.iter().zip(&out.r) {
        h.update(&l.to_le_bytes());
        h.update(&r.to_le_bytes());
    }
    assert_eq!(fin_audio::AUDIO_VERSION, 2);
    assert_eq!(
        h.finalize().to_hex().as_str(),
        "46c0e17d671eacae6f74c9a6e6ae06a5112177aa7af2ebfe8ac2798ef222ca1d"
    );
}

#[test]
fn out_of_range_numbers_clamp_to_the_registry() {
    // A plan can carry numbers outside the registry ranges (a hand-edited or newer model);
    // each band clamps freq to 20…20,000 Hz, gain to ±24 dB and Q to 0.1…18 before designing.
    let x = pink(12, 1.0, RATE);
    let wild = [
        ("peak", 30_000.0, 40.0, 50.0),
        ("lowShelf", 5.0, -40.0, 0.01),
        ("highPass", -100.0, 0.0, 0.0),
    ];
    let clamped = [
        ("peak", 20_000.0, 24.0, 18.0),
        ("lowShelf", 20.0, -24.0, 0.1),
        ("highPass", 20.0, 0.0, 0.1),
    ];
    let run = |bands: &[Band]| {
        let it = item("a", s(0), s(1), vec![eq(s(0), s(1), bands)]);
        chain_mono(&it, &x, 1_000)
    };
    let (a, b) = (run(&wild), run(&clamped));
    assert!(b.iter().all(|v| v.is_finite()) && b != x);
    assert!(
        a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()),
        "out-of-range numbers must play as their clamped values"
    );
}

/// `x` through one biquad of `c`.
fn through(c: &fin_audio::effects::biquad::Coeffs, x: &[f32]) -> Vec<f32> {
    let mut y = x.to_vec();
    Biquad::default().run(c, &mut y);
    y
}

#[test]
fn band_pass_unity_at_centre_bandwidth_from_q_rejects_dc_and_nyquist() {
    // The de-esser's split filter: RBJ "BPF, constant 0 dB peak gain".
    let (f0, fs) = (6_500.0, f64::from(RATE));
    for q in [2.0, 4.0] {
        let c = design(BandKind::BandPass, f0, 0.0, q, fs);
        // Unity at the centre (6,500 whole periods in the last second): 0 ± 0.05 dB.
        let x = sine(f0, -12.0, 1.5, RATE);
        let y = through(&c, &x);
        let g = goertzel_db(&y[24_000..], f0, RATE) - goertzel_db(&x[24_000..], f0, RATE);
        assert!(g.abs() <= 0.05, "Q {q}: {g} dB at the centre (0 ± 0.05)");
        // The −3 dB edges follow Q through the bilinear transform: |Ω − 1/Ω| = 1/Q with
        // Ω = tan(πf/fs)/tan(πf0/fs). RMS over 2 s (not whole periods): −3.01 ± 0.05 dB.
        let t0 = libm::tan(core::f64::consts::PI * f0 / fs);
        let k = 1.0 / (2.0 * q);
        for omega in [libm::sqrt(k * k + 1.0) + k, libm::sqrt(k * k + 1.0) - k] {
            let f = fs / core::f64::consts::PI * libm::atan(omega * t0);
            let x = sine(f, -12.0, 2.5, RATE);
            let y = through(&c, &x);
            let g = rms_dbfs(&y[24_000..]) - rms_dbfs(&x[24_000..]);
            assert!(
                (g + 3.0103).abs() <= 0.05,
                "Q {q}: {g} dB at the edge {f} Hz (−3.01 ± 0.05)"
            );
        }
        // DC and Nyquist: below −100 dB relative to the input after settling.
        let dc = through(&c, &vec![0.5f32; 48_000]);
        let ny: Vec<f32> = (0..48_000)
            .map(|n| if n % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let ny = through(&c, &ny);
        for (name, y) in [("DC", dc), ("Nyquist", ny)] {
            let worst = y[24_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(
                f64::from(worst) <= 0.5 * amp(-100.0),
                "Q {q}: {name} leaks at {worst}"
            );
        }
    }
}
