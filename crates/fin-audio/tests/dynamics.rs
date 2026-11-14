//! The dynamics effects (plan 6C Task 9; Decisions 13, 16): `compressor`, `gate` and `deEsser`.
//!
//! Levels are dBFS of the detector's measure: the sample peak of the linked channels over the
//! last 10 ms for `peak` (and the gate), the mean square of the linked channels over the last
//! 10 ms for `rms` (and the de-esser's band); a sine's RMS is its peak − 3.01 dB.

use std::collections::BTreeMap;

use fin_audio::channels::Canonical;
use fin_audio::effects::Chain;
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

/// A `kind` effect over `[0, 100)` s with `numbers` constant and `values` as text.
fn fx(kind: &str, numbers: &[(&str, f64)], values: &[(&str, &str)]) -> AudioEffectPlan {
    let (from, to) = (s(0), s(100));
    AudioEffectPlan {
        id: format!("fx-{kind}"),
        kind: kind.to_string(),
        numbers: numbers
            .iter()
            .map(|&(k, v)| (k.to_string(), vec![constant(from, to, v)]))
            .collect(),
        values: values
            .iter()
            .map(|&(k, v)| (k.to_string(), PlanValue::Text(v.to_string())))
            .collect(),
    }
}

/// A compressor with `detector`, threshold, ratio and knee (attack 10, release 100, makeup 0).
fn comp(detector: &str, threshold: f64, ratio: f64, knee: f64) -> AudioEffectPlan {
    fx(
        "compressor",
        &[
            ("threshold", threshold),
            ("ratio", ratio),
            ("knee", knee),
            ("attack", 10.0),
            ("release", 100.0),
            ("makeup", 0.0),
        ],
        &[("detector", detector)],
    )
}

/// A gate with the registry defaults but `hold`.
fn gate(hold: f64) -> AudioEffectPlan {
    fx(
        "gate",
        &[
            ("threshold", -50.0),
            ("range", -80.0),
            ("attack", 1.0),
            ("hold", hold),
            ("release", 100.0),
        ],
        &[],
    )
}

/// A de-esser at the registry defaults but `ratio` and `range`.
fn deesser(ratio: f64, range: f64) -> AudioEffectPlan {
    fx(
        "deEsser",
        &[
            ("frequency", 6_500.0),
            ("q", 2.0),
            ("threshold", -30.0),
            ("ratio", ratio),
            ("range", range),
        ],
        &[],
    )
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

/// `x` (mono, from the item's first sample) through `effect`'s chain in calls of 1,000 samples.
fn run_mono(effect: AudioEffectPlan, x: &[f32]) -> Vec<f32> {
    let it = item("a", s(0), s(100), vec![effect]);
    let mut chain = Chain::new(&it, SourceLayout::Mono).unwrap();
    let start = sample_at(it.from).unwrap();
    let mut out = Vec::with_capacity(x.len());
    for (k, piece) in x.chunks(1_000).enumerate() {
        let mut buf = Canonical::Mono(piece.to_vec());
        chain.process(start + (k * 1_000) as i64, &mut buf);
        let Canonical::Mono(y) = buf else {
            panic!("the chain changed the layout")
        };
        out.extend_from_slice(&y);
    }
    out
}

/// `(l, r)` through `effect`'s chain in one call.
fn run_stereo(effect: AudioEffectPlan, l: &[f32], r: &[f32]) -> Stereo {
    let it = item("a", s(0), s(100), vec![effect]);
    let mut chain = Chain::new(&it, SourceLayout::Stereo).unwrap();
    let mut buf = Canonical::Stereo(Stereo {
        l: l.to_vec(),
        r: r.to_vec(),
    });
    chain.process(0, &mut buf);
    let Canonical::Stereo(st) = buf else { panic!() };
    st
}

const MS: usize = 48;

/// `secs` of an alternating ±`a` signal (constant magnitude, so `y/x` is the gain per sample).
fn square(a: f64, secs: f64) -> Vec<f32> {
    (0..(secs * f64::from(RATE)) as usize)
        .map(|n| if n % 2 == 0 { a as f32 } else { -a as f32 })
        .collect()
}

/// The gain per sample in dB of `y` against `x` (where `x ≠ 0`).
fn gain_db(x: &[f32], y: &[f32]) -> Vec<f64> {
    x.iter()
        .zip(y)
        .map(|(&a, &b)| 20.0 * libm::log10(f64::from(b) / f64::from(a)))
        .collect()
}

/// The first index from `from` where `g` crosses `level` (going `down` or up).
fn crossing(g: &[f64], from: usize, level: f64, down: bool) -> usize {
    (from..g.len())
        .find(|&n| if down { g[n] <= level } else { g[n] >= level })
        .expect("never crossed")
}

fn near(got: f64, want: f64, tol: f64, what: &str) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got:.4} (want {want} ± {tol})"
    );
}

/// The last `secs` of `x`.
fn tail(x: &[f32], secs: f64) -> &[f32] {
    &x[x.len() - (secs * f64::from(RATE)) as usize..]
}

#[test]
fn compressor_static_curve_peak() {
    // −6 dBFS peak: overshoot 12 dB, reduction 12 × (1 − 1/4) = 9 dB.
    let x = sine(1_000.0, -6.0, 2.0, RATE);
    let y = run_mono(comp("peak", -18.0, 4.0, 0.0), &x);
    near(peak_dbfs(tail(&y, 0.5)), -15.0, 0.1, "output peak");
}

#[test]
fn compressor_static_curve_rms() {
    // RMS −9.03 dBFS: overshoot 8.97 dB, reduction 6.73 dB.
    let x = sine(1_000.0, -6.0, 2.0, RATE);
    let y = run_mono(comp("rms", -18.0, 4.0, 0.0), &x);
    near(rms_dbfs(tail(&y, 0.5)), -15.76, 0.1, "output RMS");
    near(peak_dbfs(tail(&y, 0.5)), -12.73, 0.1, "output peak");
}

#[test]
fn compressor_knee_midpoint() {
    // Knee 6 dB, ratio 4: at the threshold the quadratic gives (1/4 − 1)·(W/2)²/(2W) = −0.5625
    // dB; at its edges it meets the hard curve (no reduction at T − 3, T + 3/4 at T + 3).
    for (level, want) in [(-18.0, -18.5625), (-21.0, -21.0), (-15.0, -17.25)] {
        let x = sine(1_000.0, level, 2.0, RATE);
        let y = run_mono(comp("peak", -18.0, 4.0, 6.0), &x);
        near(
            peak_dbfs(tail(&y, 0.5)),
            want,
            0.02,
            &format!("{level} dBFS"),
        );
    }
}

/// A step from `a` dB to `b` dB at 0.5 s through `effect`: the per-sample gain and the step.
fn step(effect: AudioEffectPlan, a: f64, b: f64) -> (Vec<f64>, usize) {
    let mut x = square(amp(a), 0.5);
    let at = x.len();
    x.extend(square(amp(b), 1.5));
    let y = run_mono(effect, &x);
    (gain_db(&x, &y), at)
}

#[test]
fn attack_time_constant() {
    for attack in [10.0, 50.0] {
        let mut c = comp("peak", -18.0, 4.0, 0.0);
        c.numbers
            .insert("attack".to_string(), vec![constant(s(0), s(100), attack)]);
        // −20 → 0 dBFS (+20 dB): the gain goes from 0 to −13.5 dB.
        let (g, at) = step(c, -20.0, 0.0);
        assert!(g[at - 1].abs() < 1e-9, "no reduction below the threshold");
        near(g[g.len() - 1], -13.5, 0.01, "settled reduction");
        let n = crossing(&g, at, 0.632 * -13.5, true);
        near(
            (n - at) as f64 / MS as f64,
            attack,
            1.0,
            "attack 63 % time (ms)",
        );
    }
}

#[test]
fn release_time_constant() {
    for release in [100.0, 400.0] {
        let mut c = comp("peak", -18.0, 4.0, 0.0);
        c.numbers
            .insert("release".to_string(), vec![constant(s(0), s(100), release)]);
        // 0 → −20 dBFS: the gain returns from −13.5 to 0 dB once the 10 ms window has passed.
        let (g, at) = step(c, 0.0, -20.0);
        near(g[at - 1], -13.5, 0.01, "settled reduction");
        let moved = crossing(&g, at, -13.5 + 1e-3, false);
        assert!(
            moved - at <= 10 * MS,
            "the window holds {} samples",
            moved - at
        );
        let n = crossing(&g, moved, -13.5 * (1.0 - 0.632), false);
        near(
            (n - moved) as f64 / MS as f64,
            release,
            1.0,
            "release 63 % time (ms)",
        );
    }
}

#[test]
fn rms_vs_peak_detector_differ_on_crest() {
    // Clicks of 0.5 (−6.02 dBFS) every 5 ms: their peak is 11.98 dB over the threshold, their
    // RMS (−6.02 − 10·log10(240) ≈ −29.8 dBFS) far under it.
    let x: Vec<f32> = (0..2 * 48_000)
        .map(|n| if n % 240 == 0 { 0.5 } else { 0.0 })
        .collect();
    let last = |y: &[f32]| peak_dbfs(tail(y, 0.25));
    let peak = run_mono(comp("peak", -18.0, 4.0, 0.0), &x);
    let rms = run_mono(comp("rms", -18.0, 4.0, 0.0), &x);
    let click = 20.0 * libm::log10(0.5);
    near(
        last(&peak),
        click - 11.98 * 0.75,
        0.1,
        "peak detector: clicks reduced by 8.99 dB",
    );
    near(last(&rms), click, 0.01, "rms detector: clicks untouched");
}

#[test]
fn stereo_detection_is_linked() {
    // Loud left, silent-ish right: the right channel gets the left's reduction.
    let l = sine(1_000.0, -6.0, 2.0, RATE);
    let rr = sine(1_000.0, -40.0, 2.0, RATE);
    let y = run_stereo(comp("peak", -18.0, 4.0, 0.0), &l, &rr);
    near(peak_dbfs(tail(&y.l, 0.5)), -15.0, 0.1, "left");
    near(peak_dbfs(tail(&y.r, 0.5)), -49.0, 0.1, "right");
}

#[test]
fn makeup_adds_gain() {
    let mut c = comp("peak", -18.0, 4.0, 0.0);
    c.numbers
        .insert("makeup".to_string(), vec![constant(s(0), s(100), 6.0)]);
    let y = run_mono(c, &sine(1_000.0, -6.0, 2.0, RATE));
    near(
        peak_dbfs(tail(&y, 0.5)),
        -9.0,
        0.1,
        "output peak with 6 dB makeup",
    );
}

#[test]
fn gate_closes_below_threshold() {
    let x: Vec<f32> = noise(5, 3.0, RATE)
        .iter()
        .map(|&v| (f64::from(v) * amp(-70.0)) as f32)
        .collect();
    let y = run_mono(gate(50.0), &x);
    let att = rms_dbfs(tail(&y, 1.0)) - rms_dbfs(tail(&x, 1.0));
    near(att, -80.0, 0.5, "attenuation of −70 dBFS noise");
    // Above the threshold it stays open.
    let loud = sine(1_000.0, -20.0, 1.0, RATE);
    let y = run_mono(gate(50.0), &loud);
    assert!(y == loud, "an open gate changed the signal");
}

#[test]
fn gate_hold_keeps_open() {
    // −20 → −70 dBFS at 0.5 s: the 10 ms window passes, then the gain stays open `hold` ms.
    let (g, at) = step(gate(50.0), -20.0, -70.0);
    // The window holds the last 480 samples, the current one included.
    let closes = at + 10 * MS - 1 + 50 * MS;
    assert!(
        g[at..closes].iter().all(|v| v.abs() < 1e-6),
        "closed during the hold"
    );
    // Release 100 ms towards −80 dB: −80·(1 − e^(−1/20)) ≈ −3.9 dB 5 ms later.
    assert!(g[closes + 5 * MS] <= -3.0, "{} dB", g[closes + 5 * MS]);
    // Without a hold it starts closing as soon as the window passes.
    let (g0, _) = step(gate(0.0), -20.0, -70.0);
    assert!(g0[at + 15 * MS] <= -3.0, "{} dB", g0[at + 15 * MS]);
    // And it opens again with the attack (1 ms): −80 dB → 63 % of the way within 1 ± 1 ms.
    let (g, at) = step(gate(50.0), -70.0, -20.0);
    assert!(g[at - 1] <= -78.0, "closed before: {} dB", g[at - 1]);
    let n = crossing(&g, at, -80.0 * (1.0 - 0.632), false);
    near(
        (n - at) as f64 / MS as f64,
        1.0,
        1.0,
        "gate attack 63 % time (ms)",
    );
}

/// A 200 Hz tone (−12 dBFS) with a 6.5 kHz burst (`burst` dBFS peak) from 0.5 to 1.5 s.
fn sibilant(burst: f64) -> Vec<f32> {
    let low = sine(200.0, -12.0, 2.0, RATE);
    let hi = tone_burst(6_500.0, burst, 0.5, 1.0, 2.0, RATE);
    low.iter().zip(&hi).map(|(a, b)| a + b).collect()
}

/// Goertzel level change at `hz` over 1.0–1.5 s (inside the burst, settled).
fn change_at(x: &[f32], y: &[f32], hz: f64) -> f64 {
    let w = 48_000..72_000;
    goertzel_db(&y[w.clone()], hz, RATE) - goertzel_db(&x[w], hz, RATE)
}

#[test]
fn deesser_reduces_band_only() {
    let x = sibilant(-12.0);
    let y = run_mono(deesser(6.0, 12.0), &x);
    let hi = change_at(&x, &y, 6_500.0);
    assert!(hi <= -6.0, "6.5 kHz changed by {hi} dB (≤ −6)");
    near(change_at(&x, &y, 200.0), 0.0, 0.1, "200 Hz change (dB)");
    // Before the burst nothing is over the threshold: the tone passes bit-exact.
    assert!(
        x[..20_000] == y[..20_000],
        "the de-esser touched a quiet band"
    );
}

#[test]
fn deesser_range_caps_reduction() {
    // −6 dBFS burst: its RMS is 27 dB over the threshold, ratio 20 asks for ~25.6 dB.
    let x = sibilant(-6.0);
    for range in [3.0, 12.0] {
        let y = run_mono(deesser(20.0, range), &x);
        near(
            change_at(&x, &y, 6_500.0),
            -range,
            0.1,
            &format!("reduction capped at {range} dB"),
        );
    }
}

/// `effect` with `name` at `a` until 1 s, then `b`.
fn switch(mut effect: AudioEffectPlan, name: &str, a: f64, b: f64) -> AudioEffectPlan {
    effect.numbers.insert(
        name.to_string(),
        vec![constant(s(0), s(1), a), constant(s(1), s(100), b)],
    );
    effect
}

#[test]
fn animated_range_caps_the_applied_gain_at_once() {
    // De-esser: a steady −6 dBFS 6.5 kHz sine held 12 dB down; at 1 s the range drops to 0, so
    // from that block on nothing may be reduced (the smoothed gain must not lag the cap).
    let x = sine(6_500.0, -6.0, 2.0, RATE);
    let y = run_mono(switch(deesser(20.0, 12.0), "range", 12.0, 0.0), &x);
    near(
        peak_dbfs(&y[40_000..48_000]),
        -18.0,
        0.1,
        "reduced 12 dB before 1 s",
    );
    assert!(
        y[48_000..] == x[48_000..],
        "reduction left after the range went to 0 dB"
    );
    // Gate: closed on −70 dBFS, its range rises from −80 to −20 dB at 1 s: the gain is at most
    // 20 dB down from that sample on.
    let (g, at) = step(switch(gate(0.0), "range", -80.0, -20.0), -70.0, -70.0);
    assert!(g[at + 23_000] <= -79.5, "closed to −80 dB before 1 s");
    let worst = g[at + 24_000..].iter().fold(0.0f64, |m, &v| m.min(v));
    assert!(
        worst >= -20.0 - 1e-4,
        "{worst} dB below the −20 dB range after 1 s"
    );
}

#[test]
fn no_nan_on_silence() {
    let zero = silence(1.0, RATE);
    let tiny = vec![1e-38f32; 48_000];
    for effect in [
        comp("rms", -60.0, 20.0, 24.0),
        comp("peak", -60.0, 20.0, 0.0),
        gate(0.0),
        deesser(20.0, 24.0),
    ] {
        let kind = effect.kind.clone();
        let y = run_mono(effect.clone(), &zero);
        assert!(
            y.iter().all(|&v| v == 0.0),
            "{kind}: silence in, silence out"
        );
        let y = run_mono(effect, &tiny);
        assert!(
            y.iter().all(|v| v.is_finite()),
            "{kind}: non-finite on tiny input"
        );
    }
}

fn plan(from: Rational, to: Rational, items: Vec<AudioItem>) -> AudioPlan {
    AudioPlan {
        timeline: "tl1".to_string(),
        from,
        to,
        items,
    }
}

fn render(p: &AudioPlan, c: Canonical, frames: usize, block: usize) -> Stereo {
    let src: BTreeMap<String, Box<dyn PcmSource>> =
        BTreeMap::from([("a".to_string(), MemSource::boxed(c))]);
    let mut m = Mixer::new(p, src).unwrap();
    let mut out = Stereo::default();
    while out.len() < frames {
        let mut b = Stereo::silence(block.min(frames - out.len()));
        m.render(&mut b).unwrap();
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
    }
    assert!(m.skipped_effects().is_empty());
    out
}

/// `effect` with `name` ramping linearly from `a` to `b` over `[from, to)`.
fn ramp(
    mut effect: AudioEffectPlan,
    name: &str,
    from: Rational,
    to: Rational,
    a: f64,
    b: f64,
) -> AudioEffectPlan {
    effect.numbers.insert(
        name.to_string(),
        vec![ParamSegment {
            from,
            to,
            v: [a, b],
            interp: Interp::Linear,
            u: [0.0, 1.0],
        }],
    );
    effect
}

/// The golden scene of `kind`: a stereo item from 1/3 s to 7/3 s, the effect's threshold
/// ramping over it, rendered over 2.5 s.
fn scene(kind: &str) -> (AudioPlan, Canonical) {
    let (from, to) = (r(1, 3), r(7, 3));
    let effect = match kind {
        "compressor" => ramp(
            comp("rms", -30.0, 6.0, 6.0),
            "threshold",
            from,
            to,
            -40.0,
            -10.0,
        ),
        "compressorPeak" => ramp(comp("peak", -20.0, 3.0, 0.0), "ratio", from, to, 1.0, 20.0),
        "gate" => ramp(gate(20.0), "threshold", from, to, -40.0, 0.0),
        _ => ramp(deesser(8.0, 18.0), "frequency", from, to, 3_000.0, 9_000.0),
    };
    let it = item("a", from, to, vec![effect]);
    let src = Canonical::Stereo(Stereo {
        l: pink(31, 3.0, RATE),
        r: sweep(40.0, 16_000.0, 3.0, RATE)
            .iter()
            .zip(tone_burst(5_000.0, -10.0, 1.0, 0.4, 3.0, RATE))
            .map(|(a, b)| a * 0.25 + b)
            .collect(),
    });
    (plan(s(0), r(5, 2), vec![it]), src)
}

#[test]
fn block_size_invariant() {
    for kind in ["compressor", "compressorPeak", "gate", "deEsser"] {
        let (p, src) = scene(kind);
        let a = render(&p, src.clone(), 120_000, 48_000);
        for block in [37, 128, 1_000] {
            assert!(
                render(&p, src.clone(), 120_000, block) == a,
                "{kind}: block {block} changed the output"
            );
        }
    }
}

/// Pinned: a change here moves an A0 hash and is re-blessed only with an `AUDIO_VERSION` bump.
#[test]
fn golden_hash() {
    let want = [
        (
            "compressor",
            "ad720459a0b858e07e7ff43492c088fc55c0b1a0506962a4968a7787e99d991d",
        ),
        (
            "compressorPeak",
            "cac09a77b3e18b1baaa84b0b7576188edd5b37ea2a950f445c6feea748c909f4",
        ),
        (
            "gate",
            "079353985d1743a573b937f4db52412ed4efd5411ccc5f59422620667af269ed",
        ),
        (
            "deEsser",
            "c93ccd728368b728b2564955d3988f99e5ad76d9f2d758bdcf98e0b9fe548bc3",
        ),
    ];
    let mut got = Vec::new();
    for (kind, _) in want {
        let (mut p, src) = scene(kind);
        let out = render(&p, src.clone(), 120_000, 1_000);
        p.items[0].effects.clear();
        let dry = render(&p, src, 120_000, 1_000);
        assert!(
            null_db(&out.l, &dry.l) > -40.0,
            "{kind}: the scene must exercise the effect"
        );
        assert!(
            null_db(&out.r, &dry.r) > -40.0,
            "{kind}: the scene must exercise the effect"
        );
        let mut h = blake3::Hasher::new();
        for (l, r) in out.l.iter().zip(&out.r) {
            h.update(&l.to_le_bytes());
            h.update(&r.to_le_bytes());
        }
        got.push((kind, h.finalize().to_hex().to_string()));
    }
    assert_eq!(fin_audio::AUDIO_VERSION, 2);
    for ((kind, h), (_, w)) in got.iter().zip(want) {
        assert_eq!(h, w, "{kind}");
    }
}

/// The detectors' `log2` and `amp`'s `exp2` (6C Task 23: polynomials instead of
/// `libm::log10f`/`exp10f` per frame) stay within their documented errors over every input
/// the dynamics effects feed them, and are exact where it matters: powers of two have their
/// exponent as `log2`, integers give exact powers of two, and 0 dB is unity gain.
#[test]
fn log2_exp2_polynomials_accurate() {
    use fin_audio::effects::detector::{amp, exp2_poly, log2_poly};
    // log2 over the levels' whole range (the floors are 1e-20 and 1e-10 and up to far above
    // full scale), absolute error below 4e-5 (2.5e-4 dB of an amplitude).
    let mut worst = 0.0f64;
    let mut x = 1e-20f32;
    while x < 1e6 {
        let e = (f64::from(log2_poly(x)) - f64::from(x).log2()).abs();
        worst = worst.max(e);
        x *= 1.000_37;
    }
    assert!(worst < 4e-5, "log2: worst absolute error {worst:e}");
    for k in -66..=20 {
        assert_eq!(log2_poly(2f32.powi(k)), k as f32, "log2(2^{k})");
    }
    // exp2 over the gains' range (gate ranges down to −90 dB, makeup up to +24 dB, so about
    // −15…+4 octaves, and the clamp beyond), relative error below 5e-6 (5e-5 dB).
    let mut worst = 0.0f64;
    let mut y = -30.0f32;
    while y < 10.0 {
        let want = f64::from(y).exp2();
        let e = ((f64::from(exp2_poly(y)) - want) / want).abs();
        worst = worst.max(e);
        y += 0.000_731;
    }
    assert!(worst < 5e-6, "exp2: worst relative error {worst:e}");
    for k in -126..=127 {
        assert_eq!(exp2_poly(k as f32), 2f32.powi(k), "exp2({k})");
    }
    assert_eq!(exp2_poly(-1000.0), 2f32.powi(-126), "clamped below");
    assert_eq!(amp(0.0), 1.0, "0 dB is unity");
    // In dB: within 1e-4 dB of the exact conversion.
    for db in [-90.0f32, -60.0, -20.0, -6.0, -0.5, 0.25, 3.0, 12.0, 24.0] {
        let got = 20.0 * f64::from(amp(db)).log10();
        assert!((got - f64::from(db)).abs() < 1e-4, "amp({db}) is {got} dB");
    }
}
