//! The server-data lock: `<server-data>/projects/<id>/lease.lock`, an OS file lock
//! ([`File::try_lock`]: `flock` on Unix, `LockFileEx` on Windows) that a [`super::Lease`]
//! holds for its whole life, including while its heartbeat thread is stuck in I/O (the
//! handle is in the lease's shared state). It never lives in the project folder (the spec
//! forbids OS locks there) and is never a PID file.
//!
//! What it answers: whether any process using this server data still holds a lease on the
//! project. A restarted server finds `lease.json` naming its own server id, which the
//! in-process registry cannot vouch for; without this it would wait out staleness (about a
//! minute) like any other server. So, in [`super::Contender`]:
//! - our own record, the lock is free, the record's holder host is this host, and the server
//!   data is on local storage (an OS lock on a network or sync folder proves nothing) → the
//!   previous process is gone: re-take at once, at a **fresh** epoch through a claim (the claim
//!   still settles a race with another server);
//! - our own record and the lock is held → another process of this server data holds it: held,
//!   follow;
//! - the lock cannot be taken for another reason → no shortcut.
//!
//! Every acquisition takes the lock first when one is configured; if another process holds
//! it, the acquisition backs off.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// Where the lock lives and whether the restart shortcut may use it.
#[derive(Debug, Clone)]
pub struct ServerLock {
    pub path: PathBuf,
    /// The server data is on local storage.
    pub shortcut: bool,
}

/// A held lock (released when dropped).
#[derive(Debug)]
pub(crate) struct Held(#[allow(dead_code)] File);

pub(crate) enum Attempt {
    Locked(Held),
    /// Another process (or another open handle) holds it.
    Busy,
    /// It cannot be opened or locked for another reason.
    Failed,
}

pub(crate) fn try_lock(path: &Path) -> Attempt {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    {
        Ok(f) => f,
        Err(_) => return Attempt::Failed,
    };
    match file.try_lock() {
        Ok(()) => Attempt::Locked(Held(file)),
        Err(TryLockError::WouldBlock) => Attempt::Busy,
        Err(TryLockError::Error(_)) => Attempt::Failed,
    }
}

/// Whether a record's holder description (`"<host> (finisher <version>)"`) names this host.
pub(crate) fn same_host(holder: &str) -> bool {
    holder
        .rsplit_once(" (finisher ")
        .is_some_and(|(host, _)| host == super::host_name())
}
