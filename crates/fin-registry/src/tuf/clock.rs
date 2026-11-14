//! The clock check (8A Decision 13) over the crate's one injected [`Clock`]
//! ([`crate::clock`]), re-exported here for the TUF client's callers.

use std::time::SystemTime;

pub use crate::clock::{Clock, FixedClock, SystemClock};

pub(crate) fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// How far the clock may be behind the last successful refresh (8A Decision 13).
pub const CLOCK_SLACK_SECS: i64 = 24 * 3600;

/// `CLOCK_SUSPECT` when `now` is more than a day before `last_refresh_at`.
pub(crate) fn check(now: SystemTime, last_refresh_at: Option<i64>) -> Result<(), super::TufError> {
    let Some(last) = last_refresh_at else {
        return Ok(());
    };
    let now = unix_secs(now);
    if now + CLOCK_SLACK_SECS >= last {
        return Ok(());
    }
    Err(super::TufError::new(
        "CLOCK_SUSPECT",
        format!(
            "the clock is {} hour(s) behind the last refresh; fix the clock, then refresh",
            (last - now) / 3600
        ),
        serde_json::json!({ "now": now, "lastRefreshAt": last }),
    ))
}
