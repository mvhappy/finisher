//! The Windows system layer shared by the source, the opener and volume identity (plan 5D
//! Task 3): handles, **relative opens by raw name** (`NtCreateFile` with the parent's handle
//! as `RootDirectory`), directory queries (`NtQueryDirectoryFileEx`, [`super::win_dir`]) and
//! the facts of an open handle (`GetFileInformationByHandleEx`, `GetFinalPathNameByHandleW`).
//!
//! **Why NT relative opens:** a component is passed to the file system exactly as listed,
//! with no Win32 path parsing, so reserved names (`CON`, `NUL.txt`), trailing dots and
//! spaces, and paths far past `MAX_PATH` are opened as themselves; no component is ever
//! re-resolved from a path string, and a directory is one open from its parent (one round
//! trip on SMB). Only the root is opened by path, in its `\\?\` (verbatim) form.
//!
//! Every open here shares read, write and delete, so a scan or a read never blocks another
//! application (Decision 19).

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
    FILE_OPEN_REPARSE_POINT, FILE_SEQUENTIAL_ONLY, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, RtlNtStatusToDosError,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_CASE_SENSITIVE_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_REMOTE_PROTOCOL_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TRAVERSE, FileAttributeTagInfo,
    FileBasicInfo, FileCaseSensitiveInfo, FileIdInfo, FileRemoteProtocolInfo, FileStandardInfo,
    GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    GetVolumeInformationByHandleW, OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

pub(crate) use super::win_dir::{Class, DirInfo, QueryBuf, has_wildcard, is_class_refusal, query};

/// Share everything: never block another application's read, write, rename or delete.
pub(crate) const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
/// What a directory handle may do: list, traverse to children, read its own attributes.
pub(crate) const DIR_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
/// A handle that only reads attributes (a subdirectory's own mtime).
pub(crate) const FILE_READ_ATTRIBUTES_ONLY: u32 = FILE_READ_ATTRIBUTES | SYNCHRONIZE;
/// What a file handle of the opener may do.
pub(crate) const FILE_ACCESS: u32 = FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
/// A directory below the root: opened as itself, never through its reparse point.
pub(crate) const DIR_OPTIONS: u32 = FILE_DIRECTORY_FILE
    | FILE_OPEN_REPARSE_POINT
    | FILE_SYNCHRONOUS_IO_NONALERT
    | FILE_OPEN_FOR_BACKUP_INTENT;
/// The opener's file: **without** `FILE_OPEN_REPARSE_POINT`, so data-bearing filters (Data
/// Deduplication, WOF compression, a hydrated Cloud Files file) serve the data (Decision 19).
pub(crate) const FILE_OPTIONS: u32 =
    FILE_NON_DIRECTORY_FILE | FILE_SEQUENTIAL_ONLY | FILE_SYNCHRONOUS_IO_NONALERT;

/// `FILE_CS_FLAG_CASE_SENSITIVE_DIR` (`FILE_CASE_SENSITIVE_INFO.Flags`).
const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;

/// 100 ns intervals from 1601-01-01 to the Unix epoch.
pub(crate) const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;

/// A Windows `FILETIME`-style count (100 ns since 1601) as ns since the Unix epoch.
pub(crate) fn filetime_ns(t: i64) -> i64 {
    t.saturating_sub(EPOCH_DIFF_100NS).saturating_mul(100)
}

/// A file id as catalogue bytes: the 128-bit `FILE_ID_128` as stored (a 64-bit NTFS id is
/// zero-extended), `None` when the server gives none (all zeros, Decision 22).
pub(crate) fn id_bytes(id: [u8; 16]) -> Option<Vec<u8>> {
    (id != [0; 16]).then(|| id.to_vec())
}

/// A 64-bit id (`FileIdBothDirectoryInfo`) in the 128-bit form ([`id_bytes`]).
pub(crate) fn id64_bytes(id: i64) -> Option<Vec<u8>> {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&id.to_le_bytes());
    id_bytes(b)
}

/// The device string of a volume serial (`ScanEntry.dev`, `Expect.dev`).
pub(crate) fn dev_string(serial: u64) -> String {
    format!("{serial:016x}")
}

/// An `NTSTATUS` failure as the Win32 error it maps to.
pub(crate) fn nt_error(status: NTSTATUS) -> io::Error {
    // SAFETY: a pure mapping function.
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32)
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// A raw component (WTF-8) as UTF-16, refusing what is not one name: a separator, NUL, or
/// `:` (it would name an NTFS alternate data stream; no listed name holds one).
pub(crate) fn wide_name(name: &[u8]) -> io::Result<Vec<u16>> {
    let w = crate::raw::to_wide(name).ok_or_else(|| invalid("a name is not WTF-8"))?;
    if w.is_empty() || w.len() > 32_767 || w.iter().any(|&c| matches!(c, 0 | 0x5C | 0x2F | 0x3A)) {
        return Err(invalid("not a single name"));
    }
    Ok(w)
}

pub(crate) fn unicode(w: &[u16]) -> UNICODE_STRING {
    let bytes = (w.len() * 2) as u16;
    UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: w.as_ptr().cast_mut(),
    }
}

/// `NtCreateFile(FILE_OPEN)` of `name` relative to `parent` (an empty name reopens `parent`
/// itself, with its own enumeration state).
pub(crate) fn nt_open(
    parent: RawHandle,
    name: &[u16],
    access: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let us = unicode(name);
    let oa = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent as HANDLE,
        ObjectName: &us,
        Attributes: 0,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut h: HANDLE = ptr::null_mut();
    // SAFETY: an all-zero IO_STATUS_BLOCK is valid; it is only written by the call.
    let mut iosb: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: every pointer is valid for the call; the returned handle is owned below.
    let st = unsafe {
        NtCreateFile(
            &mut h,
            access,
            &oa,
            &mut iosb,
            ptr::null(),
            0,
            SHARE_ALL,
            FILE_OPEN,
            options,
            ptr::null(),
            0,
        )
    };
    if st < 0 {
        return Err(nt_error(st));
    }
    // SAFETY: NtCreateFile succeeded and returned a handle nobody else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as RawHandle) })
}

/// Opens `name` below `parent` as a directory, never through its reparse point.
pub(crate) fn open_dir_at(parent: RawHandle, name: &[u8]) -> io::Result<OwnedHandle> {
    nt_open(parent, &wide_name(name)?, DIR_ACCESS, DIR_OPTIONS)
}

/// A fresh handle of the same directory (its own enumeration position and search name).
pub(crate) fn reopen_dir(dir: RawHandle) -> io::Result<OwnedHandle> {
    nt_open(dir, &[], DIR_ACCESS, DIR_OPTIONS)
}

/// A root path in its verbatim form: `C:\x` → `\\?\C:\x`, `\\server\share` →
/// `\\?\UNC\server\share`; `/` becomes `\`. A path already verbatim is kept.
pub(crate) fn verbatim(root: &Path) -> Vec<u16> {
    let s: Vec<u16> = root
        .as_os_str()
        .encode_wide_vec()
        .into_iter()
        .map(|c| {
            if c == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                c
            }
        })
        .collect();
    let starts = |p: &str| s.starts_with(&p.encode_utf16().collect::<Vec<_>>());
    let mut out: Vec<u16> = if starts(r"\\?\") || starts(r"\\.\") {
        s
    } else if let Some(rest) = s.strip_prefix(&[0x5C, 0x5C][..]) {
        r"\\?\UNC\"
            .encode_utf16()
            .chain(rest.iter().copied())
            .collect()
    } else {
        r"\\?\".encode_utf16().chain(s.iter().copied()).collect()
    };
    out.push(0);
    out
}

trait EncodeWideVec {
    fn encode_wide_vec(&self) -> Vec<u16>;
}

impl EncodeWideVec for std::ffi::OsStr {
    fn encode_wide_vec(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

/// Opens the root directory by its path (the library's canonical root, resolved as given:
/// a root that is itself a junction or a mounted folder is followed, nothing below it is).
pub(crate) fn open_root(root: &Path) -> io::Result<OwnedHandle> {
    let path = verbatim(root);
    // SAFETY: `path` is NUL-terminated; the handle is owned below.
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            DIR_ACCESS,
            SHARE_ALL,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW succeeded and returned a handle nobody else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as RawHandle) })
}

/// `GetFileInformationByHandleEx(class)` into a `T`.
fn info<T>(h: &impl AsRawHandle, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    let mut v = std::mem::MaybeUninit::<T>::zeroed();
    // SAFETY: `v` is a valid out-buffer of the class's size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            h.as_raw_handle() as HANDLE,
            class,
            v.as_mut_ptr().cast(),
            std::mem::size_of::<T>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded and filled `v`.
    Ok(unsafe { v.assume_init() })
}

/// `FileIdInfo`: the 64-bit volume serial and the 128-bit file id.
pub(crate) fn file_id(h: &impl AsRawHandle) -> io::Result<(u64, [u8; 16])> {
    let i: FILE_ID_INFO = info(h, FileIdInfo)?;
    Ok((i.VolumeSerialNumber, i.FileId.Identifier))
}

/// `FileAttributeTagInfo`: the attributes and the reparse tag (0 without one).
pub(crate) fn attribute_tag(h: &impl AsRawHandle) -> io::Result<(u32, u32)> {
    let i: FILE_ATTRIBUTE_TAG_INFO = info(h, FileAttributeTagInfo)?;
    Ok((i.FileAttributes, i.ReparseTag))
}

/// The handle's `LastWriteTime` as ns since the Unix epoch.
pub(crate) fn mtime_ns(h: &impl AsRawHandle) -> io::Result<i64> {
    let i: FILE_BASIC_INFO = info(h, FileBasicInfo)?;
    Ok(filetime_ns(i.LastWriteTime))
}

/// The handle's size (`EndOfFile`) and whether it is a directory.
pub(crate) fn size(h: &impl AsRawHandle) -> io::Result<(u64, bool)> {
    let i: FILE_STANDARD_INFO = info(h, FileStandardInfo)?;
    Ok((i.EndOfFile.max(0) as u64, i.Directory))
}

/// Whether the directory is case-sensitive (Decision 10): NTFS reports
/// `FILE_CASE_SENSITIVE_SEARCH` for every volume, yet lookups are case-insensitive unless the
/// directory carries the per-directory flag; a file system without the class (FAT, SMB) is
/// case-insensitive.
pub(crate) fn case_sensitive(h: &impl AsRawHandle) -> bool {
    info::<FILE_CASE_SENSITIVE_INFO>(h, FileCaseSensitiveInfo)
        .is_ok_and(|i| i.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

/// `FileRemoteProtocolInfo` answers only for a handle on a network redirector.
pub(crate) fn remote_protocol(h: &impl AsRawHandle) -> Option<u32> {
    info::<FILE_REMOTE_PROTOCOL_INFO>(h, FileRemoteProtocolInfo)
        .ok()
        .map(|i| i.Protocol)
}

/// The file system's name (`NTFS`, `ReFS`, `FAT32`, `exFAT`, …).
pub(crate) fn fs_name(h: &impl AsRawHandle) -> io::Result<String> {
    let mut name = [0u16; 64];
    // SAFETY: `name` is a valid buffer of its length; the other outputs are optional.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            h.as_raw_handle() as HANDLE,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Ok(String::from_utf16_lossy(&name[..n]))
}

/// `GetFinalPathNameByHandleW(flags)` without the trailing NUL.
pub(crate) fn final_path(
    h: &impl AsRawHandle,
    flags: GETFINALPATHNAMEBYHANDLE_FLAGS,
) -> io::Result<Vec<u16>> {
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `buf` is a valid buffer of its length.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                h.as_raw_handle() as HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                flags,
            )
        } as usize;
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        if n < buf.len() {
            buf.truncate(n);
            return Ok(buf);
        }
        if buf.len() > 70_000 {
            return Err(io::Error::from_raw_os_error(
                ERROR_INSUFFICIENT_BUFFER as i32,
            ));
        }
        buf.resize(n + 1, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_epoch_and_saturation() {
        assert_eq!(filetime_ns(EPOCH_DIFF_100NS), 0);
        assert_eq!(filetime_ns(EPOCH_DIFF_100NS + 15), 1_500);
        assert_eq!(filetime_ns(0), i64::MIN, "before 1677 saturates");
        assert_eq!(filetime_ns(i64::MAX), i64::MAX);
    }

    #[test]
    fn ids_zero_is_none_and_64_bit_extends() {
        assert_eq!(id_bytes([0; 16]), None);
        let mut want = [0u8; 16];
        want[0] = 0x2A;
        assert_eq!(id64_bytes(0x2A), Some(want.to_vec()));
        assert_eq!(id64_bytes(0), None);
    }

    #[test]
    fn verbatim_forms() {
        let w = |s: &str| String::from_utf16(&verbatim(Path::new(s))).unwrap();
        assert_eq!(w(r"C:\x"), "\\\\?\\C:\\x\0");
        assert_eq!(w("C:/x/y"), "\\\\?\\C:\\x\\y\0");
        assert_eq!(w(r"\\srv\share\d"), "\\\\?\\UNC\\srv\\share\\d\0");
        assert_eq!(w(r"\\?\C:\x"), "\\\\?\\C:\\x\0");
    }

    #[test]
    fn names_refuse_separators() {
        assert!(wide_name(b"a\\b").is_err());
        assert!(wide_name(b"a/b").is_err());
        assert!(wide_name(b"a.mov:stream").is_err());
        assert!(wide_name(b"a.mov::$DATA").is_err());
        assert!(wide_name(b"").is_err());
        assert_eq!(
            wide_name(b"CON").unwrap(),
            "CON".encode_utf16().collect::<Vec<_>>()
        );
    }
}
