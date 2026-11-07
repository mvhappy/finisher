//! What a scan reads a root through (Decision 4). Implementations: [`NativeSource`] (Linux
//! `getdents64` + `statx` and macOS `getattrlistbulk`, Task 2; Windows, Task 3) and [`fake`]
//! (feature `test-hooks`). A source **never opens a file**: it opens
//! directories (without following links, always by their raw component chain from the root)
//! and lists them with each entry's metadata.

use std::any::Any;
use std::fmt;
use std::io;

use crate::raw::RawRel;
use crate::types::{EntryType, SampleEntry, VolumeIdentity};

#[cfg(feature = "test-hooks")]
pub mod fake;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(unix)]
pub(crate) mod unix;

#[cfg(unix)]
pub use unix::NativeSource;

/// The OS attribute word of an entry, tagged by its OS family so the pure rules
/// ([`crate::placeholder`]) read it the same on every host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Attrs {
    /// No attribute word (Linux).
    #[default]
    None,
    /// macOS `ATTR_CMN_FLAGS` (`st_flags`: `SF_DATALESS`, …).
    Macos { flags: u32 },
    /// Windows `FILE_ATTRIBUTE_*` (from the directory information class).
    Windows { attributes: u32 },
}

/// One entry as the OS listed it. `name` is the raw component (Unix bytes; Windows UTF-16 as
/// WTF-8, [`crate::raw::from_wide`]); `mtime_ns` is nanoseconds since the Unix epoch;
/// `reparse_tag` is Windows' tag when `FILE_ATTRIBUTE_REPARSE_POINT` is set. An entry that
/// vanished between the listing and its stat is left out; any other per-entry failure fails
/// the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub name: Vec<u8>,
    pub entry_type: EntryType,
    pub size: u64,
    pub mtime_ns: i64,
    pub file_id: Option<Vec<u8>>,
    pub dev: Option<String>,
    pub attrs: Attrs,
    pub reparse_tag: Option<u32>,
    /// The entry is the root of a mount (Linux `STATX_ATTR_MOUNT_ROOT`; elsewhere its device
    /// differs from its directory's).
    pub mount_root: bool,
    /// Linux `STATX_ATTR_AUTOMOUNT`: an automount point, never descended (Decision 5).
    pub automount: bool,
}

impl RawEntry {
    /// A plain entry with no ids, attributes or flags (tests and sources fill the rest).
    pub fn new(name: impl Into<Vec<u8>>, entry_type: EntryType, size: u64, mtime_ns: i64) -> Self {
        Self {
            name: name.into(),
            entry_type,
            size,
            mtime_ns,
            file_id: None,
            dev: None,
            attrs: Attrs::None,
            reparse_tag: None,
            mount_root: false,
            automount: false,
        }
    }

    /// The entry as a [`SampleEntry`].
    pub fn sample(&self) -> SampleEntry {
        SampleEntry {
            file_id: self.file_id.clone(),
            size: self.size,
            mtime_ns: self.mtime_ns,
        }
    }
}

/// A root's facts, read without opening a file (Decision 17): the OS layer's measurement of
/// the volume plus what [`crate::volume::classify_root`] needs to judge it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootStat {
    /// [`crate::volume::key`] of this OS's identity fields, measured now.
    pub key: String,
    pub kind: String,
    pub dev_id: String,
    /// The parent directory's device (`None` for a filesystem root).
    pub parent_dev_id: Option<String>,
    /// Linux `STATX_ATTR_MOUNT_ROOT` when the kernel reports it; `None` elsewhere (then the
    /// device comparison decides).
    pub mount_root_attr: Option<bool>,
    pub case_sensitive: bool,
    pub enumeration: String,
    pub remote: bool,
    pub root_file_id: Option<Vec<u8>>,
    /// [`crate::volume::sample`] of the root's listing.
    pub sample: Vec<SampleEntry>,
    /// How many entries the root's listing holds.
    pub entries: u64,
}

impl RootStat {
    /// Whether the root is a mount root now ([`crate::volume::is_mount_root`]).
    pub fn mount_root(&self) -> bool {
        crate::volume::is_mount_root(
            self.mount_root_attr,
            &self.dev_id,
            self.parent_dev_id.as_deref(),
        )
    }

    /// The identity measured now.
    pub fn identity(&self) -> VolumeIdentity {
        VolumeIdentity {
            key: self.key.clone(),
            kind: self.kind.clone(),
            dev_id: self.dev_id.clone(),
            mount_root: self.mount_root(),
            case_sensitive: self.case_sensitive,
            enumeration: self.enumeration.clone(),
            remote: self.remote,
            root_file_id: self.root_file_id.clone(),
            sample: self.sample.clone(),
        }
    }
}

/// An open directory: its raw chain and the source's own handle (a file descriptor, a
/// Windows handle, the fake's marker).
pub struct DirHandle {
    rel: RawRel,
    os: Box<dyn Any + Send + Sync>,
}

impl DirHandle {
    pub fn new(rel: RawRel, os: impl Any + Send + Sync) -> Self {
        Self {
            rel,
            os: Box::new(os),
        }
    }

    pub fn rel(&self) -> &RawRel {
        &self.rel
    }

    /// The source's handle, if it is a `T`.
    pub fn os<T: 'static>(&self) -> Option<&T> {
        self.os.downcast_ref()
    }
}

impl fmt::Debug for DirHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirHandle")
            .field("depth", &self.rel.depth())
            .finish_non_exhaustive()
    }
}

/// A root's directories and their listings. Calls may block (a sleeping disk, a dead hard
/// mount): the walker (Task 4) runs them on its own threads with deadlines.
pub trait Source: Send + Sync {
    /// The root's facts; the first call of a scan is its "first stat" (60 s spin-up budget).
    fn stat_root(&self) -> io::Result<RootStat>;
    /// Opens a directory by its raw chain from the root, following no link.
    fn open_dir(&self, rel: &RawRel) -> io::Result<DirHandle>;
    /// Opens `name` below an open directory, following no link: the same directory as
    /// `open_dir(&parent.rel().join(name))`, without re-walking the chain (one open instead
    /// of one per component, which matters on network roots).
    fn open_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<DirHandle> {
        self.open_dir(&parent.rel().join(name))
    }
    /// Lists an open directory, in the OS's order.
    fn read_dir(&self, dir: &DirHandle) -> io::Result<Vec<RawEntry>>;
    /// [`Source::read_dir`], calling `progress` as the listing advances (after each batch of
    /// names and each entry's stat), so the walker's per-entry deadline (Decision 18: 30 s)
    /// restarts on every step instead of covering a whole large directory. The default calls
    /// `read_dir` without progress (the deadline then covers the whole call).
    fn read_dir_with(&self, dir: &DirHandle, progress: &dyn Fn()) -> io::Result<Vec<RawEntry>> {
        let _ = progress;
        self.read_dir(dir)
    }
    /// The entry `name` of an open directory, stat'ed without listing it, following no link
    /// and triggering no automount; `None` when it does not exist. Polls (Task 4) re-stat
    /// known directories with it. The default lists `parent` and picks the entry.
    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<Option<RawEntry>> {
        Ok(self.read_dir(parent)?.into_iter().find(|e| e.name == name))
    }
    /// An open directory's own mtime (ns since the Unix epoch), read before it is listed (the
    /// root's, and a targeted directory's); `None` when the source cannot tell, which makes
    /// a poll re-list it. The default answers `None`.
    fn dir_mtime(&self, dir: &DirHandle) -> io::Result<Option<i64>> {
        let _ = dir;
        Ok(None)
    }
    /// The volume an open directory is on (nested mounts).
    fn volume(&self, dir: &DirHandle) -> io::Result<VolumeIdentity>;
}
