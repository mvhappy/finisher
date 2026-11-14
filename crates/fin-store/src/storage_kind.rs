//! What kind of storage a project folder lives on (sub-project 4 spec §3.5):
//! `local | network | sync(provider)`. The kind picks the lease parameters, the poll cadence,
//! the compaction grace period and whether chain resolution may apply the epoch rule.
//!
//! [`detect`] splits into pure classifiers over already-gathered inputs (these compile and
//! are tested on every OS) and thin per-OS probes in `probe` that gather those inputs.
//!
//! Guiding principle: misdetecting a sync folder as local or network is the dangerous error
//! (it enables automatic lease takeover on what is only an advisory lease); the reverse is
//! merely annoying. Where the evidence is ambiguous the answer leans to sync.
//!
//! Decisions:
//! - A probe that fails (`statfs` error, unreadable drive type) yields `Network`, unless a
//!   sync signal was already found.
//! - On Linux FUSE is `Network` for sshfs, `Local` for block-backed `fuseblk`, and a sync
//!   folder (`unknown`) for everything else, including an unreadable subtype.
//! - A user override (`storageOverride` in `finisher.project.json`) is honoured, but one that
//!   weakens a detected sync folder to local or network is reported through the warning, and
//!   one that does not parse is ignored with a warning ([`resolve`]).

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::conflict_names::Provider;

mod classify;
mod classify_win;
pub mod local_disk;
mod probe;
#[cfg(test)]
mod tests;

pub use classify::*;
pub use classify_win::*;

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

/// Whether `kind` is a network share mounted by macOS: its client (smbfs; the measured case)
/// serves a directory listing from a cache for up to 60 s (`dir_cache_max`, `man nsmb.conf`),
/// to every process on the machine. The ordering rule waits longer there
/// (`tick::dir_cache_window`) and each scan refreshes the listings it follows from
/// (`scan::Scanner::scan_for`). A sync folder on macOS is a local file system (its provider
/// syncs it), so it is not affected.
pub fn slow_dir_cache(kind: StorageKind) -> bool {
    slow_dir_cache_on(kind, cfg!(target_os = "macos"))
}

/// [`slow_dir_cache`] as on macOS (`is_macos`) or elsewhere: apart from the `cfg`, so both
/// answers are tested on every OS.
pub fn slow_dir_cache_on(kind: StorageKind, is_macos: bool) -> bool {
    is_macos && kind == StorageKind::Network
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
            "{:?} is not a storage kind (local, network or sync[:<provider>])",
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
            // A provider we do not know fails strict (sync), not open.
            "sync" => Ok(StorageKind::Sync(Provider::Unknown)),
            other => other
                .strip_prefix("sync:")
                .map(|p| StorageKind::Sync(parse_provider(p).unwrap_or(Provider::Unknown)))
                .ok_or_else(|| ParseStorageKindError(s.to_string())),
        }
    }
}

impl Serialize for StorageKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Every provider (a new [`Provider`] variant fails this `match` until it is listed here).
const PROVIDERS: [Provider; 6] = [
    Provider::Dropbox,
    Provider::Syncthing,
    Provider::ICloud,
    Provider::OneDrive,
    Provider::GoogleDrive,
    Provider::Unknown,
];

const _: () = match Provider::Unknown {
    Provider::Dropbox
    | Provider::Syncthing
    | Provider::ICloud
    | Provider::OneDrive
    | Provider::GoogleDrive
    | Provider::Unknown => {}
};

/// Every serialised form: `local`, `network`, `sync:<provider>`.
pub fn all_storage_kinds() -> Vec<StorageKind> {
    let mut v = vec![StorageKind::Local, StorageKind::Network];
    v.extend(PROVIDERS.map(StorageKind::Sync));
    v
}

/// Its serde form is a string: one of [`all_storage_kinds`]'s.
impl schemars::JsonSchema for StorageKind {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "StorageKind".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let names: Vec<String> = all_storage_kinds().iter().map(|k| k.to_string()).collect();
        schemars::json_schema!({
            "description": "The project folder's storage kind: `local`, `network` or `sync:<provider>`.",
            "type": "string",
            "enum": names,
        })
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

/// The kind to use for `dir` given the raw `storageOverride` value of the project file.
/// The folder is always probed; a valid override is honoured, with a warning when it weakens
/// a detected sync folder. One that is not a storage-kind string is ignored and described.
pub fn resolve(
    dir: &Path,
    override_value: Option<&serde_json::Value>,
) -> (StorageKind, Option<String>) {
    combine(detect(dir, None), override_value)
}

/// The pure half of [`resolve`].
pub fn combine(
    detected: StorageKind,
    override_value: Option<&serde_json::Value>,
) -> (StorageKind, Option<String>) {
    let Some(value) = override_value else {
        return (detected, None);
    };
    let parsed = match value {
        serde_json::Value::String(s) => s.parse::<StorageKind>().map_err(|e| e.to_string()),
        other => Err(format!("{other} is not a string")),
    };
    match parsed {
        Ok(kind) if detected.is_sync() && !kind.is_sync() => (
            kind,
            Some(format!(
                "storageOverride {kind} replaces the detected {detected}: automatic lease takeover applies; \
                 two offline writers can diverge"
            )),
        ),
        Ok(kind) => (kind, None),
        Err(why) => (detected, Some(format!("storageOverride ignored: {why}"))),
    }
}
