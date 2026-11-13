//! `<project-data>/db.lock`: who has the project database open. Every [`crate::Store`] holds
//! a shared OS lock on it ([`File::lock_shared`]: `flock` on Unix, `LockFileEx` on Windows)
//! for its lifetime; a rebuild replaces the database file and so holds it exclusively.
//! Without this a rebuild would rename the file under another store, whose writes would then
//! go to the kept copy. Server data is local, so OS locks are reliable here (they are never
//! used in the project folder).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fin_core::CoreError;
use serde_json::json;

use crate::StoreError;

/// The lock file's name in a project's server-data directory.
pub(crate) const FILE: &str = "db.lock";

fn path(project_data: &Path) -> PathBuf {
    project_data.join(FILE)
}

fn file(project_data: &Path) -> Result<File, StoreError> {
    std::fs::create_dir_all(project_data).map_err(StoreError::io(project_data))?;
    let p = path(project_data);
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&p)
        .map_err(StoreError::io(&p))
}

/// The shared lock of an open store (waits while a rebuild holds it).
pub(crate) fn shared(project_data: &Path) -> Result<File, StoreError> {
    let f = file(project_data)?;
    f.lock_shared()
        .map_err(StoreError::io(&path(project_data)))?;
    Ok(f)
}

fn in_use(project_data: &Path) -> StoreError {
    CoreError::new(
        "DB_IN_USE",
        "the project database is open in another store or process; a rebuild needs it to itself",
    )
    .with_details(json!({ "lock": path(project_data).to_string_lossy() }))
    .with_fix("close the project everywhere else on this machine (other servers, fin sessions), then retry")
    .into()
}

/// The exclusive lock, now or `DB_IN_USE`.
pub(crate) fn exclusive_now(project_data: &Path) -> Result<File, StoreError> {
    exclusive_within(project_data, Duration::ZERO, || false)?.ok_or_else(|| in_use(project_data))
}

/// The exclusive lock, waiting up to `wait` for other holders (an open finishing its own
/// rebuild), then `DB_IN_USE`. `None` as soon as `done()` says the wait is moot (another open
/// made the database meanwhile and now holds it shared).
pub(crate) fn exclusive_within(
    project_data: &Path,
    wait: Duration,
    done: impl Fn() -> bool,
) -> Result<Option<File>, StoreError> {
    let f = file(project_data)?;
    let until = Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(Some(f)),
            Err(TryLockError::WouldBlock) if done() => return Ok(None),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::WouldBlock) => return Err(in_use(project_data)),
            Err(TryLockError::Error(e)) => return Err(StoreError::io(&path(project_data))(e)),
        }
    }
}
