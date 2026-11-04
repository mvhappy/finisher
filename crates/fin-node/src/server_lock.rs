//! `lockExclusive`: a kernel-held, non-blocking exclusive lock on a file (one finisherd per
//! server-data). `std::fs::File::try_lock` is `flock(LOCK_EX | LOCK_NB)` on Unix and
//! `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` on Windows. The OS drops
//! the lock when the process dies, so there is no staleness logic. flock locks belong to the
//! open file description: a second `lockExclusive` in the same process fails too.
//! On Windows the lock is mandatory: other processes cannot read the file (`EBUSY`) while it
//! is held, so the diagnostic pid is only readable there once the lock is released.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::sync::Mutex;

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

use crate::{guarded, reject};

/// A held lock; the file stays open (and locked) until `release()` or process exit.
#[napi]
pub struct ServerLock {
    path: String,
    file: Mutex<Option<File>>,
}

#[napi]
impl ServerLock {
    #[napi(getter, catch_unwind)]
    pub fn path(&self) -> String {
        self.path.clone()
    }

    /// Unlocks and closes the file. Releasing twice is a no-op. The file is left in place.
    #[napi(catch_unwind)]
    pub fn release(&self) -> Result<()> {
        guarded(
            || {
                let file = self.file.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some(file) = file {
                    // Closing the descriptor releases the lock even if unlock fails.
                    let _ = file.unlock();
                }
                Ok(())
            },
            || {},
        )
    }
}

fn start_failed(path: &str, what: &str, e: &std::io::Error) -> Error {
    reject(
        CoreError::new("SERVER_START_FAILED", format!("cannot {what} {path}: {e}"))
            .with_details(json!({ "path": path, "cause": e.kind().to_string() })),
    )
}

fn open(path: &str) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Opens or creates `path` (0600) and takes a non-blocking exclusive lock on it. Fails with
/// `SERVER_RUNNING` if another descriptor holds it. The pid is then written into the file for
/// diagnostics only.
#[napi(catch_unwind)]
pub fn lock_exclusive(path: String) -> Result<ServerLock> {
    guarded(
        || {
            let mut file = open(&path).map_err(|e| start_failed(&path, "open", &e))?;
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    return Err(reject(
                        CoreError::new(
                            "SERVER_RUNNING",
                            format!("{path} is locked by another finisherd"),
                        )
                        .with_details(json!({ "path": path }))
                        .with_fix("use the running server, or stop it before starting another"),
                    ));
                }
                Err(TryLockError::Error(e)) => return Err(start_failed(&path, "lock", &e)),
            }
            // Diagnostics only: nothing reads the pid for correctness.
            let _ = file
                .set_len(0)
                .and_then(|()| file.seek(SeekFrom::Start(0)))
                .and_then(|_| writeln!(file, "{}", std::process::id()));
            Ok(ServerLock {
                path,
                file: Mutex::new(Some(file)),
            })
        },
        || {},
    )
}
