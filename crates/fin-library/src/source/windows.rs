//! The Windows source (plan 5D Task 3, Decision 7): directories opened from their parent's
//! handle by raw name, never through a reparse point (`NtCreateFile(FILE_OPEN_REPARSE_POINT
//! | FILE_DIRECTORY_FILE)`, [`super::win`]), and listed from the directory handle alone with
//! `NtQueryDirectoryFileEx`: name, size (`EndOfFile`), `LastWriteTime`, attributes, the
//! reparse tag and the 128-bit file id, without opening or stat'ing any entry.
//!
//! - **Fallback chain** (per source, so per root): `FileIdExtdDirectoryInfo` →
//!   `FileIdBothDirectoryInfo` (64-bit ids; older SMB servers) → `FileFullDirectoryInfo`
//!   (no ids), stepped down at the first class refusal and recorded in
//!   `VolumeIdentity.enumeration`. In the last two the tag is in `EaSize`.
//! - **Reparse points:** a name-surrogate tag (symlink, junction, volume mount point, WSL
//!   symlink) is listed as [`EntryType::Symlink`] and never opened or followed (Decision 8),
//!   so a scan never leaves the root's volume and every entry's `dev` is its directory's
//!   volume serial; WSL special files and AF_UNIX sockets are [`EntryType::Other`]; every
//!   other tag (Cloud Files, dedup, WOF) is data-bearing and the entry keeps its type.
//! - **Placeholders** come from the attribute bits alone ([`crate::placeholder`]); a listing
//!   never opens an entry, so it never hydrates one.
//! - **Names** are the UTF-16 names as WTF-8 ([`crate::raw::from_wide`]); the long name only
//!   (no 8.3 short name is read or matched).
//! - **Directory mtimes** are read from each subdirectory's own handle ([`exact_dir_mtime`]):
//!   NTFS lists an entry's times from the copy in its parent's index, which for a directory
//!   lags behind until that directory is next opened and closed (measured: the first scan
//!   after files were added saw the old mtime, the second the new one), so a listed value
//!   would make the next poll re-list it and scans differ run to run. One extra relative open
//!   per subdirectory (never a placeholder one); a file's listed times are exact once it is
//!   closed.

use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use super::win::{self, Class, DirInfo, QueryBuf};
use super::{Attrs, DirHandle, Listing, RawEntry, RootStat, Source};
use crate::placeholder::win::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT};
use crate::raw::{RawRel, from_wide};
use crate::reparse;
use crate::types::{EntryType, VolumeIdentity};

/// `FILE_ATTRIBUTE_DEVICE`: reserved, never a file to read.
const FILE_ATTRIBUTE_DEVICE: u32 = 0x40;

/// An open directory of this source: its handle and its volume's serial.
#[derive(Debug)]
pub(crate) struct WinDir {
    pub handle: OwnedHandle,
    pub serial: u64,
}

/// The native source of a root on Windows. The root directory is opened by
/// [`Source::stat_root`] (the scan's first stat) and held for the scan; every directory is
/// reached from it by raw names, one relative open per directory.
#[derive(Debug)]
pub struct NativeSource {
    root: PathBuf,
    root_dir: Mutex<Option<Arc<WinDir>>>,
    /// The index in [`Class::ALL`] in use (the fallback chain only steps down).
    class: AtomicU8,
    /// Test hooks: the first this-many classes are refused as unsupported.
    refused: AtomicU8,
}

impl NativeSource {
    /// A source for `root` (an absolute, canonical directory). Nothing is opened yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            root_dir: Mutex::new(None),
            class: AtomicU8::new(0),
            refused: AtomicU8::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The information class listings use now (`VolumeIdentity.enumeration`).
    pub fn enumeration(&self) -> &'static str {
        self.current().name()
    }

    /// Test hooks: answer the first `n` classes of the fallback chain as unsupported (an old
    /// SMB server), so the chain is exercised on a local volume.
    #[cfg(feature = "test-hooks")]
    pub fn refuse_classes(self, n: u8) -> Self {
        self.refused.store(n.min(2), Ordering::Relaxed);
        self
    }

    fn current(&self) -> Class {
        Class::ALL[usize::from(self.class.load(Ordering::Relaxed).min(2))]
    }

    fn root_dir(&self) -> io::Result<Arc<WinDir>> {
        let mut g = self.root_dir.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(d) = g.as_ref() {
            return Ok(Arc::clone(d));
        }
        let d = Arc::new(open_root_dir(&self.root)?);
        *g = Some(Arc::clone(&d));
        Ok(d)
    }

    fn dir(dir: &DirHandle) -> io::Result<&Arc<WinDir>> {
        dir.os::<Arc<WinDir>>().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "not a handle of this source")
        })
    }

    /// One query sequence on a fresh handle of `dir`, stepping down the fallback chain when
    /// the class is refused. `name`: a single-entry lookup; `progress` after each buffer.
    fn query_all(
        &self,
        dir: &WinDir,
        name: Option<&[u16]>,
        progress: &dyn Fn(),
    ) -> io::Result<Vec<DirInfo>> {
        loop {
            let class = self.current();
            match self.query_class(dir, class, name, progress) {
                Err(e) if win::is_class_refusal(&e) && class != Class::Full => {
                    // Several threads may step at once: only ever down, one level per class.
                    let i = Class::ALL.iter().position(|&c| c == class).unwrap_or(2) as u8;
                    let _ =
                        self.class
                            .compare_exchange(i, i + 1, Ordering::Relaxed, Ordering::Relaxed);
                }
                r => return r,
            }
        }
    }

    fn query_class(
        &self,
        dir: &WinDir,
        class: Class,
        name: Option<&[u16]>,
        progress: &dyn Fn(),
    ) -> io::Result<Vec<DirInfo>> {
        let level = Class::ALL.iter().position(|&c| c == class).unwrap_or(2);
        if level < usize::from(self.refused.load(Ordering::Relaxed)) {
            return Err(io::Error::from_raw_os_error(87));
        }
        let fresh = win::reopen_dir(dir.handle.as_raw_handle())?;
        let mut buf = QueryBuf::new();
        let mut out = Vec::new();
        let mut first = true;
        while win::query(&fresh, &mut buf, class, first, name, &mut out)? {
            first = false;
            progress();
            if name.is_some() {
                break;
            }
        }
        Ok(out)
    }

    /// The entry `name` of `dir` by a directory query, never an open. The query also matches
    /// a short (8.3) name and, case-insensitively, another case: only an entry whose long
    /// name is exactly `name` answers.
    pub(crate) fn lookup(&self, dir: &WinDir, name: &[u8]) -> io::Result<Option<RawEntry>> {
        let w = win::wide_name(name)?;
        let found = if win::has_wildcard(&w) {
            self.query_all(dir, None, &|| {})?
        } else {
            self.query_all(dir, Some(&w), &|| {})?
        };
        Ok(found
            .into_iter()
            .find(|e| e.name == w)
            .map(|e| entry(&e, dir.serial)))
    }
}

/// Opens the root directory and reads its volume serial.
pub(crate) fn open_root_dir(root: &Path) -> io::Result<WinDir> {
    let handle = win::open_root(root)?;
    let (serial, _) = win::file_id(&handle)?;
    Ok(WinDir { handle, serial })
}

/// The type of a listed entry (Decision 8; see the module doc).
pub(crate) fn entry_type(attributes: u32, tag: Option<u32>) -> EntryType {
    match tag {
        Some(t) if reparse::is_name_surrogate(t) => EntryType::Symlink,
        Some(t) if reparse::is_special(t) => EntryType::Other,
        _ if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 => EntryType::Dir,
        _ if attributes & FILE_ATTRIBUTE_DEVICE != 0 => EntryType::Other,
        _ => EntryType::File,
    }
}

/// A query entry as a [`RawEntry`] on the volume `serial`.
pub(crate) fn entry(e: &DirInfo, serial: u64) -> RawEntry {
    let tag = (e.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(e.tag);
    let entry_type = entry_type(e.attributes, tag);
    RawEntry {
        name: from_wide(&e.name),
        entry_type,
        size: if entry_type == EntryType::File {
            e.size.max(0) as u64
        } else {
            0
        },
        mtime_ns: win::filetime_ns(e.write_time),
        file_id: e.file_id.clone(),
        dev: Some(win::dev_string(serial)),
        attrs: Attrs::Windows {
            attributes: e.attributes,
        },
        reparse_tag: tag,
        mount_root: false,
        automount: false,
    }
}

impl Source for NativeSource {
    fn stat_root(&self) -> io::Result<RootStat> {
        let d = Arc::new(open_root_dir(&self.root)?);
        let stat = self.dir_stat(&d)?;
        *self.root_dir.lock().unwrap_or_else(|p| p.into_inner()) = Some(d);
        Ok(stat)
    }

    fn open_dir(&self, rel: &RawRel) -> io::Result<DirHandle> {
        rel.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut cur = self.root_dir()?;
        for c in rel.components() {
            cur = Arc::new(open_child_dir(&cur, c)?);
        }
        Ok(DirHandle::new(rel.clone(), cur))
    }

    fn open_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<DirHandle> {
        let rel = parent.rel().join(name);
        rel.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let d = open_child_dir(Self::dir(parent)?, name)?;
        Ok(DirHandle::new(rel, Arc::new(d)))
    }

    fn read_dir(&self, dir: &DirHandle) -> io::Result<Vec<RawEntry>> {
        self.read_dir_with(dir, &|| {})?.strict()
    }

    fn read_dir_with(&self, dir: &DirHandle, progress: &dyn Fn()) -> io::Result<Listing> {
        let d = Self::dir(dir)?;
        let infos = self.query_all(d, None, progress)?;
        let mut out: Vec<RawEntry> = infos.iter().map(|e| entry(e, d.serial)).collect();
        for e in &mut out {
            if exact_dir_mtime(d, e) {
                progress();
            }
        }
        Ok(out.into())
    }

    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<Option<RawEntry>> {
        let d = Self::dir(parent)?;
        let mut e = self.lookup(d, name)?;
        if let Some(e) = e.as_mut() {
            exact_dir_mtime(d, e);
        }
        Ok(e)
    }

    fn dir_mtime(&self, dir: &DirHandle) -> io::Result<Option<i64>> {
        win::mtime_ns(&Self::dir(dir)?.handle).map(Some)
    }

    fn volume(&self, dir: &DirHandle) -> io::Result<VolumeIdentity> {
        self.dir_stat(Self::dir(dir)?).map(|s| s.identity())
    }
}

/// Replaces a listed subdirectory's mtime with its own (see the module doc); `true` when it
/// opened the directory. A placeholder directory is left as listed (opening it is avoided),
/// as is one that cannot be opened (its listing will say why).
pub(crate) fn exact_dir_mtime(parent: &WinDir, e: &mut RawEntry) -> bool {
    if e.entry_type != EntryType::Dir || crate::placeholder::dir_from_attrs(e.attrs).is_some() {
        return false;
    }
    let Ok(w) = win::wide_name(&e.name) else {
        return false;
    };
    let access = win::FILE_READ_ATTRIBUTES_ONLY;
    if let Ok(h) = win::nt_open(parent.handle.as_raw_handle(), &w, access, win::DIR_OPTIONS)
        && let Ok(m) = win::mtime_ns(&h)
    {
        e.mtime_ns = m;
    }
    true
}

/// Opens `name` below `parent` as a directory. A name-surrogate reparse point is refused
/// (`ERROR_STOPPED_ON_SYMLINK`), as is a placeholder directory (`ERROR_CLOUD_FILE_ACCESS_DENIED`):
/// listing it would make the provider populate it (Decision 13). The walker never asks for
/// either (they are not descended); a targeted or resumed directory may name one.
pub(crate) fn open_child_dir(parent: &WinDir, name: &[u8]) -> io::Result<WinDir> {
    let handle = win::open_dir_at(parent.handle.as_raw_handle(), name)?;
    let (attributes, tag) = win::attribute_tag(&handle)?;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse::is_name_surrogate(tag) {
        return Err(io::Error::from_raw_os_error(ERROR_STOPPED_ON_SYMLINK));
    }
    if crate::placeholder::dir_from_attrs(Attrs::Windows { attributes }).is_some() {
        return Err(io::Error::from_raw_os_error(ERROR_CLOUD_FILE_ACCESS_DENIED));
    }
    Ok(WinDir {
        handle,
        serial: parent.serial,
    })
}

/// `ERROR_STOPPED_ON_SYMLINK`, `ERROR_CLOUD_FILE_ACCESS_DENIED`.
pub(crate) const ERROR_STOPPED_ON_SYMLINK: i32 = 681;
pub(crate) const ERROR_CLOUD_FILE_ACCESS_DENIED: i32 = 395;

impl NativeSource {
    /// The facts of an open directory as a root (Decision 17): its volume
    /// ([`crate::volume::windows`]), its file id, and the sample of its listing.
    fn dir_stat(&self, d: &WinDir) -> io::Result<RootStat> {
        let fields = crate::volume::windows::fields(&d.handle)?;
        let infos = self.query_all(d, None, &|| {})?;
        let entries: Vec<RawEntry> = infos.iter().map(|e| entry(e, d.serial)).collect();
        Ok(RootStat {
            key: fields.key(),
            kind: fields.fs_name.clone(),
            dev_id: win::dev_string(fields.serial),
            parent_dev_id: None,
            mount_root_attr: Some(fields.volume_root),
            case_sensitive: fields.case_sensitive,
            enumeration: self.enumeration().into(),
            remote: fields.remote,
            root_file_id: win::id_bytes(fields.file_id),
            sample: crate::volume::sample(&entries),
            entries: entries.len() as u64,
        })
    }
}
