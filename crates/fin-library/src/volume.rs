//! Volume rules (Decisions 15, 17), pure; the per-OS measurement is Tasks 2–3.
//!
//! - [`key`]: BLAKE3-16 of the OS's identity fields (macOS `ATTR_VOL_UUID`, or a network
//!   volume's [`strip_userinfo`]-ed `f_mntfromname` plus `f_fstypename`; Linux the mount
//!   source plus type for network and FUSE mounts, `f_fsid` plus type locally; Windows the
//!   volume serial plus `\\server\share`).
//! - [`classify_root`]: unmounted, volume changed (a different key **and** a failed
//!   confirmation by file ids), empty, or online.

use crate::source::{RawEntry, RootStat};
use crate::types::{EntryType, RootState, SampleEntry, VolumeIdentity};

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
/// userinfo is returned unchanged.
pub fn strip_userinfo(source: &str) -> String {
    let (prefix, rest) = match source.strip_prefix("//") {
        Some(r) => ("//", r),
        None => ("", source),
    };
    // The authority (where userinfo lives) ends at the first `/`; a password may hold `:`.
    let end = rest.find('/').unwrap_or(rest.len());
    match rest[..end].rfind('@') {
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
    let root_same = match (&stored.root_file_id, &now.root_file_id) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    root_same && sample_matches(&stored.sample, &now.sample) * 2 >= stored.sample.len()
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
