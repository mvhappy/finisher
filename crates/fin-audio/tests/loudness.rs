//! The BS.1770 meter and the normalisation decision (plan 6C Decision 18, spec §8.6).
//!
//! EBU Tech 3341 cases 1–5 and Tech 3342 cases 1–4 are generated (1 kHz stereo sines). The
//! programme-material cases (3341 cases 7–8) are not generatable and no media is committed, so
//! the true-peak check uses generated tones in the spirit of 3341's true-peak cases: sines at
//! fs/4 whose phase puts the samples below the waveform's crest, so the true peak is known
//! analytically (`20·log10(A)`) while the sample peak is lower.

use fin_audio::loudness::{Limiter, Measured, Meter, normalise};
use fin_audio::testing::signals::*;
use fin_audio::{RATE, Stereo};

const TAU: f64 = 2.0 * core::f64::consts::PI;

/// Measures `s`, pushing uneven blocks (not aligned to the meter's 100 ms hop).
fn measure(s: &Stereo) -> Measured {
    let mut m = Meter::new(2);
    let mut at = 0;
    while at < s.len() {
        let end = (at + 1_237).min(s.len());
        m.push(&Stereo {
            l: s.l[at..end].to_vec(),
            r: s.r[at..end].to_vec(),
        });
        at = end;
    }
    m.measured()
}

fn stereo(l: Vec<f32>, r: Vec<f32>) -> Stereo {
    Stereo { l, r }
}

/// EBU Tech 3341 cases 1–5: integrated within ±0.1 LU (the Tech 3341 tolerance).
#[test]
fn ebu3341_cases() {
    for (n, want) in [(1, -23.0), (2, -33.0), (3, -23.0), (4, -23.0), (5, -23.0)] {
        let got = measure(&ebu3341_case(n).unwrap()).integrated.unwrap();
        assert!(
            (got - want).abs() <= 0.1,
            "case {n}: {got} LUFS, want {want}"
        );
    }
    // Case 5 holds 20.1 s at −20: the short-term maximum is −20 LUFS (±0.1 LU).
    let st = measure(&ebu3341_case(5).unwrap()).short_term_max.unwrap();
    assert!((st + 20.0).abs() <= 0.1, "short-term max {st}");
}

/// A sine at fs/4 with phase `deg`, scaled so its sample peak is `sample_dbfs`; returns the
/// signal and its analytic true peak in dBTP.
fn quarter_rate_tone(deg: f64, sample_dbfs: f64, secs: f64) -> (Vec<f32>, f64) {
    let phi = deg * TAU / 360.0;
    let crest = libm::fabs(libm::sin(phi)).max(libm::fabs(libm::cos(phi)));
    let a = db_to_amp(sample_dbfs) / crest;
    let n = (secs * f64::from(RATE)) as usize;
    let x = (0..n)
        .map(|i| (a * libm::sin(TAU * i as f64 / 4.0 + phi)) as f32)
        .collect();
    (x, 20.0 * libm::log10(a))
}

/// The true-peak substitute for 3341's programme-material cases: the meter's true peak within
/// +0.2 / −0.4 dB of the analytic value (Tech 3341's true-peak tolerance), and the sample peak
/// is the generated one (±0.01 dB).
#[test]
fn ebu3341_true_peak_tones() {
    for deg in [0.0, 45.0, 60.0, 67.5, 90.0] {
        let (x, tp) = quarter_rate_tone(deg, -6.0, 2.0);
        let m = measure(&stereo(x.clone(), x));
        let got = m.true_peak.unwrap();
        assert!(
            got - tp <= 0.2 && tp - got <= 0.4,
            "{deg}°: {got} dBTP, analytic {tp}"
        );
        let sp = m.sample_peak.unwrap();
        assert!((sp + 6.0).abs() <= 0.01, "{deg}°: sample peak {sp}");
    }
}

/// EBU Tech 3342 cases 1–4: loudness range within ±1 LU of the documented values.
#[test]
fn ebu3342_cases() {
    for (n, want) in [(1, 10.0), (2, 5.0), (3, 20.0), (4, 15.0)] {
        let got = measure(&ebu3342_case(n).unwrap()).range;
        assert!(
            (got - want).abs() <= 1.0,
            "case {n}: LRA {got}, want {want}"
        );
    }
}

#[test]
fn silence_is_minus_inf() {
    let m = measure(&Stereo::silence(5 * RATE as usize));
    assert_eq!(m.integrated, None);
    assert_eq!(m.true_peak, None);
    assert_eq!(m.sample_peak, None);
    assert_eq!(m.short_term_max, None);
    assert!(m.range == 0.0, "range {}", m.range);
    // Nothing measured at all is the same.
    let empty = Meter::new(2).measured();
    assert_eq!(empty.integrated, None);
    assert!(!empty.range.is_nan());
}

fn scaled(s: &Stereo, g: f32) -> Stereo {
    Stereo {
        l: s.l.iter().map(|v| v * g).collect(),
        r: s.r.iter().map(|v| v * g).collect(),
    }
}

/// Applies a decision: the static gain, then (when asked) the limiter.
fn apply(s: &Stereo, gain: f32, limit: bool, ceiling: f64) -> Stereo {
    let mut out = scaled(s, gain);
    if limit {
        let mut lim = Limiter::new(ceiling);
        lim.process(&mut out);
        lim.finish(&mut out);
    }
    out
}

/// Pink noise (L and R from different seeds) with a decaying 1 kHz click of peak `click`
/// every second (0 for none).
fn pink_with_clicks(secs: f64, click: f64) -> Stereo {
    let add = |x: &mut Vec<f32>| {
        for start in (0..x.len()).step_by(RATE as usize) {
            for i in 0..(RATE as usize / 50).min(x.len() - start) {
                let t = i as f64 / f64::from(RATE);
                let v = click * libm::exp(-t / 0.003) * libm::sin(TAU * 1_000.0 * t);
                x[start + i] += v as f32;
            }
        }
    };
    let (mut l, mut r) = (pink(11, secs, RATE), pink(12, secs, RATE));
    add(&mut l);
    add(&mut r);
    stereo(l, r)
}

/// Pink noise at −30 LUFS normalised to −14 LUFS / −1 dBTP, alone (it peaks below the ceiling
/// after the gain) and with clicks that need the limiter: after the gain and the limiter the
/// result is −14 ± 0.2 LUFS and the true peak at most −1 + 0.1 dBTP.
#[test]
fn normalise_reaches_target() {
    for (click, limits) in [(0.0, false), (1.0, true)] {
        let raw = pink_with_clicks(20.0, click);
        let m0 = measure(&raw).integrated.unwrap();
        let input = scaled(&raw, db_to_amp(-30.0 - m0) as f32);
        let m = measure(&input);
        assert!((m.integrated.unwrap() + 30.0).abs() < 0.05);
        let d = normalise(&m, -14.0, -1.0);
        assert!(d.warning.is_none(), "{:?}", d.warning);
        assert_eq!(d.limit, limits, "click {click}: {m:?} {d:?}");
        // The limiter always runs here (it is transparent below the ceiling).
        let after = measure(&apply(&input, d.linear(), true, -1.0));
        let i = after.integrated.unwrap();
        assert!((i + 14.0).abs() <= 0.2, "click {click}: {i} LUFS");
        let tp = after.true_peak.unwrap();
        assert!(tp <= -0.9, "click {click}: {tp} dBTP");
    }
}

fn measured(integrated: f64, true_peak: f64) -> Measured {
    Measured {
        integrated: Some(integrated),
        range: 5.0,
        true_peak: Some(true_peak),
        sample_peak: Some(true_peak - 0.5),
        short_term_max: Some(integrated + 3.0),
    }
}

#[test]
fn normalise_gain_quantised_to_hundredths() {
    let d = normalise(&measured(-23.456, -20.0), -14.0, -1.0);
    assert!((d.gain_db - 9.46).abs() < 1e-9, "{}", d.gain_db);
    assert!(!d.limit && d.warning.is_none());
    let d = normalise(&measured(-14.004_9, -5.0), -14.0, -1.0);
    assert_eq!(d.gain_db, 0.0);
    let d = normalise(&measured(-10.333_3, -2.0), -14.0, -1.0);
    assert!((d.gain_db + 3.67).abs() < 1e-9, "{}", d.gain_db);
    assert!((d.linear() - db_to_amp(-3.67) as f32).abs() < 1e-6);
}

#[test]
fn normalise_silence_warns() {
    for m in [
        Meter::new(2).measured(),
        Measured {
            integrated: Some(-75.0),
            ..measured(-75.0, -60.0)
        },
    ] {
        let d = normalise(&m, -14.0, -1.0);
        assert_eq!(d.gain_db, 0.0);
        assert!(!d.limit);
        let w = d.warning.expect("a warning");
        assert_eq!(w.code, "LOUDNESS_TARGET_UNREACHABLE");
        assert_eq!(w.details["target"], -14.0);
        assert_eq!(
            w.details["measured"],
            m.integrated.map_or(serde_json::Value::Null, |v| v.into())
        );
    }
}

/// A sine (whose loudness equals its peak) raised to 0 LUFS under a −1 dBTP ceiling: the
/// limiter can hold it only by about 1.5 LU of reduction, so the decision warns; running it
/// confirms the result misses the target by more than 1 LU. Within 1 LU it does not warn.
#[test]
fn normalise_unreachable_beyond_1lu_warns() {
    let x = sine(1000.0, -20.0, 10.0, RATE);
    let s = stereo(x.clone(), x);
    let m = measure(&s);
    let d = normalise(&m, 0.5, -1.0);
    assert!(d.limit);
    let w = d.warning.clone().expect("a warning");
    assert_eq!(w.code, "LOUDNESS_TARGET_UNREACHABLE");
    assert_eq!(w.details["target"], 0.5);
    assert_eq!(w.details["measured"], m.integrated.unwrap());
    let i = measure(&apply(&s, d.linear(), d.limit, -1.0))
        .integrated
        .unwrap();
    assert!(0.5 - i > 1.0, "{i} LUFS");
    // −0.5 LUFS needs about 0.5 LU: limited, no warning.
    let d = normalise(&m, -0.5, -1.0);
    assert!(d.limit && d.warning.is_none(), "{d:?}");
}
