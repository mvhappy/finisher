//! Volume rules (Decisions 15, 17) and the per-OS measurement (Task 2: Linux, macOS; Task 3:
//! Windows).
//!
//! - [`key`]: BLAKE3-16 of the OS's identity fields (macOS `ATTR_VOL_UUID`, or a network
//!   volume's [`strip_userinfo`]-ed `f_mntfromname` plus `f_fstypename`; Linux the mount
//!   source plus type for network and FUSE mounts, `f_fsid` plus type locally; Windows the
//!   volume serial plus `\\server\share`).
//! - [`classify_root`]: unmounted, volume changed (a different key **and** a failed
//!   confirmation by file ids), empty, or online.
//! - [`identity`], [`root_state`] (frozen): the measurement of a root path now.
//!
//! ## Step 0 findings (2026-11-07; plan 5D Task 2, written into the spec by Task 16)
//!
//! **`libc` 0.2.191** has `statx` with `AT_NO_AUTOMOUNT`, `STATX_ATTR_MOUNT_ROOT`,
//! `STATX_ATTR_AUTOMOUNT` and `STATX_MNT_ID`; `open_how`, `RESOLVE_*` and `SYS_openat2` (no
//! wrapper: called through `syscall`); `O_NOATIME`; `getattrlistbulk`, `getattrlistat`,
//! `fgetattrlist`, `attrlist`, `ATTR_VOL_UUID`, `ATTR_DIR_MOUNTSTATUS`, `O_NOFOLLOW_ANY`,
//! `FSOPT_PACK_INVAL_ATTRS`, `_PC_CASE_SENSITIVE`. Declared locally: `setiopolicy_np`,
//! `getiopolicy_np` and the `IOPOL_*` values ([`crate::process`]); `ATTR_CMN_ERROR`,
//! `DIR_MNTSTATUS_TRIGGER`, `VREG`/`VDIR`/`VLNK` (`source::macos`); `SF_DATALESS`
//! ([`crate::placeholder`]).
//!
//! **Linux identity across a remount** (kernel 6.8, ext4; bind mounts and tmpfs in an
//! unprivileged user namespace, loop images as root in a container):
//!
//! | case | key | mount source | `f_fsid` | `st_dev` | root ino, sample |
//! |---|---|---|---|---|---|
//! | bind mount, umount and mount again | same | same | same | same | same |
//! | ext4 image, umount and mount on the same loop device | same | same | same | same | same |
//! | the same image on another loop device | same | changed | same (from the UUID) | changed | same |
//! | a new tmpfs | **changed** | same | changed (per mount) | same (reused) | ino 1; content decides |
//! | unmounted (the empty mount-point directory) | — | parent's | parent's | bind mount: equal to the mounted one | other |
//!
//! So: local filesystems are keyed by `f_fsid`, not the mount source (a loop or USB device
//! number changes); a bind mount's unmount is seen only through `STATX_ATTR_MOUNT_ROOT`
//! (`st_dev` is the parent's both before and after); a remount that changes the key is told
//! apart from a new volume by the root's file id and the sample (Decision 17). NFS and CIFS
//! (`FIN_TEST_SHARE`) and macOS (disk image detach/attach, SMB reconnect, a real dataless
//! file and folder) are measured by the controller: the tests `identity_stable_across_remount`,
//! `disk_image_detached_is_offline` and `dataless_dir_not_descended` (`FIN_TEST_ICLOUD=1`)
//! print and assert them, and `examples/volume_identity.rs` prints the raw fields of any
//! path.

use std::io;
use std::path::Path;
use std::time::Duration;

use crate::source::{RawEntry, RootStat};
use crate::types::{EntryType, RootState, SampleEntry, VolumeIdentity};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
mod probe;

#[cfg(feature = "test-hooks")]
pub use probe::hooks as probe_hooks;

/// At most this many top-level entries are sampled.
pub const SAMPLE_MAX: usize = 16;

/// BLAKE3-16 (32 hex digits) of the fields, each length-prefixed so field boundaries count.
pub fn key(fields: &[&[u8]]) -> String {
    let mut h = blake3::Hasher::new();
    for f in fields {
        h.update(&(f.len() as u64).to_le_bytes());
        h.update(f);
    }
    h.finalize().as_bytes()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A mount source without credentials: `//user:pass@server/share` → `//server/share`,
/// `user@host:/path` → `host:/path` (SMB as macOS reports it, sshfs); a source without
/// userinfo is returned unchanged. A password may hold `:`, `@` and `/`, so the userinfo runs
/// to the **last** `@` before the host part:
/// - `//…`: when the part before the first `/` holds a `:` (a password), the last `@` of the
///   whole source (a share path holding `@` loses more than its userinfo, which is the safe
///   way to be wrong); else the last `@` before the first `/` (`user;DOMAIN@server`);
/// - `host:/path`: the last `@` before the first `:/`, else before the first `/`.
pub fn strip_userinfo(source: &str) -> String {
    let (prefix, rest) = match source.strip_prefix("//") {
        Some(r) => ("//", r),
        None => ("", source),
    };
    let first = rest.find('/').unwrap_or(rest.len());
    let host = if !prefix.is_empty() && rest[..first].contains(':') {
        rest.len()
    } else if prefix.is_empty()
        && let Some(i) = rest.find(":/")
    {
        i
    } else {
        first
    };
    match rest[..host].rfind('@') {
        Some(at) => format!("{prefix}{}", &rest[at + 1..]),
        None => source.to_owned(),
    }
}

/// The sample of a root's listing: its first [`SAMPLE_MAX`] files and directories by raw
/// name (links and other entries are left out).
pub fn sample(entries: &[RawEntry]) -> Vec<SampleEntry> {
    let mut v: Vec<&RawEntry> = entries
        .iter()
        .filter(|e| matches!(e.entry_type, EntryType::File | EntryType::Dir))
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v.into_iter()
        .take(SAMPLE_MAX)
        .map(RawEntry::sample)
        .collect()
}

/// Whether a root is a mount root: Linux's `STATX_ATTR_MOUNT_ROOT` when known (it also sees
/// bind mounts on one device), else its device differing from its parent's; a filesystem
/// root (no parent) always is.
pub fn is_mount_root(attr: Option<bool>, dev: &str, parent_dev: Option<&str>) -> bool {
    match (attr, parent_dev) {
        (Some(a), _) => a,
        (None, Some(p)) => p != dev,
        (None, None) => true,
    }
}

/// How many `stored` entries are found in `now` with equal size and mtime and an equal file
/// id when both have one; each `now` entry matches once.
pub fn sample_matches(stored: &[SampleEntry], now: &[SampleEntry]) -> usize {
    let mut used = vec![false; now.len()];
    let mut n = 0;
    for s in stored {
        let hit = now.iter().enumerate().position(|(i, e)| {
            !used[i]
                && e.size == s.size
                && e.mtime_ns == s.mtime_ns
                && match (&s.file_id, &e.file_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                }
        });
        if let Some(i) = hit {
            used[i] = true;
            n += 1;
        }
    }
    n
}

/// Whether a key change is confirmed harmless: the root's file id is unchanged (or unknown on
/// either side) and at least half of the stored sample still matches.
pub fn confirms(stored: &VolumeIdentity, now: &RootStat) -> bool {
    confirmed(stored, now.root_file_id.as_deref(), &now.sample)
}

fn confirmed(stored: &VolumeIdentity, root_file_id: Option<&[u8]>, sample: &[SampleEntry]) -> bool {
    let root_same = match (&stored.root_file_id, root_file_id) {
        (Some(a), Some(b)) => a.as_slice() == b,
        _ => true,
    };
    root_same && sample_matches(&stored.sample, sample) * 2 >= stored.sample.len()
}

/// Whether a nested mount's volume measured `now` is the `stored` one (Decision 17, per
/// directory): the same key, or a different key that [`confirms`]' rule accepts.
pub fn same_volume(stored: &VolumeIdentity, now: &VolumeIdentity) -> bool {
    now.key == stored.key || confirmed(stored, now.root_file_id.as_deref(), &now.sample)
}

/// A root's state from its stored identity and its facts now (Decision 17). Unreachable and
/// timed-out roots never get here: the OS layer answers those states itself.
///
/// 1. It was a mount root and is not now → `unmounted` (the empty mount-point directory is
///    never scanned);
/// 2. a different key that [`confirms`] rejects → `volumeChanged`; one it accepts → the new
///    key, silently (the caller logs it at debug);
/// 3. no entries now while the stored sample had some → `empty`;
/// 4. else `online`, with the identity measured now. The device id alone is never compared.
pub fn classify_root(stored: &VolumeIdentity, now: &RootStat) -> RootState {
    if stored.mount_root && !now.mount_root() {
        return RootState::Unmounted;
    }
    if now.key != stored.key && !confirms(stored, now) {
        return RootState::VolumeChanged {
            key: now.key.clone(),
        };
    }
    if now.entries == 0 && !stored.sample.is_empty() {
        return RootState::Empty;
    }
    RootState::Online(now.identity())
}

/// Linux case sensitivity (Decision 10) from the mount's type and options:
/// case-insensitive for `vfat`, `exfat`, `ntfs3` and a `cifs`/`smb3` mount with `nocase`;
/// else sensitive.
pub fn linux_case_sensitive(fstype: &str, options: &str) -> bool {
    match fstype {
        "vfat" | "exfat" | "ntfs3" => false,
        "cifs" | "smb3" => !options
            .split(',')
            .any(|o| o == "nocase" || o == "ignorecase"),
        _ => true,
    }
}

/// A root's facts now ([`crate::source::Source::stat_root`] of the native source).
pub fn root_stat(root: &Path) -> io::Result<RootStat> {
    #[cfg(unix)]
    {
        use crate::source::Source as _;
        crate::source::NativeSource::new(root).stat_root()
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this OS's volume identity is not built yet",
        ))
    }
}

/// A root's volume identity, measured now (at `library.add` and each full scan).
pub fn identity(root: &Path) -> io::Result<VolumeIdentity> {
    root_stat(root).map(|s| s.identity())
}

/// The short code of an unreachable root (never a path).
pub fn unreachable_code(e: &io::Error) -> &'static str {
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        match code {
            // A dataless directory listed (or file read) with the process's materialisation
            // policy off (Decision 6, `crate::process`): the data is in the cloud.
            #[cfg(target_os = "macos")]
            libc::EDEADLK => return crate::walk::code::CLOUD_ONLY,
            libc::ESTALE => return "STALE",
            libc::ENOTCONN | libc::EHOSTDOWN | libc::EHOSTUNREACH | libc::ENETUNREACH => {
                return "DISCONNECTED";
            }
            libc::EIO => return "IO",
            _ => {}
        }
    }
    match e.kind() {
        io::ErrorKind::NotFound => "NOT_FOUND",
        io::ErrorKind::PermissionDenied => "PERMISSION_DENIED",
        io::ErrorKind::NotADirectory => "NOT_A_DIRECTORY",
        io::ErrorKind::TimedOut => "TIMEOUT",
        _ => "IO",
    }
}

/// A root's state now against its stored identity (Decision 17). The measurement runs on its
/// own thread: past `timeout` (60 s for a first stat, spin-up included) the answer is
/// `Timeout` and the thread is left to finish (a dead hard mount blocks only it). **One
/// measurement per root is in flight**: a call while one is pending waits on it rather than
/// starting another, so a dead mount polled every few seconds keeps one stuck thread, not
/// one per poll.
pub fn root_state(root: &Path, stored: &VolumeIdentity, timeout: Duration) -> RootState {
    match probe::run(root, timeout) {
        Some(Ok(now)) => classify_root(stored, &now),
        Some(Err(code)) => RootState::Unreachable { code: code.into() },
        None => RootState::Timeout,
    }
}
