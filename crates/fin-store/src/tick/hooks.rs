//! Test hooks for the phased tick (`test-hooks` feature): points in a tick's folder I/O,
//! outside the store lock, where it can be held, as a share that stalls would hold it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Where a held tick stops (both outside the store lock: `Store::tick_phased`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickPoint {
    /// In an import pass, after it scanned and verified the folder, before it applies.
    ImportRead,
    /// In an export step, with the segment encoded, before the fence right before its write.
    ExportWrite,
}

#[derive(Debug, Default)]
struct State {
    armed: Option<TickPoint>,
    hung: bool,
    released: bool,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

/// A store's tick hooks; clones share them (a tick's folder I/O carries one).
#[derive(Debug, Clone, Default)]
pub struct TickHooks(Arc<Shared>);

/// How long the test-side waits allow before failing the test.
const WAIT: Duration = Duration::from_secs(10);

impl TickHooks {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The next tick reaching `point` stops there until [`TickHooks::release`].
    pub fn hang(&self, point: TickPoint) {
        *self.lock() = State {
            armed: Some(point),
            ..State::default()
        };
    }

    /// Waits (real time) until a tick is stopped at the armed point.
    pub fn wait_hung(&self) {
        let s = self.lock();
        let (s, _) = self
            .0
            .cv
            .wait_timeout_while(s, WAIT, |s| !s.hung)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(s.hung, "no tick reached the hang point");
    }

    /// Whether a tick is stopped at the armed point now.
    pub fn is_hung(&self) -> bool {
        self.lock().hung
    }

    /// Lets the stopped tick go on (or the next one pass, if none has arrived yet).
    pub fn release(&self) {
        let mut s = self.lock();
        s.armed = None;
        s.released = true;
        self.0.cv.notify_all();
    }

    /// Blocks here if `point` is armed, until released (the arming is used up).
    pub(crate) fn at(&self, point: TickPoint) {
        let mut s = self.lock();
        if s.armed != Some(point) {
            return;
        }
        s.armed = None;
        s.hung = true;
        self.0.cv.notify_all();
        while !s.released {
            s = self
                .0
                .cv
                .wait(s)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        s.hung = false;
        s.released = false;
    }
}
