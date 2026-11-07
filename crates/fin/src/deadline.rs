//! fin's overall deadline (sub-project 5A Decision 36): one `fin` run talks to the server for
//! at most `FIN_DEADLINE_S` seconds (default 300; a decimal such as `0.5` is allowed; `0`
//! turns it off). Each server operation waits for the smaller of its own timeout and what is
//! left; once the deadline passes, the call fails with `SERVER_TIMEOUT {deadline}` (exit 5).
//! Long operations that report their own progress (`fin fonts fetch`, streams) are excluded:
//! their clients are made with [`crate::client::Client::without_deadline`].

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use serde_json::json;

pub const DEADLINE_ENV: &str = "FIN_DEADLINE_S";
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    pub at: Instant,
    pub total: Duration,
}

static DEADLINE: OnceLock<Option<Deadline>> = OnceLock::new();

/// Parses a `FIN_DEADLINE_S` value: `None` for off (`0`), else the duration.
pub fn parse(value: Option<&str>) -> Result<Option<Duration>, CoreError> {
    let Some(text) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(Some(DEFAULT_DEADLINE));
    };
    match text.parse::<f64>() {
        Ok(0.0) => Ok(None),
        Ok(s) if s.is_finite() && s > 0.0 && s <= 86_400.0 * 365.0 => {
            Ok(Some(Duration::from_secs_f64(s)))
        }
        _ => Err(crate::output::usage(
            format!("{DEADLINE_ENV} must be a number of seconds, not {text:?}"),
            json!({ "variable": DEADLINE_ENV, "value": text }),
            format!("set {DEADLINE_ENV} to a positive number of seconds, or 0 for no deadline"),
        )),
    }
}

/// Starts the deadline for this run from `FIN_DEADLINE_S`. Called once at the start of `main`.
pub fn start() -> Result<(), CoreError> {
    let var = std::env::var(DEADLINE_ENV).ok();
    let total = parse(var.as_deref())?;
    let now = Instant::now();
    let _ = DEADLINE.set(total.map(|total| Deadline {
        at: now + total,
        total,
    }));
    Ok(())
}

/// This run's deadline, if `start` set one (never in unit tests).
pub fn current() -> Option<Deadline> {
    DEADLINE.get().copied().flatten()
}

/// The `SERVER_TIMEOUT` of a passed deadline.
pub fn exceeded(deadline: &Deadline, endpoint: &str) -> CoreError {
    CoreError::new(
        "SERVER_TIMEOUT",
        format!(
            "fin's overall deadline of {} s passed",
            deadline.total.as_secs_f64()
        ),
    )
    .with_details(json!({ "deadline": deadline.total.as_secs_f64(), "endpoint": endpoint }))
    .with_fix(format!(
        "check that finisherd is responsive, or raise {DEADLINE_ENV}; retry writes with the same --idem-key"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse() {
        assert_eq!(parse(None).unwrap(), Some(DEFAULT_DEADLINE));
        assert_eq!(parse(Some("")).unwrap(), Some(DEFAULT_DEADLINE));
        assert_eq!(parse(Some("60")).unwrap(), Some(Duration::from_secs(60)));
        assert_eq!(
            parse(Some(" 0.5 ")).unwrap(),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse(Some("0")).unwrap(), None);
        for bad in ["-1", "x", "inf", "NaN", "1e99"] {
            assert_eq!(parse(Some(bad)).unwrap_err().code, "USAGE", "{bad}");
        }
    }
}
