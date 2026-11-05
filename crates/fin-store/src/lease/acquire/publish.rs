//! Creating `lease.json`, the only time it is ever created ([`create_record`]).
//!
//! Never replacing one: two acquirers can both get here (the claims alone do not prevent it:
//! acquirers whose views differ claim different epochs), so the step that gives the record
//! its name must be decided by the file system or file server, as `create_new` is. And, where
//! possible, never leaving a partial record: a short `lease.json` reads as damaged to
//! everyone, and a damaged record is never taken over automatically.
//!
//! The whole record is staged in a temp file and fsynced, then published:
//! - **Unix:** `link(tmp, lease.json)`, which never replaces: decided by the server on NFS
//!   (`LINK`) and SMB (`FileLinkInformation` with `ReplaceIfExists` 0), and by the kernel
//!   locally. No-replace *renames* are not used: Linux cifs enforces `RENAME_NOREPLACE` only
//!   by a local lookup and sends the rename with `ReplaceIfExists` 1, and `renamex_np`'s
//!   `RENAME_EXCL` on macOS smbfs is no better known.
//! - **Windows:** `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`, which is
//!   `FileRenameInformation` with `ReplaceIfExists` 0 on SMB, decided by the server.
//! - **No hard links** (`link` unsupported or refused, e.g. FAT/exFAT, macOS smbfs, some NAS
//!   shares answering `EACCES`): `create_new`, then writing in place. Exclusive still, but a
//!   crash mid-write leaves a short record there. Misreading a refusal as "unsupported" only
//!   costs that crash-safety, never exclusivity: `create_new` fails on an existing name.
//!
//! Crash-safe, then: local disks with hard links (ext4, XFS, Btrfs, APFS, HFS+, NTFS), NFS,
//! Linux cifs to servers with hard links (Windows, Samba), and every Windows client. Only the
//! fallback: file systems and shares without hard links on Unix. Task 19 (spec §3.13) records
//! which real mounts take which path.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use crate::{fsio, names};

/// Creates `dir/lease.json` (`path`) holding `bytes` (module docs). `AlreadyExists` if another
/// record holds the name. An error means our record is not in place: a publish reported as
/// failed that did land (a lost reply) is recognised by reading the record back. The temp
/// file is removed on every path but a crash; [`remove_temps`] sweeps those.
pub(super) fn create_record(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(names::temp_name());
    let staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            fsio::sync_file(&file)
        });
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    if super::super::hooks::faults(dir, |f| std::mem::take(&mut f.crash_before_publish)) {
        return Err(io::Error::other(
            "injected crash before publishing lease.json",
        ));
    }
    let published = publish(dir, &tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match published {
        Ok(()) => {
            // Its error is not fatal: the caller reads the record back.
            let _ = fsio::sync_dir(dir);
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::Unsupported => create_in_place(path, bytes),
        // The name is decided; whether it is ours, the content tells (our epoch is ours alone).
        Err(_) if fsio::read_uncached_once(path).is_ok_and(|now| now == bytes) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `tmp` under the name `path`, never replacing it; `Unsupported` where this cannot be done
/// by the server (module docs).
fn publish(dir: &Path, tmp: &Path, path: &Path) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    if super::super::hooks::faults(dir, |f| f.no_links) {
        return Err(ErrorKind::Unsupported.into());
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = dir;
    #[cfg(unix)]
    {
        std::fs::hard_link(tmp, path).map_err(|e| {
            let refused = e.kind() == ErrorKind::Unsupported
                || e.raw_os_error().is_some_and(|code| {
                    // ENOTSUP and EOPNOTSUPP are one value on Linux, two on macOS.
                    [
                        libc::EPERM,
                        libc::EACCES,
                        libc::ENOTSUP,
                        libc::EOPNOTSUPP,
                        libc::ENOSYS,
                        libc::EINVAL,
                    ]
                    .contains(&code)
                });
            if refused {
                ErrorKind::Unsupported.into()
            } else {
                e
            }
        })
    }
    #[cfg(windows)]
    {
        fsio::rename_noreplace(tmp, path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (tmp, path);
        Err(ErrorKind::Unsupported.into())
    }
}

/// The fallback of [`create_record`]: `create_new`, then the write; the file is removed again
/// if the write fails. It is ours and was never valid, so nobody holds it.
fn create_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file.write_all(bytes).and_then(|()| fsio::sync_file(&file));
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Best effort, once we hold the lease: removes temp files in `dir` left by a
/// [`create_record`] that crashed. Only that writes temp files directly in `.finisher/`, and
/// with `lease.json` in place any creation still in progress fails anyway.
pub(super) fn remove_temps(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name().to_str().is_some_and(names::is_temp) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both ways never replace an existing record, and leave no temp file.
    #[test]
    fn create_record_never_replaces() {
        for no_links in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();
            if no_links {
                crate::lease::no_hard_links_for_test(dir);
            }
            let path = dir.join("lease.json");
            create_record(dir, &path, b"first").unwrap();
            let e = create_record(dir, &path, b"second").unwrap_err();
            assert_eq!(e.kind(), ErrorKind::AlreadyExists, "{no_links}: {e}");
            assert_eq!(std::fs::read(&path).unwrap(), b"first");
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1, "{no_links}");
        }
    }
}
