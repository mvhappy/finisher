//! Pure classifiers for macOS and Linux paths and file systems (and the pieces shared with
//! Windows). Everything here takes already-gathered inputs so it is testable on any OS.

use std::path::{Component, Path, PathBuf};

use super::StorageKind;
use crate::conflict_names::Provider;

fn comps(p: &Path, fold: bool) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| {
            let s = c.as_os_str().to_string_lossy();
            if fold {
                s.to_lowercase()
            } else {
                s.into_owned()
            }
        })
        .collect()
}

/// Is `path` equal to or below `root`? Component-wise; `fold` compares case-insensitively
/// (macOS and Windows volumes). An empty root matches nothing.
pub fn path_under(path: &Path, root: &Path, fold: bool) -> bool {
    let (p, r) = (comps(path, fold), comps(root, fold));
    !r.is_empty() && p.len() >= r.len() && p[..r.len()] == r[..]
}

/// The provider named by a cloud-storage folder or client name: `OneDrive - Contoso`,
/// `GoogleDrive-me@x`, `Dropbox`, `iCloud Drive`, ... Anything else is `Unknown`.
pub fn provider_from_name(name: &str) -> Provider {
    let n = name.to_lowercase();
    if n.contains("onedrive") {
        Provider::OneDrive
    } else if n.contains("dropbox") {
        Provider::Dropbox
    } else if n.contains("google") {
        Provider::GoogleDrive
    } else if n.contains("icloud") {
        Provider::ICloud
    } else {
        Provider::Unknown
    }
}

/// A well-known sync client folder name in a home directory (`Dropbox (Personal)`,
/// `OneDrive - Contoso`, `My Drive`, `Nextcloud`, `MEGA`, `Box`, ...).
pub fn home_folder_provider(name: &str) -> Option<Provider> {
    let n = name.to_lowercase();
    let starts = |p: &str| n.starts_with(p);
    if starts("dropbox") {
        Some(Provider::Dropbox)
    } else if starts("onedrive") {
        Some(Provider::OneDrive)
    } else if starts("google drive") || n == "my drive" {
        Some(Provider::GoogleDrive)
    } else if starts("icloud") {
        Some(Provider::ICloud)
    } else if starts("nextcloud")
        || starts("owncloud")
        || n == "mega"
        || starts("mega ")
        || n == "box"
        || starts("box sync")
    {
        Some(Provider::Unknown)
    } else {
        None
    }
}

/// The provider of the well-known sync folder of `home` that `path` is in.
pub fn classify_home_folder(path: &Path, home: &Path, fold: bool) -> Option<Provider> {
    let (p, h) = (comps(path, fold), comps(home, fold));
    if h.is_empty() || p.len() <= h.len() || p[..h.len()] != h[..] {
        return None;
    }
    // Compare with the original case of the folder name for the provider lookup.
    let original = comps(path, false);
    home_folder_provider(&original[h.len()])
}

/// macOS (and any path with such a component sequence): `<x>/Library/Mobile Documents/**`
/// is iCloud, `<x>/Library/CloudStorage/<Provider>-*/**` that provider. Case-insensitive,
/// and independent of `$HOME`.
pub fn classify_macos_path(path: &Path) -> Option<Provider> {
    let c = comps(path, true);
    c.windows(2)
        .enumerate()
        .find_map(|(i, w)| match (w[0].as_str(), w[1].as_str()) {
            ("library", "mobile documents") => Some(Provider::ICloud),
            ("library", "cloudstorage") => {
                c.get(i + 2)
                    .map(|name| match name.split('-').next().unwrap_or("") {
                        "dropbox" => Provider::Dropbox,
                        "onedrive" => Provider::OneDrive,
                        "googledrive" => Provider::GoogleDrive,
                        _ => Provider::Unknown,
                    })
            }
            _ => None,
        })
}

/// iCloud's "Desktop & Documents" folders live in `com~apple~CloudDocs` and are reached
/// through links; `targets` are the canonicalised entries of that folder.
pub fn classify_icloud_redirect(path: &Path, targets: &[PathBuf]) -> Option<Provider> {
    targets
        .iter()
        .any(|t| path_under(path, t, true))
        .then_some(Provider::ICloud)
}

/// macOS `st_flags` bit for a file or folder whose data is not on this disk.
pub const SF_DATALESS: u32 = 0x4000_0000;

pub fn is_dataless(st_flags: u32) -> bool {
    st_flags & SF_DATALESS != 0
}

/// macOS `statfs.f_fstypename`: network file systems are `Network`; FUSE (macFUSE/osxfuse)
/// could be a cloud mount, so it is treated as a sync folder.
pub fn classify_macos_fs(fstypename: &str) -> StorageKind {
    let n = fstypename.to_ascii_lowercase();
    match n.as_str() {
        "smbfs" | "nfs" | "afpfs" | "webdav" => StorageKind::Network,
        _ if n.contains("fuse") => StorageKind::Sync(Provider::Unknown),
        _ => StorageKind::Local,
    }
}

pub const SMB_SUPER_MAGIC: u64 = 0x517B;
pub const SMB2_MAGIC_NUMBER: u64 = 0xFE53_4D42;
pub const CIFS_MAGIC_NUMBER: u64 = 0xFF53_4D42;
pub const NFS_SUPER_MAGIC: u64 = 0x6969;
pub const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;
pub const NINEP_MAGIC: u64 = 0x0102_1997;
pub const CEPH_SUPER_MAGIC: u64 = 0x00C3_6400;
pub const AFS_SUPER_MAGIC: u64 = 0x5346_414F;

/// Sync-folder markers found on the ancestors of a path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncMarkers {
    pub syncthing: bool,
    pub dropbox: bool,
    /// Nextcloud/ownCloud, Resilio, MEGA.
    pub other: bool,
}

impl SyncMarkers {
    pub fn provider(self) -> Option<Provider> {
        if self.syncthing {
            Some(Provider::Syncthing)
        } else if self.dropbox {
            Some(Provider::Dropbox)
        } else if self.other {
            Some(Provider::Unknown)
        } else {
            None
        }
    }
}

fn is_sync_db(name: &str) -> bool {
    // Nextcloud/ownCloud: `.sync_<hex>.db`, `._sync_<hex>.db`, legacy `.csync_journal.db`.
    name == ".csync_journal.db"
        || [".sync_", "._sync_"]
            .iter()
            .any(|p| name.strip_prefix(p).is_some_and(|r| r.ends_with(".db")))
}

/// Looks for sync markers in `path` and every ancestor. `stat` answers `Some(is_dir)` for
/// one candidate path; `list` names the entries of a directory.
pub fn find_markers(
    path: &Path,
    stat: impl Fn(&Path) -> Option<bool>,
    list: impl Fn(&Path) -> Vec<String>,
) -> SyncMarkers {
    let mut m = SyncMarkers::default();
    for dir in path.ancestors() {
        m.syncthing |= stat(&dir.join(".stfolder")).is_some()
            || stat(&dir.join(".stignore")).is_some()
            || stat(&dir.join(".stversions")).is_some();
        // `~/.dropbox` is the client's config directory: only a regular file marks a root.
        m.dropbox |= stat(&dir.join(".dropbox")) == Some(false);
        m.other |= stat(&dir.join(".sync")) == Some(true)
            || stat(&dir.join(".megaignore")).is_some()
            || stat(&dir.join(".debris")).is_some()
            || list(dir).iter().any(|n| is_sync_db(n));
    }
    m
}

/// The file-system type of the mount containing `path` in `/proc/self/mountinfo` text (the
/// deepest mount point wins; `\040`-style escapes are decoded).
pub fn mount_fstype(mountinfo: &str, path: &Path) -> Option<String> {
    fn unescape(s: &str) -> String {
        let (b, mut out, mut i) = (s.as_bytes(), Vec::new(), 0);
        while i < b.len() {
            let oct = (b[i] == b'\\' && i + 3 < b.len())
                .then(|| std::str::from_utf8(&b[i + 1..i + 4]).ok())
                .flatten()
                .and_then(|o| u8::from_str_radix(o, 8).ok());
            match oct {
                Some(v) => {
                    out.push(v);
                    i += 4;
                }
                None => {
                    out.push(b[i]);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let mut f = line.split(' ');
        let Some(mount_point) = f.nth(4) else {
            continue;
        };
        let mut rest = f.skip_while(|x| *x != "-");
        let Some(fstype) = rest.nth(1) else { continue };
        let mp = PathBuf::from(unescape(mount_point));
        if path_under(path, &mp, false) {
            let depth = mp.components().count();
            if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
                best = Some((depth, fstype.to_string()));
            }
        }
    }
    best.map(|(_, t)| t)
}

/// `/mnt/<drive>/Users/<user>/...` (WSL): the Windows home folder rules apply.
pub fn classify_wsl_path(path: &Path) -> Option<Provider> {
    let c = comps(path, true);
    let c: Vec<&str> = c.iter().map(String::as_str).collect();
    // Skip the root component: ["/", "mnt", "c", "users", "<user>", ...]. Windows spells the
    // root `\`; matching both keeps this pure function testable on every platform.
    let rest = match c.as_slice() {
        [root, "mnt", d, "users", _user, rest @ ..]
            if matches!(*root, "/" | "\\") && d.len() == 1 =>
        {
            rest
        }
        _ => return None,
    };
    let originals = comps(path, false);
    let name = originals.get(5)?;
    if rest.starts_with(&["appdata", "local", "google", "drivefs"]) {
        return Some(Provider::GoogleDrive);
    }
    home_folder_provider(name)
}

/// Linux: a sync marker means sync; else the `statfs` `f_type` decides. `fuse_subtype` is
/// the mount's file-system type from `/proc/self/mountinfo` (`fuse.sshfs`, `fuseblk`, ...).
pub fn classify_linux_fs(
    f_type: u64,
    markers: SyncMarkers,
    fuse_subtype: Option<&str>,
) -> StorageKind {
    if let Some(p) = markers.provider() {
        return StorageKind::Sync(p);
    }
    match f_type {
        SMB_SUPER_MAGIC | SMB2_MAGIC_NUMBER | CIFS_MAGIC_NUMBER | NFS_SUPER_MAGIC | NINEP_MAGIC
        | CEPH_SUPER_MAGIC | AFS_SUPER_MAGIC => StorageKind::Network,
        FUSE_SUPER_MAGIC => match fuse_subtype {
            Some("fuse.sshfs") => StorageKind::Network,
            // Block-backed (ntfs-3g, exfat): a local disk, not a cloud mount.
            Some("fuseblk") => StorageKind::Local,
            _ => StorageKind::Sync(Provider::Unknown),
        },
        _ => StorageKind::Local,
    }
}

/// The folder paths in a Dropbox `info.json` (`{"personal": {"path": ...}, ...}`). Anything
/// unparsable yields none.
pub fn parse_dropbox_info(json: &str) -> Vec<String> {
    let Ok(serde_json::Value::Object(accounts)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|v| v.get("path")?.as_str().map(str::to_string))
        .collect()
}
