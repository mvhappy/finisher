//! Automatic maintenance (spec §3.10): the daily backup and compaction a tick finds due
//! (`Store::due_work`). Neither ever makes a call fail with `ENGINE_BUSY`:
//!
//! - **When:** only once the engine has been quiet for [`AUTO_IDLE`] (no short call since, nor
//!   since the open), or the work has waited [`AUTO_DEFER_MAX`] since it was first found due.
//! - **Backup:** off the store, on a thread of its own (`fin-node-backup`): the store hands
//!   out a `DailyBackup` and keeps serving calls while it copies, verifies and moves the copy
//!   into place on its own connection (a WAL snapshot; `fin_store::backup` module docs). A
//!   long call (`verify`, `rebuild`, …, `close`) cancels it first and waits for it to stop; a
//!   cancelled backup is due again (and still waits for a quiet engine).
//! - **Compaction:** on the maintenance thread, one step per tick (a manifest's phase 2 or a
//!   branch's phase 1), through the tick's step-by-step access
//!   (`Store::run_due_compaction_step_phased`): its folder I/O (the fresh reads, the pack
//!   writes, the fences) runs without the store mutex, so calls answer meanwhile; the checks
//!   against the database and each delete take it briefly (fin-store's `compact` module docs,
//!   "Locking"). A long call that takes the store meanwhile ends the step there.
//!
//! A compaction failure is a non-fatal `error` event; a panic, in a step under the mutex or in
//! its folder I/O between steps, a fatal one (the engine is dead and the maintenance thread
//! ends). A failed backup is retried after the store's retry
//! wait (10 min).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_store::{BackupCancel, BackupOutcome, DailyBackup};

use crate::engine::{Shared, State, lock};

/// How long the engine must be quiet (no short call) before automatic work starts.
pub(crate) const AUTO_IDLE: Duration = Duration::from_secs(30);
/// Work found due this long ago starts even while calls keep coming.
pub(crate) const AUTO_DEFER_MAX: Duration = Duration::from_secs(10 * 60);

/// What a backup thread hands back when it could not report to the store itself (a long call
/// had it).
type Unreported = Option<(DailyBackup, BackupOutcome)>;

struct Running {
    cancel: Arc<BackupCancel>,
    thread: JoinHandle<Unreported>,
}

/// An engine's automatic-work state.
pub(crate) struct Auto {
    base: Instant,
    /// Milliseconds after `base` of the last short call (0: none since the open).
    last_call: AtomicU64,
    backup_since: Mutex<Option<Instant>>,
    compaction_since: Mutex<Option<Instant>>,
    backup: Mutex<Option<Running>>,
    /// Test hooks: the quiet time required (`u64::MAX`: [`AUTO_IDLE`]) and the pause inside
    /// each backup.
    idle_ms: AtomicU64,
    pause_ms: AtomicU64,
}

impl Default for Auto {
    fn default() -> Auto {
        Auto {
            base: Instant::now(),
            last_call: AtomicU64::new(0),
            backup_since: Mutex::new(None),
            compaction_since: Mutex::new(None),
            backup: Mutex::new(None),
            idle_ms: AtomicU64::new(u64::MAX),
            pause_ms: AtomicU64::new(0),
        }
    }
}

impl Auto {
    /// A short call ran: automatic work waits for the engine to be quiet again.
    pub(crate) fn touch(&self) {
        let ms = u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_call.store(ms.max(1), Ordering::SeqCst);
    }

    fn quiet_for(&self) -> Duration {
        let last = Duration::from_millis(self.last_call.load(Ordering::SeqCst));
        self.base.elapsed().saturating_sub(last)
    }

    fn idle(&self) -> Duration {
        match self.idle_ms.load(Ordering::SeqCst) {
            u64::MAX => AUTO_IDLE,
            ms => Duration::from_millis(ms),
        }
    }

    /// Whether due work (first seen due at `since`, updated here) may start now.
    fn may_start(&self, since: &Mutex<Option<Instant>>, due: bool) -> bool {
        let mut since = lock(since);
        if !due {
            *since = None;
            return false;
        }
        let first = *since.get_or_insert_with(Instant::now);
        ready(self.quiet_for(), self.idle(), first.elapsed())
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn set_idle_for_test(&self, ms: u64) {
        self.idle_ms.store(ms, Ordering::SeqCst);
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn set_backup_pause_for_test(&self, ms: u64) {
        self.pause_ms.store(ms, Ordering::SeqCst);
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn backup_running(&self) -> bool {
        lock(&self.backup)
            .as_ref()
            .is_some_and(|r| !r.thread.is_finished())
    }
}

/// The start rule (module docs): quiet for `idle`, or due for [`AUTO_DEFER_MAX`].
fn ready(quiet: Duration, idle: Duration, due_for: Duration) -> bool {
    quiet >= idle || due_for >= AUTO_DEFER_MAX
}

/// After each tick: starts the due backup, runs a due compaction step. `false` once the
/// engine is dead (a compaction panicked).
pub(crate) fn after_tick(inner: &Shared) -> bool {
    let due = match &*lock(&inner.state) {
        State::Open(s) => s.due_work(),
        _ => return true,
    };
    let auto = &inner.auto;
    let idle_backup = !backup_running(inner);
    if auto.may_start(&auto.backup_since, due.backup && idle_backup) {
        start_backup(inner);
    }
    if auto.may_start(&auto.compaction_since, due.compaction) {
        return compaction_step(inner);
    }
    true
}

fn backup_running(inner: &Shared) -> bool {
    let mut slot = lock(&inner.auto.backup);
    if slot.as_ref().is_some_and(|r| r.thread.is_finished()) {
        // It reported to the store itself (the store was there when it finished).
        if let Some(r) = slot.take() {
            let _ = r.thread.join();
        }
    }
    slot.is_some()
}

fn start_backup(inner: &Shared) {
    // Held until the thread is recorded, so a long call that takes the store meanwhile either
    // finds no backup to hand out or this one to cancel (`stop_backup`).
    let mut slot = lock(&inner.auto.backup);
    let job = match &mut *lock(&inner.state) {
        State::Open(s) => s.take_due_backup(),
        _ => None,
    };
    let Some(job) = job else { return };
    let cancel = Arc::new(BackupCancel::default());
    cancel.pause_for_test(inner.auto.pause_ms.load(Ordering::SeqCst));
    let (shared, c) = (inner.clone(), cancel.clone());
    let spawned = std::thread::Builder::new()
        .name("fin-node-backup".to_string())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| job.run(&c))).unwrap_or_else(|_| {
                BackupOutcome::Failed(
                    fin_core::CoreError::new("INTERNAL", "the backup panicked").into(),
                )
            });
            match &mut *lock(&shared.state) {
                State::Open(s) => {
                    s.finish_backup(&job, &outcome);
                    None
                }
                _ => Some((job, outcome)),
            }
        });
    // Not started: due again at the next poll.
    if let Ok(thread) = spawned {
        *slot = Some(Running { cancel, thread });
        *lock(&inner.auto.backup_since) = None;
    }
}

/// Cancels a running automatic backup and waits for it to stop (a long call, `close`). What
/// it did, when it could not tell the store (the caller has the store now).
pub(crate) fn stop_backup(inner: &Shared) -> Unreported {
    let running = lock(&inner.auto.backup).take()?;
    running.cancel.cancel();
    running.thread.join().ok().flatten()
}

fn compaction_step(inner: &Shared) -> bool {
    crate::maintenance::phased(inner, false, |access| {
        fin_store::Store::run_due_compaction_step_phased(access).map(|_| ())
    })
}
