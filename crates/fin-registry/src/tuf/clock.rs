//! The injected wall clock (8A Decisions 12–13): `tough` reads the system clock itself, so
//! every expiry, freshness and clock check here uses a [`Clock`] instead and `tough` loads
//! with expiry enforcement off.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock tests set and move.
#[derive(Debug)]
pub struct FixedClock(Mutex<SystemTime>);

impl FixedClock {
    pub fn new(at: SystemTime) -> FixedClock {
        FixedClock(Mutex::new(at))
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

pub(crate) fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

