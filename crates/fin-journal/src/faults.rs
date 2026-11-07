//! Fault injection for the journal's tests and cost example (feature `test-hooks`): pause the
//! writer, stall, fail or tear its file writes (a slow or full disk, a crash mid-write), or
//! panic it. Without the feature every hook is a no-op the optimiser removes, and the type is
//! not exported.

#[cfg(feature = "test-hooks")]
pub use hooks::Faults;

#[cfg(not(feature = "test-hooks"))]
#[derive(Debug, Default)]
pub(crate) struct Faults;

#[cfg(not(feature = "test-hooks"))]
impl Faults {
    #[inline]
    pub(crate) fn before_write(&self) -> std::io::Result<()> {
        Ok(())
    }

    #[inline]
    pub(crate) fn tear_at(&self, _buf: &[u8]) -> Option<usize> {
        None
    }

    #[inline]
    pub(crate) fn park_torn(&self) {}

    #[inline]
    pub(crate) fn note_repair(&self) {}

    #[inline]
    pub(crate) fn check_panic(&self) {}

    #[inline]
    pub(crate) fn in_purge(&self) {}

    #[inline]
    pub(crate) fn purge_write(&self) -> std::io::Result<()> {
        Ok(())
    }

    #[inline]
    pub(crate) fn before_purge_rename(&self) {}

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
        /// Buffer writes wait while set; `holding` once one does.
        hold: bool,
        holding: bool,
    }

    #[derive(Debug, Default)]
    pub struct Faults {
        stall_ms: AtomicU64,
        fail: AtomicBool,
        tear: AtomicBool,
        torn: AtomicBool,
        repairs: AtomicU64,
        panic_next: AtomicBool,
        /// Purge rewrites left before the writer parks for good (0: never).
        purge_park_after: AtomicU64,
        /// Purge rewrites that fail (an I/O error) before one succeeds.
        purge_fail: AtomicU64,
        /// The next purge rewrite parks for good between its temp file and the rename.
        purge_park_rename: AtomicBool,
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

        /// The next buffer write stops inside a line, halfway, and the writer parks for good:
        /// a process killed now leaves a torn tail ([`Faults::torn`] once it has).
        pub fn tear_writes(&self) {
            self.tear.store(true, Ordering::SeqCst);
        }

        /// True once a torn write happened and the writer is parked.
        pub fn torn(&self) -> bool {
            self.torn.load(Ordering::SeqCst)
        }

        /// Torn tails this journal truncated when it scanned its files.
        pub fn repairs(&self) -> u64 {
            self.repairs.load(Ordering::SeqCst)
        }

        /// The writer panics on its next message.
        pub fn panic_next(&self) {
            self.panic_next.store(true, Ordering::SeqCst);
        }

        /// A purge parks the writer for good after `n` file rewrites (a kill mid-purge);
        /// [`Faults::torn`] says once it has.
        pub fn park_purge_after(&self, n: u64) {
            self.purge_park_after.store(n, Ordering::SeqCst);
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

        /// Buffer writes wait while `on` ([`Faults::wait_held`] for the first to): what the
        /// writer did before its next flush is on disk, the flush itself is not.
        pub fn hold_writes(&self, on: bool) {
            let mut g = self.lock();
            g.hold = on;
            if !on {
                g.holding = false;
            }
            self.cv.notify_all();
        }

        /// Waits until a write is held (at most `timeout`).
        pub fn wait_held(&self, timeout: Duration) -> bool {
            let guard = self.lock();
            let (guard, _) = self
                .cv
                .wait_timeout_while(guard, timeout, |p| !p.holding)
                .unwrap_or_else(|e| e.into_inner());
            guard.holding
        }

        pub(crate) fn before_write(&self) -> io::Result<()> {
            {
                let mut g = self.lock();
                while g.hold {
                    g.holding = true;
                    self.cv.notify_all();
                    g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
                }
            }
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

        /// Where to cut `buf` for a torn write: halfway, moved off a line end so the file
        /// cannot end on a whole line. `None` unless [`Faults::tear_writes`] is set.
        pub(crate) fn tear_at(&self, buf: &[u8]) -> Option<usize> {
            if buf.len() < 2 || !self.tear.swap(false, Ordering::SeqCst) {
                return None;
            }
            let mut n = buf.len() / 2;
            while n > 1 && buf[n - 1] == b'\n' {
                n -= 1;
            }
            Some(n)
        }

        /// Marks the torn write and parks the writer until the process is killed.
        pub(crate) fn park_torn(&self) -> ! {
            self.torn.store(true, Ordering::SeqCst);
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }

        pub(crate) fn note_repair(&self) {
            self.repairs.fetch_add(1, Ordering::SeqCst);
        }

        /// The next `n` purge rewrites fail with an I/O error.
        pub fn fail_purge_writes(&self, n: u64) {
            self.purge_fail.store(n, Ordering::SeqCst);
        }

        /// The next purge rewrite parks for good after its temp file, before the rename.
        pub fn park_before_purge_rename(&self) {
            self.purge_park_rename.store(true, Ordering::SeqCst);
        }

        pub(crate) fn purge_write(&self) -> io::Result<()> {
            let n = self.purge_fail.load(Ordering::SeqCst);
            if n == 0 {
                return Ok(());
            }
            self.purge_fail.store(n - 1, Ordering::SeqCst);
            Err(io::Error::other("injected: purge rewrite failed"))
        }

        pub(crate) fn before_purge_rename(&self) {
            if self.purge_park_rename.swap(false, Ordering::SeqCst) {
                self.park_torn();
            }
        }

        /// Called after each purge rewrite: parks for good once the count runs out.
        pub(crate) fn in_purge(&self) {
            match self.purge_park_after.load(Ordering::SeqCst) {
                0 => {}
                1 => self.park_torn(),
                n => self.purge_park_after.store(n - 1, Ordering::SeqCst),
            }
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
