//! `fin_core::view::range` and `screenplay` (the plugin host's `project.view`, which has no
//! timeline parameter): frames count at the rate the shown timelines share; over timelines of
//! different rates a frame is `USAGE` (`MIXED_RATES`) whose fix needs no flag.

use fin_core::view::{range, screenplay};
use fin_core::{Rational, new_project};

fn all(p: &fin_core::Project) -> Vec<&fin_core::Timeline> {
    p.timelines.iter().collect()
}

fn r(s: &str) -> Rational {
    s.parse().unwrap()
}

#[test]
fn frames_need_one_rate() {
    let mut p = new_project("p".into(), "P".into(), r("30")).unwrap();
    assert_eq!(range(&all(&p), p.rate, "f30..").unwrap().0, Some(r("1")));
    let mut tl2 = p.timelines[0].clone();
    tl2.id = "tl2".into();
    tl2.rate = r("30000/1001");
    p.timelines.push(tl2);
    assert_eq!(
        range(&[&p.timelines[1]], p.rate, "f30..").unwrap().0,
        Some(r("1001/1000"))
    );
    assert_eq!(
        range(&all(&p), p.rate, "1s..2s").unwrap(),
        (Some(r("1")), Some(r("2")))
    );
    let e = screenplay(&p, Some("f30..f31"), None).unwrap_err();
    assert_eq!(e.code, "USAGE");
    assert_eq!(e.details["reason"], "MIXED_RATES");
    assert_eq!(e.details["rates"][1]["timeline"], "tl2");
    let fix = e.fix.unwrap();
    assert!(
        fix.contains("seconds") && !fix.contains("--") && !fix.contains("show one timeline"),
        "{fix}"
    );
    assert!(screenplay(&p, Some("1s..2s"), None).is_ok());
}
