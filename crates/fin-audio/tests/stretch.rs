//! WSOLA time-stretch (plan 6C Task 4, Decision 6).

use fin_audio::testing::{analyse::*, signals::*};
use fin_audio::{RATE, Stereo, Wsola, sample_at};
use fin_core::Rational;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn stereo(l: Vec<f32>, r: Vec<f32>) -> Stereo {
    Stereo { l, r }
}

fn dual(x: Vec<f32>) -> Stereo {
    stereo(x.clone(), x)
}

/// Stretches `input` fed in chunks of `chunk` frames, then finishes.
fn stretch(speed: Rational, input: &Stereo, chunk: usize) -> Stereo {
    let mut w = Wsola::new(speed);
    let mut out = Stereo::default();
    let mut at = 0;
    while at < input.len() {
        let end = (at + chunk).min(input.len());
        let part = stereo(input.l[at..end].to_vec(), input.r[at..end].to_vec());
        w.process(&part, &mut out);
        at = end;
    }
    w.finish(&mut out);
    out
}

/// The output length a source of `frames` frames plays for at `speed`: `sample_at(duration)`.
fn expected_len(frames: usize, speed: Rational) -> usize {
    let d = r(frames as i64, i64::from(RATE))
        .checked_div(speed)
        .unwrap();
    sample_at(d).unwrap() as usize
}

/// Drops `edge` frames at both ends (start-up and the hard end of the source).
fn interior(x: &[f32], edge: usize) -> &[f32] {
    &x[edge..x.len() - edge]
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// Speech-like: pink noise in 200 ms raised-cosine "syllables" every 300 ms, plus a 140 Hz
/// voiced buzz (three harmonics) under the same envelope.
fn speechlike(secs: f64) -> Stereo {
    let n = pink(11, secs, RATE);
    let tau = 2.0 * core::f64::consts::PI;
    let x: Vec<f32> = n
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = i as f64 / f64::from(RATE);
            let ph = t % 0.3;
            let env = if ph < 0.2 {
                libm::pow(libm::sin(core::f64::consts::PI * ph / 0.2), 2.0)
            } else {
                0.0
            };
            let buzz = (1..=3)
                .map(|h| libm::sin(tau * 140.0 * f64::from(h) * t) / f64::from(h))
                .sum::<f64>();
            (env * (0.6 * f64::from(v) + 0.2 * buzz)) as f32
        })
        .collect();
    let right: Vec<f32> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| if i % 2 == 0 { v } else { 0.9 * v })
        .collect();
    stereo(x, right)
}

#[test]
fn speed_one_bypasses_bit_exact() {
    let input = stereo(noise(1, 0.5, RATE), pink(2, 0.5, RATE));
    for chunk in [1, 333, 48_000] {
        let out = stretch(r(1, 1), &input, chunk);
        assert!(out == input, "chunk {chunk}: {} frames out", out.len());
    }
    let w = Wsola::new(r(1, 1));
    assert_eq!(w.input_needed(1000), 1000);
    assert_eq!(w.output_len(1000), 1000);
}

#[test]
fn length_exact_at_speeds() {
    // (speed, input frames): the long-output speeds get short inputs to keep the test quick.
    let cases = [
        (r(1, 2), 4_801),
        (r(3, 2), 9_601),
        (r(2, 1), 9_599),
        (r(1001, 1000), 4_801),
        (r(1, 100), 481),
        (r(100, 1), 48_007),
        (r(1, 1), 1_234),
    ];
    for (speed, n) in cases {
        let input = dual(noise(3, n as f64 / f64::from(RATE), RATE));
        assert_eq!(input.len(), n);
        let want = expected_len(n, speed);
        let w = Wsola::new(speed);
        assert_eq!(w.output_len(n as u64), want as u64, "output_len at {speed}");
        for chunk in [n, 1_000] {
            let out = stretch(speed, &input, chunk);
            assert_eq!(out.len(), want, "speed {speed}, chunk {chunk}");
        }
    }
}

#[test]
fn pitch_preserved() {
    let input = dual(sine(440.0, -6.0, 1.0, RATE));
    for speed in [r(1, 2), r(3, 2), r(2, 1)] {
        let out = stretch(speed, &input, 4_096);
        let hz = dominant_hz(interior(&out.l, 2_048), RATE);
        assert!(
            (hz - 440.0).abs() <= 440.0 * 0.005,
            "speed {speed}: {hz} Hz"
        );
        assert!(out.l == out.r);
    }
}

#[test]
fn level_preserved() {
    let speed = r(3, 2);
    for (name, x) in [
        ("sine", sine(440.0, -6.0, 1.0, RATE)),
        ("chord", {
            let (a, b, c) = (
                sine(220.0, -12.0, 1.0, RATE),
                sine(330.0, -12.0, 1.0, RATE),
                sine(550.0, -12.0, 1.0, RATE),
            );
            a.iter()
                .zip(&b)
                .zip(&c)
                .map(|((x, y), z)| x + y + z)
                .collect()
        }),
    ] {
        let input = dual(x);
        let out = stretch(speed, &input, 4_096);
        let (i, o) = (
            rms_dbfs(interior(&input.l, 2_048)),
            rms_dbfs(interior(&out.l, 2_048)),
        );
        assert!((o - i).abs() <= 0.5, "{name}: in {i:.3} dB, out {o:.3} dB");
    }
}

#[test]
fn no_clicks_on_sine() {
    let input = dual(sine(440.0, -6.0, 1.0, RATE));
    let out = stretch(r(3, 2), &input, 4_096);
    // The last 1,536 output frames read past the source's hard end (zeros), like any hard cut.
    let body = &out.l[..out.len() - 1_536];
    let (si, so) = (max_step(&input.l), max_step(body));
    assert!(so <= 1.2 * si, "output step {so}, input step {si}");
}

#[test]
fn stereo_offsets_shared() {
    let x = speechlike(1.0);
    // L and R identical in → identical out.
    let out = stretch(r(3, 2), &dual(x.l.clone()), 4_096);
    assert!(out.l == out.r, "L and R differ");
    // One offset for both channels: R = L/2 in → R = L/2 out (the halving is exact in f32).
    let half: Vec<f32> = x.l.iter().map(|v| v * 0.5).collect();
    let out = stretch(r(3, 2), &stereo(x.l.clone(), half), 4_096);
    let want: Vec<f32> = out.l.iter().map(|v| v * 0.5).collect();
    assert!(out.r == want, "R is not L/2");
    // The channels are symmetric: swapping them swaps the output.
    let a = stretch(r(3, 2), &x, 4_096);
    let b = stretch(r(3, 2), &stereo(x.r.clone(), x.l.clone()), 4_096);
    assert!(
        a.l == b.r && a.r == b.l,
        "swapped input does not swap output"
    );
}

#[test]
fn chunking_invariant() {
    let input = speechlike(0.6);
    for speed in [r(3, 2), r(1, 2), r(1001, 1000)] {
        let whole = stretch(speed, &input, input.len());
        for chunk in [1, 127, 4_096] {
            assert!(
                stretch(speed, &input, chunk) == whole,
                "speed {speed}, chunk {chunk}"
            );
        }
    }
}

#[test]
fn source_at_is_decision_4() {
    // Output frame k plays source frame round_half_up(k × speed): Decision 4's mapping, in
    // frames relative to the item's start, the same number the mixer computes with sample_at.
    for speed in [r(3, 2), r(1001, 1000), r(1, 3), r(100, 1)] {
        let w = Wsola::new(speed);
        for k in [0u64, 1, 2, 511, 512, 30_000, 1_000_003] {
            let t = r(k as i64, i64::from(RATE)).checked_mul(speed).unwrap();
            assert_eq!(
                w.source_at(k),
                sample_at(t).unwrap(),
                "speed {speed}, k {k}"
            );
        }
    }
}

#[test]
fn input_needed_drives_emission() {
    // Pushing exactly input_needed(k) source frames emits at least k output frames; one frame
    // less emits fewer. So the mixer knows, before reading, which source frames k output frames need.
    let x = speechlike(1.0);
    for speed in [r(3, 2), r(1, 2), r(7, 3)] {
        for k in [1u64, 512, 513, 4_000] {
            let w0 = Wsola::new(speed);
            let need = w0.input_needed(k) as usize;
            assert!(need <= x.len(), "speed {speed}, k {k}: {need}");
            for (push, enough) in [(need, true), (need - 1, false)] {
                let mut w = Wsola::new(speed);
                let mut out = Stereo::default();
                w.process(
                    &stereo(x.l[..push].to_vec(), x.r[..push].to_vec()),
                    &mut out,
                );
                assert_eq!(w.pushed(), push as u64);
                assert_eq!(w.emitted(), out.len() as u64);
                assert_eq!(
                    out.len() as u64 >= k,
                    enough,
                    "speed {speed}, k {k}, pushed {push}"
                );
            }
        }
        // input_needed never decreases.
        let w = Wsola::new(speed);
        assert!((0..5_000).all(|k| w.input_needed(k) <= w.input_needed(k + 1)));
    }
}

#[test]
fn golden_hash() {
    let out = stretch(r(3, 2), &speechlike(2.0), 4_096);
    assert_eq!(out.len(), expected_len(96_000, r(3, 2)));
    let mut h = blake3::Hasher::new();
    for (l, r) in out.l.iter().zip(&out.r) {
        h.update(&l.to_le_bytes());
        h.update(&r.to_le_bytes());
    }
    // Bumping this hash bumps AUDIO_VERSION (Global Constraints).
    assert_eq!(
        h.finalize().to_hex().as_str(),
        "2ee0c52837eefdcd5e84316ed3f687bfd106378258c143b8ff68324ab81c83b0"
    );
}
