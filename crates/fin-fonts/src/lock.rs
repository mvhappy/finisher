//! The per-file fetch lock `<sha256>.lock` (4C Decision 32): an OS file lock
//! ([`File::try_lock`]: `flock` on Unix, `LockFileEx` on Windows, as `fin-store`'s
//! `lease.lock`), held while a file is fetched. Separate open handles conflict, so two
//! fetchers in one process exclude each other as two processes do. Lock files are never
//! deleted: unlinking a lock another process is about to take would let two holders in.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// A held lock, released when dropped.
#[derive(Debug)]
pub(crate) struct Held(#[allow(dead_code)] File);

/// Takes the lock at `path`, waiting at most `wait`. `None` when another holder kept it
/// that long.
pub(crate) fn acquire(path: &Path, wait: Duration) -> io::Result<Option<Held>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let deadline = Instant::now() + wait;
    let mut pause = Duration::from_millis(10);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(Held(file))),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(e),
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(200));
    }
}

/// Takes the lock at `path` if nobody holds it now (creating the lock file), so the caller
/// can act on what it guards while holding it; `None` when it is held or cannot be taken.
pub(crate) fn try_take(path: &Path) -> Option<Held> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .ok()?;
    file.try_lock().ok().map(|()| Held(file))
}
