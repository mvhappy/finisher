//! The Windows opener (Decision 19; see [`super`] for the three steps).
//!
//! 1. **Walk without following:** each directory component is opened from its parent's
//!    handle by raw name with `FILE_OPEN_REPARSE_POINT` ([`crate::source::win`]); one that
//!    is a name-surrogate reparse point (junction, symlink, mount point) is
//!    [`OpenError::NameSurrogate`], a placeholder directory [`OpenError::CloudOnly`] unless
//!    `hydrate`.
//! 2. **Re-check without opening:** a directory query on the parent for the exact long name
//!    (never an 8.3 short name or another case): a placeholder (`RECALL_ON_DATA_ACCESS`,
//!    `RECALL_ON_OPEN`, `OFFLINE`, `UNPINNED`) is `CloudOnly` unless `hydrate`; a
//!    name-surrogate tag `NameSurrogate`; anything but a file `Changed`.
//! 3. **Open** from the parent's handle **without** `FILE_OPEN_REPARSE_POINT` (data-bearing
//!    filters, dedup and WOF, serve the data), `FILE_READ_DATA` sharing read, write and
//!    delete; then on the handle: no name-surrogate tag and no recall bit
//!    (`FileAttributeTagInfo`), `GetFinalPathNameByHandleW` under the root's own final path
//!    and ending in the requested name ([`OpenError::Escapes`], [`OpenError::Changed`]), the
//!    volume ([`Expect::check_volume`] against the serial), and size, `LastWriteTime` and
//!    the 128-bit id ([`Expect::matches`]).
//!
//! **Residual window:** a provider may evict the file between (2) and (3); the handle's
//! attributes are read again before it is returned, but Windows has no process-wide policy
//! that fails a read of an evicted file, so an eviction after that check and before the read
//! would hydrate it.

use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, FILE_NAME_OPENED, VOLUME_NAME_NT,
};

use super::stub_names;
use crate::placeholder::{self, win::FILE_ATTRIBUTE_REPARSE_POINT};
use crate::reparse;
use crate::source::win;
use crate::source::windows::{NativeSource, WinDir, open_root_dir};
use crate::source::{Attrs, RawEntry};
use crate::types::{EntryType, Expect, OpenError};

/// Win32 errors → [`OpenError`] for an open that follows no link.
pub(crate) fn map_error(e: io::Error) -> OpenError {
    match e.raw_os_error() {
        // FILE_NOT_FOUND, PATH_NOT_FOUND, INVALID_NAME, DIRECTORY (a file where a directory
        // was expected), BAD_NETPATH.
        Some(2 | 3 | 123 | 267) => OpenError::NotFound,
        // STOPPED_ON_SYMLINK, CANT_ACCESS_FILE (an unreadable reparse point).
        Some(681 | 1920) => OpenError::NameSurrogate,
        // A cloud file the provider would have to fetch.
        Some(358..=406) => OpenError::CloudOnly,
        _ => OpenError::Io(e),
    }
}

fn is_surrogate(attributes: u32, tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse::is_name_surrogate(tag)
}

fn is_placeholder(attributes: u32) -> bool {
    placeholder::from_attrs(Attrs::Windows { attributes }).is_some()
}

fn is_placeholder_dir(attributes: u32) -> bool {
    placeholder::dir_from_attrs(Attrs::Windows { attributes }).is_some()
}

/// One directory component, opened as itself.
fn open_component(parent: &WinDir, name: &[u8], hydrate: bool) -> Result<WinDir, OpenError> {
    let handle = win::open_dir_at(parent.handle.as_raw_handle(), name).map_err(map_error)?;
    let (attributes, tag) = win::attribute_tag(&handle)?;
    if is_surrogate(attributes, tag) {
        return Err(OpenError::NameSurrogate);
    }
    if is_placeholder_dir(attributes) && !hydrate {
        return Err(OpenError::CloudOnly);
    }
    Ok(WinDir {
        handle,
        serial: parent.serial,
    })
}

/// The re-check (step 2): the listed entry for `name`, or why there is none to open.
fn recheck(
    src: &NativeSource,
    parent: &WinDir,
    name: &[u8],
    hydrate: bool,
) -> Result<RawEntry, OpenError> {
    let e = match src.lookup(parent, name).map_err(map_error)? {
        Some(e) => e,
        None => return Err(missing(src, parent, name)),
    };
    if e.reparse_tag.is_some_and(reparse::is_name_surrogate) {
        return Err(OpenError::NameSurrogate);
    }
    if placeholder::from_attrs(e.attrs).is_some() && !hydrate {
        return Err(OpenError::CloudOnly);
    }
    if e.entry_type != EntryType::File {
        return Err(OpenError::Changed);
    }
    Ok(e)
}

/// A missing file with a stub listed for it is cloud-only; else it is gone.
fn missing(src: &NativeSource, parent: &WinDir, name: &[u8]) -> OpenError {
    for stub in stub_names(name) {
        if src
            .lookup(parent, &stub)
            .is_ok_and(|e| e.is_some_and(|e| e.entry_type == EntryType::File))
        {
            return OpenError::CloudOnly;
        }
    }
    OpenError::NotFound
}

/// The handle's final NT path (normalised where the file system can, else as opened).
fn nt_path(h: &impl AsRawHandle) -> io::Result<Vec<u16>> {
    win::final_path(h, FILE_NAME_NORMALIZED | VOLUME_NAME_NT)
        .or_else(|_| win::final_path(h, FILE_NAME_OPENED | VOLUME_NAME_NT))
}

fn fold(w: &[u16]) -> String {
    String::from_utf16_lossy(w).to_lowercase()
}

/// Whether `file` lies below `root` (final NT paths), and its last component is `name`. The
/// comparison ignores case: both paths come from the same file system, but a redirector may
/// give either the opened or the normalised case.
fn under_root(root: &[u16], file: &[u16], name: &[u16]) -> (bool, bool) {
    let (r, f) = (fold(root), fold(file));
    let r = r.trim_end_matches('\\');
    let below = f
        .strip_prefix(r)
        .is_some_and(|rest| rest.starts_with('\\') && rest.len() > 1);
    let last = f.rsplit('\\').next().unwrap_or("");
    (below, last == fold(name))
}

pub(super) fn open(
    root: &Path,
    dir: &[Vec<u8>],
    name: &[u8],
    expect: &Expect,
    hydrate: bool,
) -> Result<File, OpenError> {
    let root_dir = open_root_dir(root).map_err(map_error)?;
    let root_path = nt_path(&root_dir.handle)?;
    let src = NativeSource::new(root);
    let mut chain: Vec<WinDir> = Vec::with_capacity(dir.len());
    for c in dir {
        let next = open_component(chain.last().unwrap_or(&root_dir), c, hydrate)?;
        chain.push(next);
    }
    let parent = chain.last().unwrap_or(&root_dir);
    let wname = win::wide_name(name)?;

    // 2. Re-check without opening.
    recheck(&src, parent, name, hydrate)?;

    // 3. Open, then check the handle.
    let h: OwnedHandle = win::nt_open(
        parent.handle.as_raw_handle(),
        &wname,
        win::FILE_ACCESS,
        win::FILE_OPTIONS,
    )
    .map_err(map_error)?;
    let (attributes, tag) = win::attribute_tag(&h)?;
    if is_surrogate(attributes, tag) {
        return Err(OpenError::NameSurrogate);
    }
    if is_placeholder(attributes) && !hydrate {
        return Err(OpenError::CloudOnly);
    }
    let (below, same_name) = under_root(&root_path, &nt_path(&h)?, &wname);
    if !below {
        return Err(OpenError::Escapes);
    }
    if !same_name {
        return Err(OpenError::Changed);
    }
    let (serial, id) = win::file_id(&h)?;
    expect.check_volume(&win::dev_string(serial), || serial == root_dir.serial)?;
    let (size, is_dir) = win::size(&h)?;
    let mtime = win::mtime_ns(&h)?;
    if is_dir || !expect.matches(size, mtime, win::id_bytes(id).as_deref()) {
        return Err(OpenError::Changed);
    }
    Ok(File::from(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn under_root_by_component() {
        let root = w(r"\Device\HarddiskVolume3\Lib");
        assert_eq!(
            under_root(
                &root,
                &w(r"\Device\HarddiskVolume3\Lib\a\b.mov"),
                &w("b.mov")
            ),
            (true, true)
        );
        assert_eq!(
            under_root(&root, &w(r"\Device\HarddiskVolume3\lib\B.MOV"), &w("b.mov")),
            (true, true)
        );
        assert!(
            !under_root(
                &root,
                &w(r"\Device\HarddiskVolume3\Library\b.mov"),
                &w("b.mov")
            )
            .0
        );
        assert!(!under_root(&root, &w(r"\Device\HarddiskVolume4\Lib\b.mov"), &w("b.mov")).0);
        assert_eq!(
            under_root(
                &root,
                &w(r"\Device\HarddiskVolume3\Lib\LONGNA~1.MOV"),
                &w("longname.mov")
            ),
            (true, false)
        );
        let vol = w(r"\Device\HarddiskVolume3\");
        assert_eq!(
            under_root(&vol, &w(r"\Device\HarddiskVolume3\x"), &w("x")),
            (true, true)
        );
    }

    #[test]
    fn errors_map() {
        let m = |c| map_error(io::Error::from_raw_os_error(c));
        assert!(matches!(m(2), OpenError::NotFound));
        assert!(matches!(m(681), OpenError::NameSurrogate));
        assert!(matches!(m(395), OpenError::CloudOnly));
        assert!(matches!(m(5), OpenError::Io(_)));
    }
}
