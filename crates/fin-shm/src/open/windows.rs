//! The Windows opener: every segment from the volume root down is opened with
//! `FILE_FLAG_OPEN_REPARSE_POINT` (so a junction, symlink or other reparse point is seen, not
//! followed) and refused if it is one; directories are held open without `FILE_SHARE_DELETE`
//! until the file is open, so no ancestor can be renamed or replaced during the walk; the
//! file must be a disk file (`GetFileType`), not a directory, device or reparse point, and its
//! final path (`GetFinalPathNameByHandleW`) must be the walked one.
//!
//! The refusal tests for junctions and reparse points are `#[cfg(windows)]` and need a Windows
//! run (`tests/refusals.rs`).

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, Prefix};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_NOT_SAME_DEVICE, ERROR_SHARING_VIOLATION, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
    FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, FileDispositionInfo, FileRenameInfo,
    GetFileInformationByHandle, GetFileType, GetFinalPathNameByHandleW, OPEN_EXISTING,
    SetFileInformationByHandle,
};

use super::{Mode, Opened, copy_exact, temp_name};
use crate::checks::{Facts, Fail, Kind, Reason, check_file};

/// Nothing: a Windows move works on the checked handle itself.
pub(crate) struct Anchor;

fn wide(s: &[u16]) -> Vec<u16> {
    let mut v = s.to_vec();
    v.push(0);
    v
}

fn last_error() -> Fail {
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) {
        return Fail {
            reason: Reason::Busy,
            message: "another handle has the file or a directory on its path open".into(),
            cause: Some(e),
        };
    }
    Fail::io(e)
}

fn create_file(
    path: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    flags: u32,
) -> Result<OwnedHandle, Fail> {
    let p = wide(path);
    // SAFETY: a NUL-terminated wide string; no security attributes or template.
    let h: HANDLE = unsafe {
        CreateFileW(
            p.as_ptr(),
            access,
            share,
            std::ptr::null(),
            disposition,
            flags,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: `CreateFileW` returned a new handle that we own.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
}

fn info(h: &impl AsRawHandle) -> Result<Facts, Fail> {
    // SAFETY: plain data, filled by the call.
    let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid handle and out pointer.
    if unsafe { GetFileInformationByHandle(h.as_raw_handle() as HANDLE, &mut i) } == 0 {
        return Err(last_error());
    }
    // SAFETY: a valid handle.
    let disk = unsafe { GetFileType(h.as_raw_handle() as HANDLE) } == FILE_TYPE_DISK;
    let a = i.dwFileAttributes;
    let kind = if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Kind::Link
    } else if a & FILE_ATTRIBUTE_DIRECTORY != 0 {
        Kind::Dir
    } else if a & FILE_ATTRIBUTE_DEVICE != 0 || !disk {
        Kind::Other
    } else {
        Kind::File
    };
    Ok(Facts {
        kind,
        nlink: u64::from(i.nNumberOfLinks),
        size: (u64::from(i.nFileSizeHigh) << 32) | u64::from(i.nFileSizeLow),
        id: (
            u64::from(i.dwVolumeSerialNumber),
            (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow),
        ),
    })
}

/// `path` as a verbatim (`\\?\`) path: the volume root, the directory segments and the name.
fn split(path: &Path) -> Result<(Vec<u16>, Vec<Vec<u16>>), Fail> {
    let mut root: Vec<u16> = Vec::new();
    let mut segments = Vec::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                let s = match p.kind() {
                    Prefix::Disk(d) | Prefix::VerbatimDisk(d) => format!(r"\\?\{}:", d as char),
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                        format!(
                            r"\\?\UNC\{}\{}",
                            server.to_string_lossy(),
                            share.to_string_lossy()
                        )
                    }
                    _ => {
                        return Err(Fail::new(
                            Reason::NotAbsolute,
                            "the path's prefix is not a drive or a share",
                        ));
                    }
                };
                root = s.encode_utf16().collect();
            }
            Component::RootDir => root.push(u16::from(b'\\')),
            Component::Normal(s) => segments.push(s.encode_wide().collect()),
            Component::CurDir | Component::ParentDir => {
                return Err(Fail::new(
                    Reason::NotAbsolute,
                    "the path has a `.` or `..` segment",
                ));
            }
        }
    }
    if root.len() < 2 {
        return Err(Fail::new(
            Reason::NotAbsolute,
            "the path must start with a drive or a share",
        ));
    }
    Ok((root, segments))
}

fn join(base: &[u16], seg: &[u16]) -> Vec<u16> {
    let mut v = base.to_vec();
    if v.last() != Some(&u16::from(b'\\')) {
        v.push(u16::from(b'\\'));
    }
    v.extend_from_slice(seg);
    v
}

/// Opens the volume root and every directory segment without following a reparse point and
/// without `FILE_SHARE_DELETE`; answers the held handles and the file's full verbatim path.
fn walk(path: &Path) -> Result<(Vec<OwnedHandle>, Vec<u16>), Fail> {
    let (root, mut segments) = split(path)?;
    let name = segments
        .pop()
        .ok_or_else(|| Fail::new(Reason::NotRegular, "the path is a root"))?;
    let mut held = Vec::with_capacity(segments.len() + 1);
    let mut cur = root;
    let mut dirs = vec![cur.clone()];
    for s in &segments {
        cur = join(&cur, s);
        dirs.push(cur.clone());
    }
    for d in &dirs {
        let flags = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
        let h = create_file(
            d,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_EXISTING,
            flags,
        )?;
        let shown = OsString::from_wide(d).to_string_lossy().into_owned();
        match info(&h)?.kind {
            Kind::Dir => {}
            Kind::Link => {
                return Err(Fail::new(
                    Reason::Link,
                    format!("the path segment `{shown}` is a reparse point"),
                ));
            }
            _ => {
                return Err(Fail::new(
                    Reason::NotRegular,
                    format!("the path segment `{shown}` is not a directory"),
                ));
            }
        }
        held.push(h);
    }
    Ok((held, join(&cur, &name)))
}

/// The handle's final path must be the walked one (case-insensitively): nothing redirected
/// the open.
fn check_final_path(h: &OwnedHandle, walked: &[u16]) -> Result<(), Fail> {
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: a valid handle and a buffer of `buf.len()` u16s; flags 0 = normalised, DOS volume name.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                h.as_raw_handle() as HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                0,
            )
        };
        if n == 0 {
            return Err(last_error());
        }
        if (n as usize) < buf.len() {
            buf.truncate(n as usize);
            break;
        }
        buf.resize(n as usize + 1, 0);
    }
    let lower = |w: &[u16]| OsString::from_wide(w).to_string_lossy().to_lowercase();
    if lower(&buf) != lower(walked) {
        return Err(Fail::new(
            Reason::Link,
            "the open was redirected away from the walked path",
        ));
    }
    Ok(())
}

fn finish(h: OwnedHandle, walked: &[u16], held: Vec<OwnedHandle>) -> Result<Opened, Fail> {
    let facts = info(&h)?;
    if facts.kind != Kind::File {
        check_file(&facts, u64::MAX)?;
    }
    check_final_path(&h, walked)?;
    drop(held);
    Ok(Opened {
        file: File::from(h),
        facts,
        anchor: Anchor,
    })
}

pub(crate) fn open(path: &Path, mode: Mode) -> Result<Opened, Fail> {
    let (held, full) = walk(path)?;
    let (access, share) = match mode {
        Mode::Read => (
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        ),
        Mode::ReadWrite => (
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        ),
        // No `FILE_SHARE_WRITE`: a file another handle can still write is `Busy`.
        Mode::Commit => (GENERIC_READ | DELETE, FILE_SHARE_READ),
    };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS;
    let h = create_file(&full, access, share, OPEN_EXISTING, flags)?;
    finish(h, &full, held)
}

pub(crate) fn create(path: &Path) -> Result<Opened, Fail> {
    let (held, full) = walk(path)?;
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let flags = FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT;
    let h = create_file(
        &full,
        GENERIC_READ | GENERIC_WRITE,
        share,
        CREATE_NEW,
        flags,
    )?;
    finish(h, &full, held)
}

/// Makes the directory `path` names below the checked walk; one already there is kept when it
/// is a real directory (a retried attempt's), refused when it is a reparse point or a file.
pub(crate) fn create_dir(path: &Path) -> Result<(), Fail> {
    let (held, full) = walk(path)?;
    let p = wide(&full);
    // SAFETY: a NUL-terminated wide string; default security attributes.
    if unsafe { CreateDirectoryW(p.as_ptr(), std::ptr::null()) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(Fail::io(e));
        }
    }
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let flags = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
    let h = create_file(&full, FILE_READ_ATTRIBUTES, share, OPEN_EXISTING, flags)?;
    let kind = info(&h)?.kind;
    drop(held);
    match kind {
        Kind::Dir => Ok(()),
        Kind::Link => Err(Fail::new(Reason::Link, "the directory is a reparse point")),
        _ => Err(Fail::new(Reason::NotRegular, "the path is not a directory")),
    }
}

/// Removes the entry `path` names (a link itself, not its target). The caller has just walked
/// the same path (`create` failed with `Exists`).
pub(crate) fn remove(path: &Path) -> Result<(), Fail> {
    std::fs::remove_file(path).map_err(Fail::io)
}

/// Renames the entry `from` names to `to_name` in the same directory (the caller has just
/// created `from` through the checked walk).
pub(crate) fn rename_within(from: &Path, to_name: &std::ffi::OsStr) -> Result<(), Fail> {
    let to = from.with_file_name(to_name);
    std::fs::rename(from, to).map_err(Fail::io)
}

pub(crate) fn facts(file: &File) -> Result<Facts, Fail> {
    info(file)
}

/// Renames the file behind `h` to `to` (replacing it): the move is of the checked handle, so
/// nothing can be swapped in between.
fn rename_handle(h: &File, to: &Path) -> io::Result<()> {
    let (root, segments) =
        split(to).map_err(|f| io::Error::new(io::ErrorKind::InvalidInput, f.message))?;
    let mut name = root;
    for s in &segments {
        name = join(&name, s);
    }
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let bytes = offset + (name.len() + 1) * 2;
    // u64s: aligned for the struct's handle field.
    let mut buf = vec![0u64; bytes.div_ceil(8)];
    let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buf` is zeroed, aligned and large enough for the header and the name + NUL.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = (name.len() * 2) as u32;
        let dst = buf.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>();
        std::ptr::copy_nonoverlapping(name.as_ptr(), dst, name.len());
    }
    // SAFETY: a valid handle and a buffer of `bytes` bytes holding a FILE_RENAME_INFO.
    let ok = unsafe {
        SetFileInformationByHandle(
            h.as_raw_handle() as HANDLE,
            FileRenameInfo,
            buf.as_ptr().cast(),
            bytes as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Deletes the file behind `h` (opened with `DELETE`) when the handle closes.
fn delete_on_close(h: &File) {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: a valid handle and a FILE_DISPOSITION_INFO. Best effort.
    unsafe {
        SetFileInformationByHandle(
            h.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
}

pub(crate) fn commit(src: &Opened, to: &Path, cap: u64, force_copy: bool) -> Result<(), Fail> {
    if !force_copy {
        match rename_handle(&src.file, to) {
            Ok(()) => {
                // No other handle can write it (no `FILE_SHARE_WRITE`), but a hard link may
                // have been made since the checks.
                let verdict = info(&src.file).and_then(|now| check_file(&now, cap));
                if verdict.is_err() {
                    delete_on_close(&src.file);
                }
                return verdict;
            }
            Err(e) if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE as i32) => {}
            Err(e) => return Err(Fail::io(e)),
        }
    }
    let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else {
        return Err(Fail::new(
            Reason::NotAbsolute,
            "the destination needs a directory and a name",
        ));
    };
    let tmp = dir.join(temp_name(name));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(Fail::io)?;
    let done = copy_exact(&src.file, src.facts.size, &mut out).and_then(|()| {
        drop(out);
        std::fs::rename(&tmp, to).map_err(Fail::io)
    });
    if let Err(f) = done {
        let _ = std::fs::remove_file(&tmp);
        return Err(f);
    }
    delete_on_close(&src.file);
    Ok(())
}
