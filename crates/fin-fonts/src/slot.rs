//! The process's download slots (4C Decision 33): at most [`MAX_DOWNLOADS`] downloads at a
//! time, across every `Fetcher`. A slot is taken only after the locks a download needs.

use std::sync::{Condvar, Mutex};

/// Downloads at a time per process.
pub const MAX_DOWNLOADS: usize = 2;

static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// One of the process's [`MAX_DOWNLOADS`] download slots.
pub(crate) struct Slot;

impl Slot {
    pub(crate) fn take() -> Slot {
        let (m, cv) = &SLOTS;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        while *n >= MAX_DOWNLOADS {
            n = cv.wait(n).unwrap_or_else(|e| e.into_inner());
        }
        *n += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (m, cv) = &SLOTS;
        *m.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        cv.notify_one();
    }
}
