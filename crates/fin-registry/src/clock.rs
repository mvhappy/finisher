//! The injected wall clock (8A Global Constraints, Decisions 12–13): expiry, freshness, the
//! clock check, plans and the store never read the system clock directly. One trait serves the
//! TUF client (`now`, a `SystemTime`: `tough` reads the system clock itself, so it loads with
//! expiry enforcement off and every check here uses this clock instead) and the plans and the
//! store (`now_ms`, Unix milliseconds).

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;

    /// Milliseconds since the Unix epoch (0 before it).
    fn now_ms(&self) -> u64 {
        self.now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock tests set and move (and callers that already read the time).
#[derive(Debug)]
pub struct FixedClock(Mutex<SystemTime>);

impl FixedClock {
    pub fn new(at: SystemTime) -> FixedClock {
        FixedClock(Mutex::new(at))
    }

    /// A clock at `ms` milliseconds since the Unix epoch.
    pub fn at_ms(ms: u64) -> FixedClock {
        FixedClock::new(UNIX_EPOCH + Duration::from_millis(ms))
    }

    /// The system time now, frozen.
    pub fn now_frozen() -> FixedClock {
        FixedClock::new(SystemTime::now())
    }

    pub fn set(&self, at: SystemTime) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    pub fn advance_days(&self, days: i64) {
        let d = Duration::from_secs(days.unsigned_abs() * 86_400);
        let mut t = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *t = if days >= 0 { *t + d } else { *t - d };
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}
