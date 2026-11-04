//! What kind of storage a project folder lives on (sub-project 4 spec §3.5):
//! `local | network | sync(provider)`. The kind picks the lease parameters, the poll cadence,
//! the compaction grace period and whether chain resolution may apply the epoch rule.
//!
//! [`detect`] splits into pure classifiers over already-gathered inputs (these compile and
//! are tested on every OS) and thin per-OS probes in `probe` that gather those inputs.
//!
//! Decisions:
//! - A probe that fails (`statfs` error, unreadable drive type) yields `Network`: it needs
//!   no sync-folder leniency and is the more conservative of the non-sync kinds.
//! - On Linux a FUSE mount that is not under a Dropbox/Syncthing marker is `Network`
//!   (sshfs, rclone and similar behave like a remote share).
//! - A user override (`storageOverride` in `finisher.project.json`) always wins; one that
//!   does not parse is ignored and reported through [`resolve`]'s warning.

use std::fmt;
use std::path::{Component, Path};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::conflict_names::Provider;

mod probe;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageKind {
    /// A local disk (also the default for platforms without a probe).
    #[default]
    Local,
    /// A network share (SMB, NFS, AFP, WebDAV, other remote or FUSE mounts).
    Network,
    /// A sync folder (iCloud Drive, Dropbox, OneDrive, Google Drive, Syncthing). Two offline
    /// writers can legitimately hold different epochs, so diverged lines are never resolved
    /// automatically.
    Sync(Provider),
}

impl StorageKind {
    pub fn is_sync(self) -> bool {
        matches!(self, StorageKind::Sync(_))
    }
}

fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Dropbox => "dropbox",
        Provider::Syncthing => "syncthing",
        Provider::ICloud => "icloud",
        Provider::OneDrive => "onedrive",
        Provider::GoogleDrive => "googledrive",
        Provider::Unknown => "unknown",
    }
}

fn parse_provider(s: &str) -> Option<Provider> {
    Some(match s {
        "dropbox" => Provider::Dropbox,
        "syncthing" => Provider::Syncthing,
        "icloud" => Provider::ICloud,
        "onedrive" => Provider::OneDrive,
        "googledrive" => Provider::GoogleDrive,
        "unknown" => Provider::Unknown,
        _ => return None,
    })
}

impl fmt::Display for StorageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageKind::Local => f.write_str("local"),
            StorageKind::Network => f.write_str("network"),
            StorageKind::Sync(p) => write!(f, "sync:{}", provider_name(*p)),
        }
    }
}

/// The text of a string that is not `local`, `network` or `sync:<provider>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseStorageKindError(pub String);

impl fmt::Display for ParseStorageKindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a storage kind (local, network or sync:<icloud|onedrive|googledrive|dropbox|syncthing|unknown>)",
            self.0
        )
    }
}

impl std::error::Error for ParseStorageKindError {}

impl FromStr for StorageKind {
    type Err = ParseStorageKindError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "local" => Ok(StorageKind::Local),
            "network" => Ok(StorageKind::Network),
            other => other
                .strip_prefix("sync:")
                .and_then(parse_provider)
                .map(StorageKind::Sync)
                .ok_or_else(|| ParseStorageKindError(s.to_string())),
        }
    }
}

impl Serialize for StorageKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for StorageKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The storage kind of `dir`: `override_` when given, else what the OS reports. Never fails.
pub fn detect(dir: &Path, override_: Option<StorageKind>) -> StorageKind {
    override_.unwrap_or_else(|| probe::probe(dir))
}

/// [`detect`] for the raw `storageOverride` text of a project file. An override that does
/// not parse is ignored (the project must still open) and described in the second value.
pub fn resolve(dir: &Path, override_text: Option<&str>) -> (StorageKind, Option<String>) {
    match override_text.map(str::parse::<StorageKind>) {
        Some(Ok(kind)) => (detect(dir, Some(kind)), None),
        Some(Err(e)) => (
            detect(dir, None),
            Some(format!("storageOverride ignored: {e}")),
        ),
        None => (detect(dir, None), None),
    }
}

// ---- pure classifiers ----

/// `rel` = the components of `path` after `root`, compared case-insensitively; `None` when
/// `path` is not under `root`.
fn strip_fold(path: &Path, root: &Path) -> Option<Vec<String>> {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let (p, r) = (norm(path), norm(root));
    if r.is_empty() || p.len() < r.len() || p[..r.len()] != r[..] {
        return None;
    }
    Some(p[r.len()..].to_vec())
}

fn cloud_storage_provider(dir_name: &str) -> Provider {
    // `OneDrive-Personal`, `GoogleDrive-<account>`, `Dropbox`, `Box-Box`, ...
    match dir_name.split('-').next().unwrap_or("") {
        "dropbox" => Provider::Dropbox,
        "onedrive" => Provider::OneDrive,
        "googledrive" => Provider::GoogleDrive,
        _ => Provider::Unknown,
    }
}

/// macOS: `~/Library/Mobile Documents/**` is iCloud, `~/Library/CloudStorage/<Provider>-*/**`
/// that provider. Case-insensitive (default macOS volumes).
pub fn classify_macos_path(path: &Path, home: &Path) -> Option<Provider> {
    let rel = strip_fold(path, home)?;
    match rel.as_slice() {
        [lib, mobile, ..] if lib == "library" && mobile == "mobile documents" => {
            Some(Provider::ICloud)
        }
        [lib, cs, name, ..] if lib == "library" && cs == "cloudstorage" => {
            Some(cloud_storage_provider(name))
        }
        _ => None,
    }
}

/// macOS `statfs.f_fstypename`: is it a network file system?
pub fn classify_macos_fs(fstypename: &str) -> StorageKind {
    match fstypename.to_ascii_lowercase().as_str() {
        "smbfs" | "nfs" | "afpfs" | "webdav" => StorageKind::Network,
        _ => StorageKind::Local,
    }
}

pub const SMB_SUPER_MAGIC: u64 = 0x517B;
pub const SMB2_MAGIC_NUMBER: u64 = 0xFE53_4D42;
pub const CIFS_MAGIC_NUMBER: u64 = 0xFF53_4D42;
pub const NFS_SUPER_MAGIC: u64 = 0x6969;
pub const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;

/// Sync-folder markers found on the ancestors of a Linux path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncMarkers {
    pub syncthing: bool,
    pub dropbox: bool,
}

impl SyncMarkers {
    pub fn provider(self) -> Option<Provider> {
        if self.syncthing {
            Some(Provider::Syncthing)
        } else if self.dropbox {
            Some(Provider::Dropbox)
        } else {
            None
        }
    }
}

/// Looks for a `.stfolder` or `.dropbox` entry in `path` and every ancestor; `exists`
/// answers for one candidate path.
pub fn find_markers(path: &Path, exists: impl Fn(&Path) -> bool) -> SyncMarkers {
    let mut m = SyncMarkers::default();
    for dir in path.ancestors() {
        m.syncthing |= exists(&dir.join(".stfolder"));
        m.dropbox |= exists(&dir.join(".dropbox"));
    }
    m
}

/// Linux: a marker ancestor means sync; else the `statfs` `f_type` decides (SMB, CIFS, NFS
/// and FUSE are network).
pub fn classify_linux_fs(f_type: u64, markers: SyncMarkers) -> StorageKind {
    if let Some(p) = markers.provider() {
        return StorageKind::Sync(p);
    }
    match f_type {
        SMB_SUPER_MAGIC | SMB2_MAGIC_NUMBER | CIFS_MAGIC_NUMBER | NFS_SUPER_MAGIC
        | FUSE_SUPER_MAGIC => StorageKind::Network,
        _ => StorageKind::Local,
    }
}

pub const DRIVE_REMOTE: u32 = 4;
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// Windows path text normalised for comparison: `\\?\` and `\\?\UNC\` forms resolved,
/// separators `\`, lower-cased, no trailing separator.
pub fn normalize_windows(p: &str) -> String {
    let s = p.replace('/', "\\");
    let s = if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        s
    };
    let s = s.to_lowercase();
    let t = s.trim_end_matches('\\');
    if t.is_empty() { s } else { t.to_string() }
}

/// `\\server\share` (any form, including `\\?\UNC\`), not a `\\.\` device path.
pub fn is_unc(normalized: &str) -> bool {
    normalized.starts_with("\\\\") && !normalized.starts_with("\\\\.\\") && normalized.len() > 2
}

fn win_under(path: &str, root: &str) -> bool {
    let root = normalize_windows(root);
    !root.is_empty()
        && path
            .strip_prefix(root.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

/// Windows: UNC or `DRIVE_REMOTE` is network; else a path under a OneDrive root, a Dropbox
/// root, or a folder carrying the cloud-files recall attributes is sync.
pub fn classify_windows(
    path: &str,
    onedrive_roots: &[String],
    dropbox_roots: &[String],
    drive_type: Option<u32>,
    attrs: Option<u32>,
) -> StorageKind {
    let p = normalize_windows(path);
    if is_unc(&p) || drive_type == Some(DRIVE_REMOTE) {
        return StorageKind::Network;
    }
    if onedrive_roots.iter().any(|r| win_under(&p, r)) {
        return StorageKind::Sync(Provider::OneDrive);
    }
    if dropbox_roots.iter().any(|r| win_under(&p, r)) {
        return StorageKind::Sync(Provider::Dropbox);
    }
    let recall = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN;
    if attrs.is_some_and(|a| a & recall != 0) {
        return StorageKind::Sync(Provider::Unknown);
    }
    StorageKind::Local
}

/// The folder paths in a Dropbox `info.json` (`{"personal": {"path": ...}, "business": ...}`).
/// Anything unparsable yields none.
pub fn parse_dropbox_info(json: &str) -> Vec<String> {
    let Ok(serde_json::Value::Object(accounts)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|v| v.get("path")?.as_str().map(str::to_string))
        .collect()
}
