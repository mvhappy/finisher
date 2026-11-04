use fin_core::{CoreError, Rational, frame_duration, parse_time};
use proptest::prelude::*;

fn r(s: &str) -> Rational {
    s.parse().unwrap()
}
fn rate(n: i64) -> Rational {
    Rational::from_int(n)
}
fn code(e: Result<Rational, CoreError>) -> String {
    e.unwrap_err().code
}

#[test]
fn reduces() {
    assert_eq!(r("2/4").to_string(), "1/2");
    assert_eq!(r("4/2").to_string(), "2");
    assert_eq!(Rational::new(3, -6).unwrap().to_string(), "-1/2");
}

#[test]
fn parses_units_and_clocks() {
    assert_eq!(parse_time("12.4s", rate(30)).unwrap(), r("62/5"));
    assert_eq!(parse_time("12s", rate(30)).unwrap(), r("12"));
    assert_eq!(parse_time("00:12.4", rate(30)).unwrap(), r("62/5"));
    assert_eq!(parse_time("01:00:00", rate(25)).unwrap(), r("3600"));
    assert_eq!(parse_time("01:00:12.4", rate(25)).unwrap(), r("18062/5"));
    assert_eq!(parse_time("-0.5s", rate(30)).unwrap(), r("-1/2"));
    assert_eq!(parse_time("1001/30000", rate(30)).unwrap(), r("1001/30000"));
    assert_eq!(parse_time("62/5", rate(30)).unwrap(), r("62/5"));
    assert_eq!(parse_time("12", rate(30)).unwrap(), r("12"));
}

#[test]
fn frames_use_the_rate() {
    let ntsc = Rational::new(30000, 1001).unwrap();
    let t = parse_time("f300", ntsc).unwrap();
    assert_eq!(t, r("1001/100"));
    assert_eq!(t.to_string(), "1001/100");
    assert_eq!(frame_duration(ntsc).unwrap(), r("1001/30000"));
}

#[test]
fn rejects_bad_input_with_details() {
    for bad in [
        "1e3s", "12.4", "abc", "1/0", "", "1.5/2", "f", "1:2:3:4", "12.s", ".5s",
    ] {
        let e = parse_time(bad, rate(30)).unwrap_err();
        assert_eq!(e.code, "INVALID_TIME", "{bad}");
        assert_eq!(e.details["input"], bad);
    }
}

#[test]
fn serde_round_trip() {
    let v: Rational = serde_json::from_str("\"1001/30000\"").unwrap();
    assert_eq!(serde_json::to_string(&v).unwrap(), "\"1001/30000\"");
    assert_eq!(serde_json::to_string(&rate(12)).unwrap(), "\"12\"");
    assert!(serde_json::from_str::<Rational>("12.5").is_err());
    assert!(serde_json::from_str::<Rational>("\"1.5\"").is_err());
}

#[test]
fn overflow_is_typed() {
    let max = Rational::from_int(i64::MAX);
    assert_eq!(code(max + max), "TIME_OVERFLOW");
    assert_eq!(code(-Rational::from_int(i64::MIN)), "TIME_OVERFLOW");
    assert_eq!(code("99999999999999999999999".parse()), "TIME_OVERFLOW");
}

#[test]
fn clock_fields_after_the_first_are_below_60() {
    // The leading field is unbounded: 90 minutes, or 100 hours.
    assert_eq!(parse_time("90:00", rate(30)).unwrap(), r("5400"));
    assert_eq!(parse_time("100:00:00", rate(30)).unwrap(), r("360000"));
    assert_eq!(parse_time("00:59.999", rate(30)).unwrap(), r("59999/1000"));
    assert_eq!(parse_time("01:59:59", rate(30)).unwrap(), r("7199"));
    for bad in [
        "00:60",
        "00:75",
        "00:60.0",
        "00:59.9999:1",
        "01:60:00",
        "01:00:60",
        "-00:61",
    ] {
        let e = parse_time(bad, rate(30)).unwrap_err();
        assert_eq!(e.code, "INVALID_TIME", "{bad}");
        assert_eq!(e.details["input"], bad);
        assert!(e.fix.is_some(), "{bad}");
    }
    let e = parse_time("01:75:00", rate(30)).unwrap_err();
    assert!(
        e.message.contains("minutes must be below 60"),
        "{}",
        e.message
    );
    let e = parse_time("00:75", rate(30)).unwrap_err();
    assert!(
        e.message.contains("seconds must be below 60"),
        "{}",
        e.message
    );
}

#[test]
fn parse_overflow_names_the_input_and_has_a_fix() {
    let huge = "99999999999999999999999";
    for input in [
        format!("{huge}s"),
        format!("{huge}:00"),
        format!("{huge}{huge}:00:00"),
        format!("f{huge}"),
        huge.to_string(),
    ] {
        let e = parse_time(&input, rate(30)).unwrap_err();
        assert_eq!(e.code, "TIME_OVERFLOW", "{input}");
        assert_eq!(e.details["input"], input.as_str());
        assert!(e.fix.is_some(), "{input}");
    }
    let e = huge.parse::<Rational>().unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["input"]),
        ("TIME_OVERFLOW", &serde_json::json!(huge))
    );
    let max = Rational::from_int(i64::MAX);
    assert!((max + max).unwrap_err().fix.is_some());
}

#[test]
fn orders_by_value() {
    assert!(r("1/3") < r("1/2"));
    assert!(r("-1/2") < r("-1/3"));
}

proptest! {
    #[test]
    fn add_then_sub_is_identity(an in -1_000_000i64..1_000_000, ad in 1i64..10_000, bn in -1_000_000i64..1_000_000, bd in 1i64..10_000) {
        let a = Rational::new(an, ad).unwrap();
        let b = Rational::new(bn, bd).unwrap();
        prop_assert_eq!(((a + b).unwrap() - b).unwrap(), a);
    }
}

#[test]
fn frame_duration_rejects_non_positive_rates() {
    for bad in [rate(0), rate(-30)] {
        let e = frame_duration(bad).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
    }
}
