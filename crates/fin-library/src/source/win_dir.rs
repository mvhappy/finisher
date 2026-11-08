//! Directory queries for the Windows source and opener (plan 5D Task 3, Decision 7):
//! `NtQueryDirectoryFileEx` on a directory handle of its own, in one of three information
//! classes (the fallback chain), parsed from the buffer without opening any entry.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr;

use windows_sys::Wdk::Storage::FileSystem::NtQueryDirectoryFileEx;
use windows_sys::Win32::Foundation::{HANDLE, NTSTATUS, UNICODE_STRING};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::win::{id_bytes, id64_bytes, nt_error, unicode};

/// `SL_RESTART_SCAN`, `SL_RETURN_SINGLE_ENTRY` (`NtQueryDirectoryFileEx` query flags).
const SL_RESTART_SCAN: u32 = 1;
const SL_RETURN_SINGLE_ENTRY: u32 = 2;
/// `STATUS_NO_MORE_FILES`, `STATUS_NO_SUCH_FILE`: the end of a listing, or no entry by that
/// name.
const STATUS_NO_MORE_FILES: NTSTATUS = 0x8000_0006_u32 as NTSTATUS;
const STATUS_NO_SUCH_FILE: NTSTATUS = 0xC000_000F_u32 as NTSTATUS;

/// The directory information classes, best first (Decision 7's fallback chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// `FileIdExtdDirectoryInformation`: 128-bit ids and the reparse tag.
    Extd,
    /// `FileIdBothDirectoryInformation`: 64-bit ids; the tag is in `EaSize`.
    Both,
    /// `FileFullDirectoryInformation`: no ids; the tag is in `EaSize`.
    Full,
}

impl Class {
    pub(crate) const ALL: [Class; 3] = [Class::Extd, Class::Both, Class::Full];

    fn nt(self) -> i32 {
        match self {
            Class::Extd => 60,
            Class::Both => 37,
            Class::Full => 2,
        }
    }

    /// The name recorded in `VolumeIdentity.enumeration`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Class::Extd => "FileIdExtdDirectoryInfo",
            Class::Both => "FileIdBothDirectoryInfo",
            Class::Full => "FileFullDirectoryInfo",
        }
    }
}

/// One entry of a directory query, as the class gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirInfo {
    pub name: Vec<u16>,
    pub attributes: u32,
    /// `ReparsePointTag` (`Extd`) or `EaSize` (`Both`, `Full`): the tag when the reparse
    /// attribute is set.
    pub tag: u32,
    pub size: i64,
    pub write_time: i64,
    pub file_id: Option<Vec<u8>>,
}

/// The query buffer: 64 KiB, 8-byte aligned as the call requires.
pub(crate) struct QueryBuf(Vec<u64>);

impl QueryBuf {
    pub(crate) fn new() -> Self {
        Self(vec![0; 64 * 1024 / 8])
    }
}

fn rd<T: Copy>(b: &[u8], off: usize) -> T {
    assert!(
        off + std::mem::size_of::<T>() <= b.len(),
        "entry past the buffer"
    );
    // SAFETY: bounds checked above; the read is unaligned-safe.
    unsafe { ptr::read_unaligned(b.as_ptr().add(off).cast::<T>()) }
}

/// Parses one buffer of `class` entries (the layouts of `FILE_ID_EXTD_DIR_INFO`,
/// `FILE_ID_BOTH_DIR_INFO` and `FILE_FULL_DIR_INFO`), `.` and `..` left out.
pub(crate) fn parse(b: &[u8], class: Class, out: &mut Vec<DirInfo>) {
    // Offsets common to the three layouts.
    const WRITE: usize = 24;
    const EOF: usize = 40;
    const ATTRS: usize = 56;
    const NAME_LEN: usize = 60;
    const EA: usize = 64;
    let mut off = 0usize;
    loop {
        let e = &b[off..];
        let next: u32 = rd(e, 0);
        let name_len = rd::<u32>(e, NAME_LEN) as usize;
        let (name_off, tag, file_id) = match class {
            Class::Extd => (88, rd::<u32>(e, 68), id_bytes(rd::<[u8; 16]>(e, 72))),
            Class::Both => (104, rd::<u32>(e, EA), id64_bytes(rd::<i64>(e, 96))),
            Class::Full => (68, rd::<u32>(e, EA), None),
        };
        assert!(name_off + name_len <= e.len(), "name past the buffer");
        let name: Vec<u16> = (0..name_len / 2)
            .map(|i| rd::<u16>(e, name_off + 2 * i))
            .collect();
        if name != [0x2E] && name != [0x2E, 0x2E] {
            out.push(DirInfo {
                name,
                attributes: rd(e, ATTRS),
                tag,
                size: rd(e, EOF),
                write_time: rd(e, WRITE),
                file_id,
            });
        }
        if next == 0 {
            return;
        }
        off += next as usize;
    }
}

/// One `NtQueryDirectoryFileEx` call on `dir` (a handle of its own: the position and search
/// name live in it). `Ok(false)` at the end of the listing or when no entry matches `name`.
pub(crate) fn query(
    dir: &impl AsRawHandle,
    buf: &mut QueryBuf,
    class: Class,
    restart: bool,
    name: Option<&[u16]>,
    out: &mut Vec<DirInfo>,
) -> io::Result<bool> {
    // SAFETY: an all-zero IO_STATUS_BLOCK is valid.
    let mut iosb: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let us = name.map(unicode);
    let mut flags = if restart { SL_RESTART_SCAN } else { 0 };
    if name.is_some() {
        flags |= SL_RETURN_SINGLE_ENTRY;
    }
    let bytes = buf.0.len() * 8;
    // SAFETY: the buffer, status block and name are valid for the synchronous call.
    let st = unsafe {
        NtQueryDirectoryFileEx(
            dir.as_raw_handle() as HANDLE,
            ptr::null_mut(),
            None,
            ptr::null(),
            &mut iosb,
            buf.0.as_mut_ptr().cast(),
            bytes as u32,
            class.nt(),
            flags,
            us.as_ref()
                .map_or(ptr::null(), |u| u as *const UNICODE_STRING),
        )
    };
    if st == STATUS_NO_MORE_FILES || st == STATUS_NO_SUCH_FILE {
        return Ok(false);
    }
    if st < 0 {
        return Err(nt_error(st));
    }
    let n = iosb.Information.min(bytes);
    if n == 0 {
        return Ok(false);
    }
    // SAFETY: the u64 buffer viewed as its `n` filled bytes.
    let b = unsafe { std::slice::from_raw_parts(buf.0.as_ptr().cast::<u8>(), n) };
    parse(b, class, out);
    Ok(true)
}

/// Whether an error means "this information class is not supported here" (the fallback
/// chain's trigger: `ERROR_INVALID_PARAMETER`, `ERROR_NOT_SUPPORTED`, `ERROR_INVALID_LEVEL`,
/// `ERROR_INVALID_FUNCTION`).
pub(crate) fn is_class_refusal(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(87 | 50 | 124 | 1))
}

/// Whether the UTF-16 `name` holds a character the directory query reads as a wildcard
/// (`*`, `?`, `<`, `>`, `"`): such a name is looked up by listing instead.
pub(crate) fn has_wildcard(name: &[u16]) -> bool {
    name.iter()
        .any(|&c| matches!(c, 0x2A | 0x3F | 0x3C | 0x3E | 0x22))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::win::{EPOCH_DIFF_100NS, filetime_ns};

    /// The three layouts, built byte by byte: name, attributes, tag (or `EaSize`), size,
    /// time and ids land where the structs put them, and `.`/`..` are dropped.
    #[test]
    fn parse_layouts() {
        fn entry(class: Class, name: &str, next: bool) -> Vec<u8> {
            let name_off = match class {
                Class::Extd => 88,
                Class::Both => 104,
                Class::Full => 68,
            };
            let w: Vec<u16> = name.encode_utf16().collect();
            let len = (name_off + w.len() * 2).div_ceil(8) * 8;
            let mut e = vec![0u8; len];
            if next {
                e[0..4].copy_from_slice(&(len as u32).to_le_bytes());
            }
            e[24..32].copy_from_slice(&(EPOCH_DIFF_100NS + 7).to_le_bytes());
            e[40..48].copy_from_slice(&1234i64.to_le_bytes());
            e[56..60].copy_from_slice(&0x420u32.to_le_bytes());
            e[60..64].copy_from_slice(&((w.len() * 2) as u32).to_le_bytes());
            match class {
                Class::Extd => {
                    e[68..72].copy_from_slice(&0x9000_001Au32.to_le_bytes());
                    e[72] = 9;
                }
                Class::Both => {
                    e[64..68].copy_from_slice(&0x9000_001Au32.to_le_bytes());
                    e[96..104].copy_from_slice(&9i64.to_le_bytes());
                }
                Class::Full => e[64..68].copy_from_slice(&0x9000_001Au32.to_le_bytes()),
            }
            for (i, c) in w.iter().enumerate() {
                e[name_off + 2 * i..name_off + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
            }
            e
        }
        for class in Class::ALL {
            let mut b = entry(class, ".", true);
            b.extend(entry(class, "..", true));
            b.extend(entry(class, "clip.mov", false));
            let mut out = Vec::new();
            parse(&b, class, &mut out);
            assert_eq!(out.len(), 1, "{class:?}");
            let e = &out[0];
            assert_eq!(String::from_utf16(&e.name).unwrap(), "clip.mov");
            assert_eq!((e.attributes, e.tag, e.size), (0x420, 0x9000_001A, 1234));
            assert_eq!(filetime_ns(e.write_time), 700);
            let mut id = [0u8; 16];
            id[0] = 9;
            let want = (class != Class::Full).then(|| id.to_vec());
            assert_eq!(e.file_id, want, "{class:?}");
        }
    }
}
