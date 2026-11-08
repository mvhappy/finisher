//! Expiry with the injected clock (spec §6.3, 8A Decision 12). `tough` loads with expiry
//! enforcement off (`Unsafe`, which the 8A Task 1 gate showed skips only expiry checks), and
//! this applies the policy to every trusted role: root, timestamp, snapshot, targets and
//! each publisher's role.
//!
//! - `strict`: any expired role is `TUF_EXPIRED {role}`;
//! - `relaxed`: expired by more than `maxStaleDays` days is `TUF_EXPIRED {role, days,
//!   maxStaleDays}`; within it, the refresh succeeds with `REGISTRY_STALE {role, days,
//!   maxStaleDays}`.

use std::time::SystemTime;

use serde_json::json;

use super::clock::unix_secs;
use super::custom;
use super::trusted::Trusted;
use super::{Freshness, Policy, TufError, Warning};

const DAY: i64 = 86_400;

/// The earliest expiry of the trusted roles: `(role, unix seconds)`.
pub(crate) fn earliest(t: &Trusted) -> (String, i64) {
    let mut all: Vec<(String, i64)> = vec![("root".into(), t.root.signed.expires.as_second())];
    if let Some(s) = &t.timestamp {
        all.push(("timestamp".into(), s.signed.expires.as_second()));
    }
    if let Some(s) = &t.snapshot {
        all.push(("snapshot".into(), s.signed.expires.as_second()));
    }
    if let Some(s) = &t.targets {
        all.push(("targets".into(), s.signed.expires.as_second()));
    }
    for (name, s) in &t.roles {
        all.push((name.clone(), s.signed.expires.as_second()));
    }
    all.into_iter()
        .min_by_key(|(_, e)| *e)
        .expect("the root is always there")
}

/// Whole days (rounded up) `now` is past the earliest expiry, or `None` while fresh.
pub(crate) fn stale_days(t: &Trusted, now: SystemTime) -> Option<u16> {
    let (_, e) = earliest(t);
    let over = unix_secs(now) - e;
    (over > 0).then(|| ((over + DAY - 1) / DAY).min(i64::from(u16::MAX)) as u16)
}

/// Applies `policy` at `now`: `Ok(None)` when fresh, `Ok(Some(REGISTRY_STALE))` when
/// relaxed and within `maxStaleDays`, `TUF_EXPIRED` otherwise.
pub(crate) fn check(
    t: &Trusted,
    policy: &Policy,
    now: SystemTime,
) -> Result<Option<Warning>, TufError> {
    let (role, e) = earliest(t);
    let over = unix_secs(now) - e;
    if over <= 0 {
        return Ok(None);
    }
    let max = custom::max_stale_days(&t.root.signed);
    let days = stale_days(t, now).unwrap_or(0);
    match policy.freshness {
        Freshness::Strict => Err(TufError::new(
            "TUF_EXPIRED",
            format!("{role} metadata expired {days} day(s) ago"),
            json!({ "role": role, "days": days }),
        )),
        Freshness::Relaxed if over > i64::from(max) * DAY => Err(TufError::new(
            "TUF_EXPIRED",
            format!("{role} metadata expired {days} day(s) ago, more than the {max} allowed"),
            json!({ "role": role, "days": days, "maxStaleDays": max }),
        )),
        Freshness::Relaxed => {
            let serde_json::Value::Object(details) =
                json!({ "role": role, "days": days, "maxStaleDays": max })
            else {
                unreachable!("an object")
            };
            Ok(Some(Warning {
                code: "REGISTRY_STALE",
                details,
            }))
        }
    }
}
