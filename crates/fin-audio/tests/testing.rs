use fin_audio::testing::{analyse::*, signals::*};

const R: u32 = 48_000;

#[test]
fn sine_rms_matches_level() {
    let s = sine(1000.0, -20.0, 1.0, R);
    assert!((rms_dbfs(&s) + 23.01).abs() < 0.01, "{}", rms_dbfs(&s));
    assert!((peak_dbfs(&s) + 20.0).abs() < 0.01);
}

#[test]
fn goertzel_finds_tone() {
    let s = sine(1000.0, -12.0, 1.0, R);
    assert!((goertzel_db(&s, 1000.0, R) + 12.0).abs() < 0.05);
    assert!(goertzel_db(&s, 3000.0, R) < -100.0);
}

#[test]
fn thd_n_of_pure_sine_below_minus_120db() {
    let s = sine(997.0, -6.0, 1.0, R);
    let t = thd_n_db(&s, 997.0, R);
    assert!(t < -120.0, "{t}");
    let mut dirty = s.clone();
    for (i, d) in dirty.iter_mut().enumerate() {
        *d += 0.001 * libm::sin(2.0 * core::f64::consts::PI * 1994.0 * i as f64 / 48000.0) as f32;
    }
    assert!(thd_n_db(&dirty, 997.0, R) > -60.0);
}

#[test]
fn noise_is_reproducible() {
    assert_eq!(noise(7, 0.1, R), noise(7, 0.1, R));
    assert_ne!(noise(7, 0.1, R), noise(8, 0.1, R));
    assert_eq!(pink(7, 0.1, R), pink(7, 0.1, R));
    assert!(noise(1, 0.1, R).iter().all(|v| (-1.0..1.0).contains(v)));
}

#[test]
fn dominant_hz_and_null() {
    let s = sine(440.0, -10.0, 1.0, R);
    assert!((dominant_hz(&s, R) - 440.0).abs() < 0.01);
    assert!(null_db(&s, &s) == f64::NEG_INFINITY);
    let sw = sweep(20.0, 20_000.0, 1.0, R);
    assert_eq!(sw.len(), 48_000);
    assert_eq!(impulse(3, 8), [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(tone_burst(1000.0, 0.0, 0.5, 0.25, 1.0, R).len(), 48_000);
}

#[test]
fn ebu_cases_are_generated() {
    let c = ebu3341_case(3).unwrap();
    assert_eq!(c.len(), 80 * 48_000);
    assert_eq!(ebu3341_case(4).unwrap().len(), 60 * 48_000);
    assert_eq!(ebu3341_case(5).unwrap().len(), 60 * 48_000 + 4_800);
    assert!(ebu3341_case(6).is_none());
    assert_eq!(ebu3342_case(4).unwrap().len(), 100 * 48_000);
}
