//! Freshness with the injected clock and the clock check (8A Task 4b, Decisions 12–13).

mod support;

use std::time::Duration;

use fin_registry::editor::Role;
use fin_registry::tuf::{
    Clock, FixedClock, Freshness, Policy, RefreshReport, TrustedRepo, TufError,
};
use support::tuf::Reg;
use tough::schema::{Signed, Timestamp};

const DAY: u64 = 86_400;

/// A registry whose timestamp expires a day after the editor's clock, and a client clock.
fn setup(max_stale_days: Option<u16>) -> (Reg, TrustedRepo, FixedClock) {
    let mut r = Reg::new(max_stale_days);
    r.b.resign_timestamp(1).unwrap();
    let c = r.client();
    let clock = FixedClock::new(r.clock.now());
    (r, c, clock)
}

fn at(
    r: &Reg,
    c: &mut TrustedRepo,
    clock: &FixedClock,
    policy: Freshness,
) -> Result<RefreshReport, TufError> {
    r.refresh_with(c, r.transport(), Policy { freshness: policy }, clock)
}

#[test]
fn stale_within_max_stale_days_warns() {
    let (r, mut c, clock) = setup(None);
    let rep = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    assert!(rep.warnings.is_empty() && rep.stale_days.is_none());
    clock.advance_days(4);
    let rep = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    assert_eq!(rep.stale_days, Some(3));
    let w = &rep.warnings[0];
    assert_eq!(w.code, "REGISTRY_STALE");
    assert_eq!(
        (
            w.details["role"].as_str(),
            w.details["maxStaleDays"].as_u64()
        ),
        (Some("timestamp"), Some(7))
    );
    assert_eq!(c.stale_days(&clock), Some(3));
}

#[test]
fn expired_beyond_max_stale_days_is_tuf_expired() {
    let (r, mut c, clock) = setup(None);
    // Expired by exactly 7 days: still stale.
    clock.set(clock.now() + Duration::from_secs(8 * DAY));
    let rep = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    assert_eq!(rep.warnings[0].code, "REGISTRY_STALE");
    clock.set(clock.now() + Duration::from_secs(1));
    let before = Reg::files(c.store().dir());
    let e = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err();
    assert_eq!(
        (e.code, e.role()),
        ("TUF_EXPIRED", Some("timestamp")),
        "{e}"
    );
    // The failed refresh changed nothing (not even lastRefreshAt).
    assert_eq!(Reg::files(c.store().dir()), before);
    // The status read of the trusted set agrees.
    assert_eq!(
        c.freshness(&Policy::default(), &clock).unwrap_err().code,
        "TUF_EXPIRED"
    );
}

#[test]
fn max_stale_days_comes_from_the_signed_root() {
    for (set, effective) in [(30u16, 30u64), (200, 90)] {
        let (r, mut c, clock) = setup(Some(set));
        assert_eq!(u64::from(c.max_stale_days()), effective);
        clock.advance_days(effective as i64 + 1);
        let rep = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
        assert_eq!(
            rep.warnings[0].details["maxStaleDays"].as_u64(),
            Some(effective),
            "{set}"
        );
        clock.advance_days(1);
        let e = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err();
        assert_eq!(e.code, "TUF_EXPIRED", "{set}");
    }
}

#[test]
fn strict_refuses_any_expiry() {
    let (r, mut c, clock) = setup(None);
    at(&r, &mut c, &clock, Freshness::Strict).unwrap();
    clock.advance_days(2);
    let e = at(&r, &mut c, &clock, Freshness::Strict).unwrap_err();
    assert_eq!((e.code, e.role()), ("TUF_EXPIRED", Some("timestamp")));
}

#[test]
fn publisher_roles_count_for_expiry() {
    let (mut r, mut c, clock) = setup(None);
    let acme = r.publisher("acme");
    r.b.expiry.targets = 2;
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.resign_timestamp(30).unwrap();
    // `targets` and `acme` were both signed for 2 days: one of them is the earliest.
    clock.advance_days(12);
    let e = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err();
    assert_eq!(e.code, "TUF_EXPIRED");
    assert!(matches!(e.role(), Some("targets" | "acme")), "{e}");
}

#[test]
fn unsafe_expiry_only() {
    let (mut r, mut c, clock) = setup(None);
    r.b.set_next_version(&Role::Timestamp, 5);
    r.b.resign_timestamp(1).unwrap();
    at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    clock.advance_days(3);
    // Expired and badly signed: refused for the signature, not reported as stale.
    let p = r.meta_path("timestamp.json");
    let good = std::fs::read(&p).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    let sig = v["signatures"][0]["sig"].as_str().unwrap().to_string();
    let flipped = format!(
        "{}{}",
        if sig.starts_with('0') { '1' } else { '0' },
        &sig[1..]
    );
    v["signatures"][0]["sig"] = flipped.into();
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(
        at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err().code,
        "TUF_SIGNATURE"
    );
    // Expired and an unsigned edit of the body.
    let mut v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    v["signed"]["version"] = 99.into();
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(
        at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err().code,
        "TUF_SIGNATURE"
    );
    // Expired and older: still a rollback.
    std::fs::write(&p, &good).unwrap();
    r.b.set_next_version(&Role::Timestamp, 4);
    r.b.resign_timestamp(1).unwrap();
    assert_eq!(
        at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err().code,
        "TUF_ROLLBACK"
    );
    let _: Signed<Timestamp> = r.meta("timestamp.json");
}

#[test]
fn clock_suspect_changes_nothing() {
    let (mut r, mut c, clock) = setup(None);
    at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    let last = c.last_refresh_at().unwrap().unwrap();
    r.b.resign_timestamp(30).unwrap();
    // 23 hours back is within the slack.
    clock.set(clock.now() - Duration::from_secs(23 * 3600));
    at(&r, &mut c, &clock, Freshness::Relaxed).unwrap();
    assert_eq!(
        c.last_refresh_at().unwrap(),
        Some(last),
        "the watermark went back"
    );
    // More than a day back: CLOCK_SUSPECT, and the new timestamp is not trusted.
    r.b.resign_timestamp(30).unwrap();
    let ts = c.versions().1;
    clock.set(clock.now() - Duration::from_secs(2 * 3600));
    let before = Reg::files(c.store().dir());
    let e = at(&r, &mut c, &clock, Freshness::Relaxed).unwrap_err();
    assert_eq!(e.code, "CLOCK_SUSPECT");
    assert_eq!(e.details["lastRefreshAt"].as_i64(), Some(last));
    assert_eq!(Reg::files(c.store().dir()), before);
    assert_eq!(c.versions().1, ts);
    // Fixed clock: the refresh goes through.
    clock.set(clock.now() + Duration::from_secs(26 * 3600));
    assert!(at(&r, &mut c, &clock, Freshness::Relaxed).unwrap().changed);
}
