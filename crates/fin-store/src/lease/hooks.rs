//! Test hooks for the lease (`test-hooks` feature): points where a lease thread can be held,
//! as a share that stalls or a machine that sleeps would hold it.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::Inner;
use super::clock::lock;

/// Where a hung operation stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HangPoint {
    /// In a heartbeat, after it stamped and verified the record, before it writes.
    BeatBeforeWrite,
    /// In [`super::Lease::check`], before it reads the record.
    CheckBeforeRead,
}

#[derive(Debug, Default)]
pub(super) struct Hang {
    state: Mutex<(Option<HangPoint>, bool, bool)>,
    cv: Condvar,
}

/// How long the test-side waits allow before failing the test.
const WAIT: Duration = Duration::from_secs(5);

impl Hang {
    /// Blocks here if `point` is armed, until released (the arming is used up).
    pub(super) fn at(&self, point: HangPoint) {
        let mut s = lock(&self.state);
        if s.0 != Some(point) {
            return;
        }
        s.0 = None;
        s.1 = true;
        self.cv.notify_all();
        while !s.2 {
            s = self
                .cv
                .wait(s)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        (s.1, s.2) = (false, false);
    }
}

/// Handle on a lease's hooks; keeps working after the lease is dropped.
#[derive(Clone)]
pub struct LeaseHooks(pub(super) Arc<Inner>);

impl LeaseHooks {
    /// The next operation reaching `point` stops there until [`LeaseHooks::release`].
    pub fn hang(&self, point: HangPoint) {
        let mut s = lock(&self.0.hang.state);
        *s = (Some(point), false, false);
    }

    /// Waits (real time) until an operation is stopped at the armed point.
    pub fn wait_hung(&self) {
        let h = &self.0.hang;
        let s = lock(&h.state);
        let (s, t) =
            h.cv.wait_timeout_while(s, WAIT, |s| !s.1)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!t.timed_out() || s.1, "nothing reached the hang point");
    }

    pub fn release(&self) {
        let h = &self.0.hang;
        lock(&h.state).2 = true;
        h.cv.notify_all();
    }

    /// Whether the heartbeat thread has ended.
    pub fn exited(&self) -> bool {
        *lock(&self.0.exited)
    }

    /// Waits (real time) for the heartbeat thread to end.
    pub fn wait_exited(&self) {
        let until = Instant::now() + WAIT;
        while !self.exited() {
            assert!(Instant::now() < until, "the heartbeat thread did not end");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
