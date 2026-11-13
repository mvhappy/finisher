use fin_audio::sample_at;
use fin_core::Rational;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

#[test]
fn sample_at_rounds_half_up() {
    assert_eq!(sample_at(r(1, 96_000)).unwrap(), 1); // 0.5 rounds up
    assert_eq!(sample_at(r(1001, 30_000)).unwrap(), 1_602); // 1601.6
    assert_eq!(sample_at(r(-1, 96_000)).unwrap(), 0); // -0.5 rounds up to 0
    assert_eq!(sample_at(r(-3, 96_000)).unwrap(), -1); // -1.5 rounds up to -1
    assert_eq!(sample_at(r(1, 1)).unwrap(), 48_000);
    assert_eq!(sample_at(r(0, 1)).unwrap(), 0);
}

#[test]
fn sample_at_overflow_is_time_overflow() {
    let e = sample_at(r(i64::MAX, 1)).unwrap_err();
    assert_eq!(e.code, "TIME_OVERFLOW");
    let e = sample_at(r(i64::MIN, 1)).unwrap_err();
    assert_eq!(e.code, "TIME_OVERFLOW");
}

#[test]
fn sample_at_i64_boundary() {
    // i64::MAX / 48000 = 192_153_584_101_141.1: the last whole second that fits, then the next.
    let last = i64::MAX / 48_000;
    assert_eq!(sample_at(r(last, 1)).unwrap(), last * 48_000);
    assert_eq!(sample_at(r(last + 1, 1)).unwrap_err().code, "TIME_OVERFLOW");
    // A fraction past the last fitting second, rounding up to a sample that still fits.
    assert_eq!(sample_at(r(i64::MAX, 48_000)).unwrap(), i64::MAX);
}
