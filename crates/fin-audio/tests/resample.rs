//! The polyphase resampler against plan 6C Decision 5. Levels are read with single-bin DFTs over
//! exactly one second of steady-state signal, so every integer-Hz tone is orthogonal to every
//! other one and the measurement is exact (no leakage between a strong tone and a −100 dB one).

use fin_audio::testing::{analyse::*, signals::*};
use fin_audio::{Resampler, resampled_len};

/// The ratios every test runs at unless it says otherwise.
const RATIOS: [(u32, u32); 6] = [
    (44_100, 48_000),
    (48_000, 16_000),
    (22_050, 48_000),
    (96_000, 48_000),
    (8_000, 48_000),
    (47_999, 48_000),
];

/// Input length for the tone tests, and where the measured second starts (past the filter's
/// start-up, which is at most 210 input samples here).
const SECS: f64 = 1.1;
const SKIP: f64 = 0.05;

fn run_chunked(from: u32, to: u32, input: &[f32], chunk: usize) -> Vec<f32> {
    let mut r = Resampler::new(from, to).unwrap();
    let mut out = Vec::new();
    for c in input.chunks(chunk.max(1)) {
        r.process(c, &mut out);
    }
    r.finish(&mut out);
    out
}

fn run(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
    run_chunked(from, to, input, input.len().max(1))
}

/// The level in dBFS of the integer-Hz tone `hz` over one second of `x` starting at `SKIP`.
fn level(x: &[f32], hz: f64, rate: u32) -> f64 {
    let start = (SKIP * f64::from(rate)) as usize;
    let window = &x[start..start + rate as usize];
    goertzel_db(window, hz, rate)
}

/// The resampler's gain in dB at `hz` (an integer frequency), from a −6 dBFS sine.
fn gain_db(from: u32, to: u32, hz: f64) -> f64 {
    let input = sine(hz, -6.0, SECS, from);
    let out = run(from, to, &input);
    level(&out, hz, to) - level(&input, hz, from)
}

fn fs_min(from: u32, to: u32) -> f64 {
    f64::from(from.min(to))
}

#[test]
fn length_is_exact() {
    assert_eq!(resampled_len(0, 44_100, 48_000), 0);
    assert_eq!(resampled_len(1, 44_100, 48_000), 2);
    assert_eq!(resampled_len(1, 48_000, 16_000), 1);
    assert_eq!(resampled_len(441_000, 44_100, 48_000), 480_000);
    assert_eq!(resampled_len(1_000, 48_000, 16_000), 334);
    assert_eq!(resampled_len(1_000, 47_999, 48_000), 1_001);
    assert_eq!(resampled_len(441_000, 96_000, 48_000), 220_500);
    for (from, to) in RATIOS {
        for n in [0u64, 1, 1_000, 441_000] {
            let want = (u128::from(n) * u128::from(to)).div_ceil(u128::from(from)) as u64;
            assert_eq!(resampled_len(n, from, to), want, "{from} -> {to}, n = {n}");
            let out = run(from, to, &vec![0.0; n as usize]);
            assert_eq!(out.len() as u64, want, "{from} -> {to}, n = {n}");
        }
    }
}

#[test]
fn impulse_lands_on_mapped_sample() {
    let out = run(44_100, 48_000, &impulse(4_410, 8_820));
    let peak = (0..out.len())
        .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
        .unwrap();
    assert_eq!(peak, 4_800, "peak {} at {peak}", out[peak]);
}

#[test]
fn passband_flat() {
    let edges = [
        (44_100, 48_000, 20_000.0),
        (48_000, 16_000, 7_250.0),
        (22_050, 48_000, 10_000.0),
        (96_000, 48_000, 21_700.0),
        (8_000, 48_000, 3_620.0),
        (47_999, 48_000, 21_760.0),
    ];
    for (from, to, edge) in edges {
        let f64_edge: f64 = edge;
        for hz in [
            100.0,
            1_000.0,
            libm::round(f64_edge * 0.5),
            libm::round(f64_edge * 0.8),
            libm::round(f64_edge * 0.95),
            f64_edge,
        ] {
            let g = gain_db(from, to, hz);
            assert!(g.abs() <= 0.01, "{from} -> {to} at {hz} Hz: {g} dB");
        }
    }
}

#[test]
fn stopband_rejects() {
    // (from, to, tone in, the frequency it aliases or images to in the output)
    let cases = [
        (48_000, 16_000, 9_000.0, 7_000.0),
        (96_000, 48_000, 26_000.0, 22_000.0),
        (44_100, 16_000, 18_000.0, 2_000.0),
        (44_100, 48_000, 15_000.0, 18_900.0),
        // Just past the stopband edge (8 kHz at 16 kHz out): aliases to 7,980 Hz.
        (48_000, 16_000, 8_020.0, 7_980.0),
        // L > 1024 (phase tables interpolated). 48,000 -> 47,999 has no whole-Hz tone in its
        // stopband (23,999.5 to 24,000 Hz), so 48,000 -> 32,001 (L = 10,667) stands in for the
        // downsampling side: 18 kHz aliases to 14,001 Hz.
        (48_000, 32_001, 18_000.0, 14_001.0),
        // Upsampling, L = 48,000: 20 kHz images to 27,999 Hz, folding to 20,001 Hz.
        (47_999, 48_000, 20_000.0, 20_001.0),
    ];
    for (from, to, tone, alias) in cases {
        let reference = level(
            &run(from, to, &sine(1_000.0, -6.0, SECS, from)),
            1_000.0,
            to,
        );
        let out = run(from, to, &sine(tone, -6.0, SECS, from));
        let rel = level(&out, alias, to) - reference;
        assert!(
            rel <= -95.0,
            "{from} -> {to}: {tone} Hz in, {alias} Hz out at {rel} dB"
        );
    }
}

#[test]
fn transition_band_shape() {
    for (from, to) in RATIOS {
        let at_46 = gain_db(from, to, libm::round(0.46 * fs_min(from, to)));
        assert!(
            (at_46 + 0.05).abs() <= 0.05,
            "{from} -> {to}: {at_46} dB at 0.46"
        );
        let at_fc = gain_db(from, to, libm::round(0.4768 * fs_min(from, to)));
        assert!(
            (at_fc + 6.0).abs() <= 0.1,
            "{from} -> {to}: {at_fc} dB at 0.4768"
        );
    }
}

#[test]
fn thd_n_1khz() {
    for (from, to) in RATIOS {
        let out = run(from, to, &sine(1_000.0, -1.0, SECS, from));
        let start = (SKIP * f64::from(to)) as usize;
        let t = thd_n_db(&out[start..start + to as usize], 1_000.0, to);
        assert!(t <= -100.0, "{from} -> {to}: THD+N {t} dB");
    }
}

#[test]
fn chunking_invariant() {
    for (from, to) in RATIOS {
        let input = noise(11, 0.5, from);
        let whole = run(from, to, &input);
        for chunk in [1, 7, 480, 48_000] {
            let parts = run_chunked(from, to, &input, chunk);
            let same = whole.len() == parts.len()
                && whole
                    .iter()
                    .zip(&parts)
                    .all(|(a, b)| a.to_bits() == b.to_bits());
            assert!(same, "{from} -> {to}, chunks of {chunk}");
        }
    }
}

#[test]
fn identity_ratio_bit_exact() {
    let input = noise(5, 0.25, 48_000);
    for chunk in [1, 480, input.len()] {
        let out = run_chunked(48_000, 48_000, &input, chunk);
        assert_eq!(out.len(), input.len());
        assert!(
            out.iter()
                .zip(&input)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
}

/// `MEDIA_LIMIT` with `details {limit, value, max}`.
fn limit_error(from: u32, to: u32) -> (String, f64, f64) {
    let e = Resampler::new(from, to).err().expect("refused");
    assert_eq!(e.code, "MEDIA_LIMIT", "{from} -> {to}");
    let d = &e.details;
    (
        d["limit"].as_str().unwrap().to_string(),
        d["value"].as_f64().unwrap(),
        d["max"].as_f64().unwrap(),
    )
}

#[test]
fn rate_limits() {
    assert_eq!(
        limit_error(0, 48_000),
        ("sampleRate".into(), 0.0, 768_000.0)
    );
    assert_eq!(
        limit_error(48_000, 0),
        ("sampleRate".into(), 0.0, 768_000.0)
    );
    assert_eq!(
        limit_error(768_001, 48_000),
        ("sampleRate".into(), 768_001.0, 768_000.0)
    );
    assert_eq!(
        limit_error(48_000, 768_001),
        ("sampleRate".into(), 768_001.0, 768_000.0)
    );
    assert!(Resampler::new(768_000, 48_000).is_ok());
    assert!(Resampler::new(768_000, 16_000).is_ok()); // 48:1
    assert!(Resampler::new(705_600, 16_000).is_ok());
    let (limit, value, max) = limit_error(768_000, 15_999);
    assert_eq!((limit.as_str(), max), ("downsampleRatio", 48.0));
    assert!((value - 768_000.0 / 15_999.0).abs() < 1e-9, "{value}");
}

/// Pinned: a change here moves an A0 hash and is re-blessed only with an `AUDIO_VERSION` bump.
#[test]
fn golden_hash() {
    let out = run(44_100, 48_000, &sweep(20.0, 20_000.0, 1.0, 44_100));
    assert_eq!(out.len(), 48_000);
    let bytes: Vec<u8> = out.iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(fin_audio::AUDIO_VERSION, 2);
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        "87d1161c15a5eb499f1aa9ca09fabc01464ada9227ea29ab167f9fef66b815e9"
    );
}
