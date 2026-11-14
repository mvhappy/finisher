//! The OS error codes [`super`] retries for [`super::RETRIES`] × [`super::RETRY_DELAY`]
//! (1.5 s), in one place.
//!
//! The S10 spike retried `EBUSY`, `ENOENT` and `EIO` on Unix (plus `EACCES` in its fenced
//! mode), and Windows codes 2, 3, 5, 32 and 33. This generic list keeps the codes that mean
//! "the same call will work in a moment" and leaves out those that carry meaning for the
//! caller:
//!
//! - `ENOENT` / Windows 2 (file not found) / 3 (path not found): "missing" is an answer.
//!   A missing `lease.json` beside a claim is held, a heartbeat must never recreate a deleted
//!   one, and the Windows not-found cache (5 s) outlasts 1.5 s anyway; the lease handles it
//!   with claims and the 45 s staleness window instead.
//! - `EIO`: also a real media or protocol error. A Linux cifs `soft` mount returns it while it
//!   reconnects, which the lease rides out without this list: its holder treats any failed
//!   read or write as a failed beat, retried until the self-fence deadline (`lease::heartbeat`),
//!   and segment writes are retried on the next pass.
//! - `EACCES`: a real permission error would cost every caller 1.5 s for nothing.
//!
//! Still to be confirmed by the cross-host run (Task 19, spec §3.13).

use std::io::{self, ErrorKind};

/// - `EINTR`: a signal interrupted the call.
/// - `EAGAIN`/`EWOULDBLOCK`: the resource is momentarily unavailable (also what Linux CIFS
///   returns while it reconnects a session).
/// - `EBUSY`: the file or device is momentarily in use.
/// - `ESTALE`: an NFS handle went stale; every attempt reopens by path, which revalidates it.
///
/// Deliberately not retried: `ETXTBSY` (only for running executables, never our files),
/// `ETIMEDOUT`/`EHOSTDOWN`/`ECONNRESET` (a real outage outlasts 1.5 s; the caller decides), and
/// `ENOENT`, `EIO`, `EACCES` (module docs).
#[cfg(unix)]
const TRANSIENT: &[i32] = &[
    libc::EINTR,
    libc::EAGAIN,
    libc::EWOULDBLOCK,
    libc::EBUSY,
    libc::ESTALE,
];

/// - `ERROR_ACCESS_DENIED` (5): a rename or open races a reader, a pending delete or an
///   antivirus scan (parent spec §27.9); creating a lease claim whose name is still pending
///   deletion fails with it too. Also a real permission error, which then costs one 1.5 s
///   retry.
/// - `ERROR_SHARING_VIOLATION` (32): another process opened the file without sharing.
/// - `ERROR_LOCK_VIOLATION` (33): another process holds a byte-range lock on it.
/// - `ERROR_NETNAME_DELETED` (64): the SMB session dropped; reopening after the redirector
///   reconnects succeeds.
///
/// Not retried: 2 and 3 (not found, module docs).
#[cfg(windows)]
const TRANSIENT: &[i32] = {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_NETNAME_DELETED, ERROR_SHARING_VIOLATION,
    };
    &[
        ERROR_ACCESS_DENIED as i32,
        ERROR_SHARING_VIOLATION as i32,
        ERROR_LOCK_VIOLATION as i32,
        ERROR_NETNAME_DELETED as i32,
    ]
};

#[cfg(not(any(unix, windows)))]
const TRANSIENT: &[i32] = &[];

/// Whether `e` is worth retrying shortly (see the `TRANSIENT` lists).
pub fn is_transient(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock)
        || e.raw_os_error()
            .is_some_and(|code| TRANSIENT.contains(&code))
}
