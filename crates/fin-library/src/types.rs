//! The frozen wire types of plan 5D (Tasks 1–3, 7 and sub-projects 6, 8, 9 build on them).
//! TypeScript gets them generated (`@finisher/core-types`); raw bytes travel as base64 and
//! nanosecond times as decimal strings (see [`crate::raw`]).

use std::io;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::raw::{RawRel, b64, b64_opt, ns, ns_opt};

/// What an open must find (Decision 19 step 3): the catalogued size, mtime and, when the
/// catalog has one, file id. TS `ExpectWire`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expect {
    pub size: u64,
    #[serde(with = "ns")]
    #[schemars(with = "String")]
    pub mtime_ns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_opt")]
    #[schemars(with = "Option<String>")]
    pub file_id: Option<Vec<u8>>,
}

impl Expect {
    /// Whether an opened handle with these facts is the catalogued file: size and mtime equal,
    /// and the file id equal when both sides have one (a handle without an id, as some SMB
    /// servers give, is matched by size and mtime alone, Decision 22).
    pub fn matches(&self, size: u64, mtime_ns: i64, file_id: Option<&[u8]>) -> bool {
        size == self.size
            && mtime_ns == self.mtime_ns
            && match (&self.file_id, file_id) {
                (Some(want), Some(got)) => want.as_slice() == got,
                _ => true,
            }
    }
}

/// A cloud placeholder (Decision 12): catalogued from metadata only, never opened unless the
/// library has `hydrate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Placeholder {
    /// macOS `SF_DATALESS`, on a file or a directory.
    Dataless,
    /// A legacy iCloud `.<name>.icloud` stub, standing for `<name>`.
    IcloudStub,
    /// A Nextcloud `<name>.nextcloud` virtual file, standing for `<name>`.
    Nextcloud,
    /// An ownCloud `<name>.owncloud` virtual file, standing for `<name>`.
    Owncloud,
    /// Windows `FILE_ATTRIBUTE_RECALL_ON_OPEN`.
    RecallOnOpen,
    /// Windows `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`.
    RecallOnData,
    /// Windows `FILE_ATTRIBUTE_OFFLINE`.
    Offline,
    /// Windows `FILE_ATTRIBUTE_UNPINNED`: online-only even with the other bits clear.
    Unpinned,
}

impl Placeholder {
    /// The serde (catalog) name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dataless => "dataless",
            Self::IcloudStub => "icloudStub",
            Self::Nextcloud => "nextcloud",
            Self::Owncloud => "owncloud",
            Self::RecallOnOpen => "recallOnOpen",
            Self::RecallOnData => "recallOnData",
            Self::Offline => "offline",
            Self::Unpinned => "unpinned",
        }
    }
}

/// The type of a directory entry as listed (never by following a link). Windows reparse
/// points with a name-surrogate tag (symlinks, junctions, mount points) are `symlink` in a
/// [`ScanEntry`] (see [`crate::listing`]); anything that is neither a regular file, a
/// directory nor a link (sockets, devices, FIFOs) is `other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EntryType {
    File,
    Dir,
    Symlink,
    Other,
}

/// One entry of a directory listing, as a scan reports it (Decision 25 keys rows by
/// `(dir, name)` on raw bytes).
///
/// - `name`: the raw component the entry is opened by; for a Nextcloud, ownCloud or iCloud
///   stub it is the name the stub **stands for** (`placeholder` says which stub), so the row
///   survives hydration; a stub whose real file is also listed is `excluded` with
///   `STUB_SHADOWED` under its own name.
/// - `name_nfc`, `name_lossy`: the NFC display name ([`crate::names`]); `rel_key`: the
///   lookup key, `/`-joined and case-folded on case-insensitive roots. Neither is unique.
/// - `dev`, `file_id`: rename hints only (`dev:fileId`, Decision 22); `None` where the OS or
///   server gives none.
/// - `mount_root`: the entry is the root of a nested mount; `automount`: an automount point,
///   never descended (`excluded: AUTOMOUNT`).
/// - `excluded`: the exclusion code ([`crate::exclude::code`]) or `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanEntry {
    pub dir: RawRel,
    #[serde(with = "b64")]
    #[schemars(with = "String")]
    pub name: Vec<u8>,
    pub name_nfc: String,
    pub name_lossy: bool,
    pub rel_key: String,
    pub entry_type: EntryType,
    pub size: u64,
    #[serde(with = "ns")]
    #[schemars(with = "String")]
    pub mtime_ns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_opt")]
    #[schemars(with = "Option<String>")]
    pub file_id: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
    pub mount_root: bool,
    pub automount: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reparse_tag: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<Placeholder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded: Option<String>,
}

/// Closes one listing of `dir` (Decision 25): every entry of that listing precedes it in the
/// job's batches. `complete: false` (an error, a timeout, a placeholder or automount
/// directory) marks nothing missing; `code` says why. `entries` counts the listing's entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirDone {
    pub dir: RawRel,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "ns_opt")]
    #[schemars(with = "Option<String>")]
    pub mtime_ns: Option<i64>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub entries: u32,
}

/// One batch file of a scan (Decision 26): up to 4,096 entries, in `seq` order per job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanBatch {
    pub seq: u64,
    pub entries: Vec<ScanEntry>,
    pub dirs_done: Vec<DirDone>,
}

/// One top-level entry of a root, kept to confirm a volume change (Decision 17). No name:
/// entries are matched by file id, size and mtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleEntry {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_opt")]
    #[schemars(with = "Option<String>")]
    pub file_id: Option<Vec<u8>>,
    pub size: u64,
    #[serde(with = "ns")]
    #[schemars(with = "String")]
    pub mtime_ns: i64,
}

/// A root's volume (Decision 15), measured at add and refreshed at each full scan.
///
/// - `key`: BLAKE3-16 (hex) of the OS's identity fields ([`crate::volume::key`]); the raw
///   fields are never stored;
/// - `kind`: the filesystem type (`apfs`, `smbfs`, `ext4`, `nfs4`, `NTFS`, …);
/// - `dev_id`: the device id, informational only (never compared alone);
/// - `mount_root`: the root is a mount root (Decision 17's unmount rule);
/// - `enumeration`: how directories are read (`getdents64`, `getattrlistbulk`, a Windows
///   information class, `fake`);
/// - `root_file_id`, `sample`: the root's file id and up to 16 top-level entries, to confirm a
///   key change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VolumeIdentity {
    pub key: String,
    pub kind: String,
    pub dev_id: String,
    pub mount_root: bool,
    pub case_sensitive: bool,
    pub enumeration: String,
    pub remote: bool,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64_opt")]
    #[schemars(with = "Option<String>")]
    pub root_file_id: Option<Vec<u8>>,
    pub sample: Vec<SampleEntry>,
}

/// A root's state (Decision 17): `online` carries the identity measured now (its key may
/// differ from the stored one when the change was confirmed harmless); every other state
/// puts the library offline with its catalog intact (Decision 18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RootState {
    Online(VolumeIdentity),
    Unreachable { code: String },
    Unmounted,
    VolumeChanged { key: String },
    Empty,
    Timeout,
}

/// Why `open_for_read` (Tasks 2–3) refused or failed (Decision 19). Messages name no path.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// A placeholder, and the library does not hydrate.
    #[error("the file is a cloud placeholder whose data is not on this machine")]
    CloudOnly,
    /// The file's size, mtime or file id differs from what the catalog expects.
    #[error("the file changed since it was catalogued")]
    Changed,
    #[error("the file no longer exists")]
    NotFound,
    /// The path resolves outside the root (or across a mount, where that is refused).
    #[error("the path leaves the library root")]
    Escapes,
    /// A component or the file is a symlink, junction or mount point.
    #[error("the path goes through a link")]
    NameSurrogate,
    #[error("the file could not be opened: {0}")]
    Io(#[from] io::Error),
}
