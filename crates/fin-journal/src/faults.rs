//! Fault injection for the journal's tests and cost example (feature `test-hooks`): pause the
//! writer, stall or fail its file writes (a slow or full disk), or panic it. Without the
//! feature every hook is a no-op the optimiser removes.

#[cfg(feature = "test-hooks")]
pub use hooks::Faults;

#[cfg(not(feature = "test-hooks"))]
#[derive(Debug, Default)]
pub struct Faults;

#[cfg(not(feature = "test-hooks"))]
impl Faults {
    #[inline]
    pub(crate) fn before_write(&self) -> std::io::Result<()> {
        Ok(())
    }

    #[inline]
    pub(crate) fn check_panic(&self) {}

    #[inline]
    pub(crate) fn park_if_paused(&self) {}
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct Pause {
        requested: bool,
        parked: bool,
    }

    #[derive(Debug, Default)]
    pub struct Faults {
        stall_ms: AtomicU64,
        fail: AtomicBool,
        panic_next: AtomicBool,
        pause: Mutex<Pause>,
        cv: Condvar,
    }

    impl Faults {
        /// Every file write first sleeps `by` (a slow disk); zero turns it off.
        pub fn stall_writes(&self, by: Duration) {
            self.stall_ms.store(by.as_millis() as u64, Ordering::SeqCst);
        }

        /// Every file write fails with `StorageFull` (a full disk) while set.
        pub fn fail_writes(&self, on: bool) {
            self.fail.store(on, Ordering::SeqCst);
        }

        /// The writer panics on its next message.
        pub fn panic_next(&self) {
            self.panic_next.store(true, Ordering::SeqCst);
        }

        pub(crate) fn request_pause(&self) {
            self.lock().requested = true;
        }

        /// Waits until the writer is parked (at most `timeout`).
        pub(crate) fn wait_parked(&self, timeout: Duration) -> bool {
            let guard = self.lock();
            let (guard, _) = self
                .cv
                .wait_timeout_while(guard, timeout, |p| !p.parked)
                .unwrap_or_else(|e| e.into_inner());
            guard.parked
        }

        pub(crate) fn resume(&self) {
            self.lock().requested = false;
            self.cv.notify_all();
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Pause> {
            self.pause.lock().unwrap_or_else(|e| e.into_inner())
        }

        pub(crate) fn before_write(&self) -> io::Result<()> {
            let ms = self.stall_ms.load(Ordering::SeqCst);
            if ms > 0 {
                std::thread::sleep(Duration::from_millis(ms));
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected: no space left",
                ));
            }
            Ok(())
        }

        pub(crate) fn check_panic(&self) {
            if self.panic_next.swap(false, Ordering::SeqCst) {
                panic!("injected writer panic");
            }
        }

        pub(crate) fn park_if_paused(&self) {
            let mut guard = self.lock();
            if !guard.requested {
                return;
            }
            guard.parked = true;
            self.cv.notify_all();
            while guard.requested {
                guard = self.cv.wait(guard).unwrap_or_else(|e| e.into_inner());
            }
            guard.parked = false;
        }
    }
}
