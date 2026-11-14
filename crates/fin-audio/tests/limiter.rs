//! The look-ahead true-peak limiter (plan 6C Decision 18): A0 (hashed, bit-identical native and
//! on wasm32), latency-compensated (output frame `k` is input frame `k`).

use fin_audio::loudness::{Limiter, Meter};
use fin_audio::testing::signals::*;
use fin_audio::{RATE, Resampler, Stereo};

const TAU: f64 = 2.0 * core::f64::consts::PI;

/// Runs `s` through a limiter in blocks of `block` frames; returns every output, in order.
fn run(ceiling: f64, s: &Stereo, block: usize) -> Stereo {
    let mut lim = Limiter::new(ceiling);
    let mut out = Stereo::default();
    let mut at = 0;
    while at < s.len() {
        let end = (at + block).min(s.len());
        let mut b = Stereo {
            l: s.l[at..end].to_vec(),
            r: s.r[at..end].to_vec(),
        };
        lim.process(&mut b);
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
        at = end;
    }
    lim.finish(&mut out);
    out
}

fn true_peak(s: &Stereo) -> f64 {
    let mut m = Meter::new(2);
    m.push(s);
    m.measured().true_peak.unwrap()
}

fn stereo(l: Vec<f32>, r: Vec<f32>) -> Stereo {
    Stereo { l, r }
}

/// A sine at fs/4, phase 45°, sample peak 0 dBFS: its true peak is +3.01 dBTP.
fn isp_tone(secs: f64) -> Vec<f32> {
    let a = 1.0 / libm::sin(TAU / 8.0);
    (0..(secs * f64::from(RATE)) as usize)
        .map(|i| (a * libm::sin(TAU * i as f64 / 4.0 + TAU / 8.0)) as f32)
        .collect()
}

/// Band-limits `x` to 16 kHz (48 → 32 → 48 kHz through the resampler).
fn band_limited(x: &[f32]) -> Vec<f32> {
    let pass = |x: &[f32], from, to| {
        let mut r = Resampler::new(from, to).unwrap();
        let mut o = Vec::new();
        r.process(x, &mut o);
        r.finish(&mut o);
        o
    };
    pass(&pass(x, RATE, 32_000), 32_000, RATE)
}

/// The true peak by 8× oversampling through the resampler (a 140-tap Kaiser kernel), dBTP.
fn reference_true_peak(x: &[f32]) -> f64 {
    let mut r = Resampler::new(RATE, 8 * RATE).unwrap();
    let mut o = Vec::new();
    r.process(x, &mut o);
    r.finish(&mut o);
    20.0 * libm::log10(f64::from(o.iter().fold(0.0f32, |m, v| m.max(v.abs()))))
}

/// A +6 dBFS 997 Hz sine, an inter-sample-peak tone and loud band-limited pink noise, each
/// limited to −1 dBTP: the meter's true peak is at most −1.0 + 0.1 dBTP, and the length is
/// unchanged.
///
/// Full-band noise is checked against an 8× reference instead (≤ −1.0 + 0.05 dBTP): `ebur128`'s
/// 4× interpolator (a 48-tap Hann-windowed sinc cut off at Nyquist) misreads content near
/// Nyquist by tenths of a dB either way (it reads raw white noise 1 dB low and limited white
/// noise 0.35 dB high), so it is no oracle there.
#[test]
fn limiter_holds_ceiling() {
    let hot = sine(997.0, 6.0, 2.0, RATE);
    let isp = isp_tone(2.0);
    let loud = |seed| -> Vec<f32> {
        band_limited(&pink(seed, 2.0, RATE))
            .into_iter()
            .map(|v| v * 8.0)
            .collect()
    };
    let cases = [
        ("997 Hz +6 dBFS", stereo(hot.clone(), hot)),
        ("fs/4 at 45°", stereo(isp.clone(), isp)),
        ("pink to 16 kHz +18 dB", stereo(loud(3), loud(4))),
    ];
    for (name, s) in cases {
        assert!(true_peak(&s) > 0.0, "{name} starts above the ceiling");
        let out = run(-1.0, &s, 4_096);
        assert_eq!(out.len(), s.len(), "{name}");
        let tp = true_peak(&out);
        assert!(tp <= -0.9, "{name}: {tp} dBTP");
    }
    let full = |x: Vec<f32>| -> Vec<f32> { x.into_iter().map(|v| v * 8.0).collect() };
    let s = stereo(full(pink(5, 2.0, RATE)), full(noise(6, 2.0, RATE)));
    let out = run(-1.0, &s, 4_096);
    for (name, x) in [("full-band pink", &out.l), ("white", &out.r)] {
        let tp = reference_true_peak(x);
        assert!(tp <= -0.95, "{name}: {tp} dBTP by the 8x reference");
    }
}

/// Below the ceiling the limiter is the identity: a −10 dBFS sine comes out bit-exact, with the
/// first block short by the latency (the 5 ms look-ahead plus the detector's reach).
#[test]
fn limiter_transparent_below_ceiling() {
    // The 5 ms look-ahead (240 frames, the current one included) and the detector's reach (71).
    assert_eq!(Limiter::LATENCY, 310);
    let x = sine(997.0, -10.0, 1.0, RATE);
    let s = stereo(x.clone(), x);
    let mut lim = Limiter::new(-1.0);
    let mut first = Stereo {
        l: s.l[..4_096].to_vec(),
        r: s.r[..4_096].to_vec(),
    };
    lim.process(&mut first);
    assert_eq!(first.len(), 4_096 - Limiter::LATENCY);
    let out = run(-1.0, &s, 4_096);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&out.l), bits(&s.l));
    assert_eq!(bits(&out.r), bits(&s.r));
}

/// `finish` returns the tail: whatever the blocks, the output has the input's length and the
/// same bits (an output frame depends only on the input, never on the block boundaries).
#[test]
fn limiter_latency_compensated() {
    let s = stereo(isp_tone(0.5), pink(5, 0.5, RATE));
    let whole = run(-1.0, &s, s.len());
    assert_eq!(whole.len(), s.len());
    for block in [1, 7, 240, 311, 4_096] {
        let out = run(-1.0, &s, block);
        assert_eq!(out, whole, "block {block}");
    }
    // A stream shorter than the latency comes out entirely from `finish`.
    let short = stereo(vec![0.5; 100], vec![-0.5; 100]);
    let mut lim = Limiter::new(-1.0);
    let mut b = short.clone();
    lim.process(&mut b);
    assert!(b.is_empty());
    let mut tail = Stereo::default();
    lim.finish(&mut tail);
    assert_eq!(tail, short);
    // The limiter is reset by `finish`: a second stream gives the same output as a fresh one.
    let again = {
        let mut out = Stereo::default();
        let mut b = s.clone();
        lim.process(&mut b);
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
        lim.finish(&mut out);
        out
    };
    assert_eq!(again, whole);
    // Gain reduction is aligned with the peak it answers: a 10 ms over-ceiling burst on a quiet
    // bed is reduced where it sits; the bed is untouched until the look-ahead (and the
    // oversampler's reach) before it, and reduced just before it (the attack ramp).
    let bed = sine(1_000.0, -30.0, 1.0, RATE);
    let mut s = stereo(bed.clone(), bed);
    for i in 0..480 {
        let v = (2.0 * libm::sin(TAU * 1_000.0 * i as f64 / f64::from(RATE))) as f32;
        s.l[24_000 + i] += v;
        s.r[24_000 + i] += v;
    }
    let out = run(-1.0, &s, 1_000);
    let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let p = peak(&out.l[24_000..24_480]);
    assert!(p > 0.5 && p < 0.9, "burst peak {p}");
    let quiet = 24_000 - Limiter::LATENCY;
    assert_eq!(out.l[..quiet], s.l[..quiet]);
    assert!(peak(&out.l[23_976..24_000]) < 0.8 * peak(&s.l[23_976..24_000]));
}

/// Pinned: a change here moves an A0 hash and is re-blessed only with an `AUDIO_VERSION` bump.
#[test]
fn limiter_golden_hash() {
    let l: Vec<f32> = pink(21, 1.0, RATE).into_iter().map(|v| v * 6.0).collect();
    let r: Vec<f32> = sine(997.0, 3.0, 1.0, RATE);
    let out = run(-1.0, &stereo(l, r), 1_024);
    assert_eq!(out.len(), RATE as usize);
    let mut h = blake3::Hasher::new();
    for v in out.l.iter().chain(&out.r) {
        h.update(&v.to_le_bytes());
    }
    assert_eq!(fin_audio::AUDIO_VERSION, 1);
    assert_eq!(
        h.finalize().to_hex().as_str(),
        "e58c9a93008d12b61eccbbe9df6d8732d0916f09610ebd281b80df3029c37e1e"
    );
}
