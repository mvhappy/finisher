//! The daily backup off the store (spec §3.8, §3.10): [`Store::take_due_backup`] hands out
//! what the backup needs ([`DailyBackup`]: paths and the day), the caller runs it on any
//! thread ([`DailyBackup::run`]) while the store keeps serving calls, then reports back
//! ([`Store::finish_backup`]).
//!
//! The job opens its own connection to the database. In WAL mode its `VACUUM INTO` reads one
//! snapshot, so the copy is consistent while the store's connection commits meanwhile (a
//! commit is not in it; the next day's backup has it). It then verifies and moves the copy
//! into place exactly as an in-tick backup does (`super::backup_to`).
//!
//! [`BackupCancel`] stops it from another thread: between steps, and inside `VACUUM INTO`
//! (`sqlite3_interrupt`). A cancelled backup leaves nothing behind and is due again at once
//! (it is not a failure, which waits [`crate::tick::BACKUP_RETRY_MS`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use fin_core::CoreError;
use rusqlite::{Connection, InterruptHandle, OpenFlags};

use crate::{Store, StoreError};

/// The error code of a cancelled step (never reported: [`BackupOutcome::Cancelled`]).
const CANCELLED: &str = "BACKUP_CANCELLED";

pub(super) fn cancelled() -> StoreError {
    CoreError::new(CANCELLED, "the backup was cancelled").into()
}

/// Stops a running [`DailyBackup`] from another thread ([`BackupCancel::cancel`]).
#[derive(Default)]
pub struct BackupCancel {
    stop: AtomicBool,
    handle: Mutex<Option<InterruptHandle>>,
    /// Test hook: how long the backup waits between its copy and its checks.
    pause_ms: AtomicU64,
}

impl BackupCancel {
    /// Cancels the backup: it stops at its next step, or at once inside `VACUUM INTO`.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self
            .handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            h.interrupt();
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn attach(&self, conn: &Connection) {
        *self.handle.lock().unwrap_or_else(|p| p.into_inner()) = Some(conn.get_interrupt_handle());
    }

    /// Test hook: the backup waits `ms` after its copy, before checking and moving it into
    /// place (so a test can work, or cancel, while one runs). Cut short by a cancel.
    #[doc(hidden)]
    pub fn pause_for_test(&self, ms: u64) {
        self.pause_ms.store(ms, Ordering::SeqCst);
    }

    pub(super) fn test_pause(&self) {
        let ms = self.pause_ms.load(Ordering::SeqCst);
        let until = std::time::Instant::now() + Duration::from_millis(ms);
        while !self.cancelled() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// What a [`DailyBackup`] did.
#[derive(Debug)]
pub enum BackupOutcome {
    /// Today's backup was taken (its path).
    Taken(PathBuf),
    /// Today already had a whole backup.
    Present,
    /// Cancelled ([`BackupCancel`]): nothing written, due again.
    Cancelled,
    /// It failed (retried after [`crate::tick::BACKUP_RETRY_MS`]).
    Failed(StoreError),
}

/// The daily backup of one local day, to run off the store (module docs).
#[derive(Debug, Clone)]
pub struct DailyBackup {
    db: PathBuf,
    project_data: PathBuf,
    now_ms: i64,
}

impl DailyBackup {
    /// Takes the backup on a connection of its own (module docs).
    pub fn run(&self, cancel: &BackupCancel) -> BackupOutcome {
        let result = (|| {
            // Never `CREATE`: a database that vanished must not be backed up empty.
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let conn = Connection::open_with_flags(&self.db, flags)?;
            conn.busy_timeout(Duration::from_secs(5))?;
            cancel.attach(&conn);
            super::daily(&conn, &self.project_data, self.now_ms, Some(cancel))
        })();
        match result {
            Ok(Some(path)) => BackupOutcome::Taken(path),
            Ok(None) => BackupOutcome::Present,
            Err(_) if cancel.cancelled() => BackupOutcome::Cancelled,
            Err(e) => BackupOutcome::Failed(e),
        }
    }

    /// The database this backs up.
    pub fn db(&self) -> &Path {
        &self.db
    }
}

impl Store {
    /// Today's daily backup when a tick found it due ([`crate::TickOptions`] with `backup`
    /// off), to run off the store ([`DailyBackup::run`]); clears it from
    /// [`Store::due_work`]. `None` when there is none to take (taken today, or a failure's
    /// retry wait).
    pub fn take_due_backup(&mut self) -> Option<DailyBackup> {
        self.due.backup = false;
        let now_ms = crate::commit::now_ms();
        if !self.backup_due(now_ms) {
            return None;
        }
        let project_data = self
            .db_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        Some(DailyBackup {
            db: self.db_path.clone(),
            project_data,
            now_ms,
        })
    }

    /// Records what `job` did: the day is done, or a failure waits before the next try. A
    /// cancelled job is due again.
    pub fn finish_backup(&mut self, job: &DailyBackup, outcome: &BackupOutcome) {
        match outcome {
            BackupOutcome::Taken(_) | BackupOutcome::Present => self.backup_done(job.now_ms),
            BackupOutcome::Failed(_) => self.backup_failed(job.now_ms),
            BackupOutcome::Cancelled => {}
        }
    }
}
