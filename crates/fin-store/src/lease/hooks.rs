//! Test hooks for the lease (`test-hooks` feature): points where a lease thread can be held,
//! as a share that stalls or a machine that sleeps would hold it, and per-folder faults
//! (reads of `lease.json`, creating it, creating claims).

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
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
    /// In [`super::Lease::check`], after it read the record (the record lock released),
    /// before it lists claims.
    CheckAfterVerify,
}

#[derive(Debug, Default)]
pub(super) struct Hang {
    /// Full checks ([`super::Lease::check`]) so far.
    pub(super) full_checks: std::sync::atomic::AtomicU64,
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

    /// How many full checks (with folder I/O) this lease has done.
    pub fn full_checks(&self) -> u64 {
        self.0
            .hang
            .full_checks
            .load(std::sync::atomic::Ordering::SeqCst)
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

/// What one read of `lease.json` returns ([`fault_record_reads_for_test`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFault {
    /// The real read.
    Real,
    /// The read fails with this error (as a cifs `soft` mount's `EIO` while it reconnects).
    Error(io::ErrorKind),
    /// The read returns only the first half of the file (a torn or short SMB read).
    Torn,
    /// The read returns the record one heartbeat older (a stale cached read, or a write
    /// that has not landed).
    Stale,
}

#[derive(Debug, Default)]
pub(super) struct Faults {
    pub(super) reads: VecDeque<ReadFault>,
    pub(super) crash_before_publish: bool,
    pub(super) deny_claims: u32,
    pub(super) no_links: bool,
}

pub(super) fn faults<T>(dir: &Path, f: impl FnOnce(&mut Faults) -> T) -> T {
    static ALL: OnceLock<Mutex<HashMap<PathBuf, Faults>>> = OnceLock::new();
    let mut all = lock(ALL.get_or_init(Default::default));
    f(all.entry(dir.to_path_buf()).or_default())
}

/// Test hook: of the next reads of `dir/lease.json` (by anyone in this process), `skip` are
/// real, then `n` return `fault`; later ones are real again. Replaces any earlier script.
#[doc(hidden)]
pub fn fault_record_reads_for_test(dir: &Path, skip: usize, fault: ReadFault, n: usize) {
    faults(dir, |f| {
        f.reads = std::iter::repeat_n(ReadFault::Real, skip)
            .chain(std::iter::repeat_n(fault, n))
            .collect();
    });
}

/// Test hook: the next acquisition that creates `dir/lease.json` "crashes" once its record is
/// staged in a temp file, before publishing it: the temp file is left behind, as after a
/// real crash. (Combine with `claim::keep_backed_off_claims_for_test` to leave the claim too.)
#[doc(hidden)]
pub fn crash_before_publishing_record_for_test(dir: &Path) {
    faults(dir, |f| f.crash_before_publish = true);
}

/// Test hook: publishing a new `lease.json` in `dir` finds no hard links (FAT, macOS smbfs),
/// so it falls back to `create_new` and writing in place.
#[doc(hidden)]
pub fn no_hard_links_for_test(dir: &Path) {
    faults(dir, |f| f.no_links = true);
}

/// Test hook: the next `n` attempts to create a claim in `dir` fail as creating a name that is
/// still pending deletion does on Windows (`ERROR_ACCESS_DENIED`; elsewhere `EBUSY`, which
/// is just as transient). The name is not created.
#[doc(hidden)]
pub fn deny_claim_creates_for_test(dir: &Path, n: u32) {
    faults(dir, |f| f.deny_claims = n);
}

/// The error [`deny_claim_creates_for_test`] injects.
pub(super) fn delete_pending() -> io::Error {
    #[cfg(windows)]
    let code = windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32;
    #[cfg(unix)]
    let code = libc::EBUSY;
    #[cfg(not(any(unix, windows)))]
    return io::Error::from(io::ErrorKind::Interrupted);
    #[cfg(any(unix, windows))]
    io::Error::from_raw_os_error(code)
}
