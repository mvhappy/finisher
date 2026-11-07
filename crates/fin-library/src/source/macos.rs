//! macOS listings (Decision 6): `getattrlistbulk` on a fresh descriptor of the directory with
//! `ATTR_CMN_RETURNED_ATTRS | NAME | ERROR | DEVID | OBJTYPE | MODTIME | FLAGS | FILEID`,
//! `ATTR_DIR_MOUNTSTATUS` and `ATTR_FILE_DATALENGTH`, `FSOPT_PACK_INVAL_ATTRS`, a 256 KiB
//! buffer. Dataless files and directories are seen by `SF_DATALESS` in their flags without
//! being touched; nothing is opened per entry.
//!
//! **Layout.** Each record is `length`, `returned` (`attribute_set_t`), then the common
//! attributes in bit order except `ATTR_CMN_ERROR`, which comes right after `returned`; with
//! `FSOPT_PACK_INVAL_ATTRS` every requested common attribute is packed. The directory and
//! file sections are packed per object type; rather than trust that, the parser finds the end
//! of the fixed area from the name's `attrreference_t` (the name is the only variable-length
//! attribute, so its bytes start where the fixed area ends) and reads `ATTR_FILE_DATALENGTH`
//! as its last 8 bytes and `ATTR_DIR_MOUNTSTATUS` as the first 4 after `ATTR_CMN_FILEID`.

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, OwnedFd};

use libc::c_void;

use super::unix::{cvt, fstat, ino_id, lstat_at, ns, reopen_dir, retry};
use super::{Attrs, RawEntry, RootStat};
use crate::types::EntryType;
use crate::volume;

/// `<sys/attr.h>` values `libc` 0.2.191 lacks.
pub(crate) const ATTR_CMN_ERROR: u32 = 0x2000_0000;
pub(crate) const DIR_MNTSTATUS_MNTPOINT: u32 = 0x0000_0001;
pub(crate) const DIR_MNTSTATUS_TRIGGER: u32 = 0x0000_0002;
/// `<sys/vnode.h>` `enum vtype`.
pub(crate) const VREG: u32 = 1;
pub(crate) const VDIR: u32 = 2;
pub(crate) const VLNK: u32 = 5;

const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID
    | ATTR_CMN_ERROR;
const BUF: usize = 256 * 1024;

/// Offsets in one record (see the module docs).
const OFF_RETURNED: usize = 4;
const OFF_ERROR: usize = OFF_RETURNED + 20;
const OFF_NAME: usize = OFF_ERROR + 4;
const OFF_DEVID: usize = OFF_NAME + 8;
const OFF_OBJTYPE: usize = OFF_DEVID + 4;
const OFF_MODTIME: usize = OFF_OBJTYPE + 4;
const OFF_FLAGS: usize = OFF_MODTIME + 16;
const OFF_FILEID: usize = OFF_FLAGS + 4;
const OFF_TAIL: usize = OFF_FILEID + 8;

fn bad(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("getattrlistbulk: {what}"),
    )
}

fn rd<const N: usize>(rec: &[u8], at: usize) -> io::Result<[u8; N]> {
    rec.get(at..at + N)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| bad("record too short"))
}

fn u32_at(rec: &[u8], at: usize) -> io::Result<u32> {
    rd::<4>(rec, at).map(u32::from_ne_bytes)
}

fn i64_at(rec: &[u8], at: usize) -> io::Result<i64> {
    rd::<8>(rec, at).map(i64::from_ne_bytes)
}

/// What one record says (`None`: the entry vanished, `ENOENT`).
fn parse(rec: &[u8], dir_dev: i64) -> io::Result<Option<RawEntry>> {
    let returned_common = u32_at(rec, OFF_RETURNED)?;
    if returned_common & ATTR_CMN_ERROR != 0 {
        let err = u32_at(rec, OFF_ERROR)?;
        if err != 0 {
            if err as i32 == libc::ENOENT {
                return Ok(None);
            }
            return Err(io::Error::from_raw_os_error(err as i32));
        }
    }
    let name_off = i32::from_ne_bytes(rd::<4>(rec, OFF_NAME)?);
    let name_len = u32_at(rec, OFF_NAME + 4)? as usize;
    let start = usize::try_from(name_off)
        .ok()
        .and_then(|o| o.checked_add(OFF_NAME))
        .ok_or_else(|| bad("name offset"))?;
    let name = rec
        .get(start..start + name_len)
        .ok_or_else(|| bad("name out of the record"))?;
    let name = name.strip_suffix(&[0]).unwrap_or(name).to_vec();
    let fixed_end = start;
    if fixed_end < OFF_TAIL {
        return Err(bad("fixed area too short"));
    }
    let dev = i64::from(i32::from_ne_bytes(rd::<4>(rec, OFF_DEVID)?));
    let objtype = u32_at(rec, OFF_OBJTYPE)?;
    let sec = i64_at(rec, OFF_MODTIME)?;
    let nsec = i64_at(rec, OFF_MODTIME + 8)?;
    let flags = u32_at(rec, OFF_FLAGS)?;
    let fileid = u64::from_ne_bytes(rd::<8>(rec, OFF_FILEID)?);
    let tail = fixed_end - OFF_TAIL;
    let (entry_type, size, mount_status) = match objtype {
        VDIR => {
            let ms = if tail >= 4 { u32_at(rec, OFF_TAIL)? } else { 0 };
            (EntryType::Dir, 0, ms)
        }
        t => {
            let size = if tail >= 8 {
                i64_at(rec, fixed_end - 8)?.max(0) as u64
            } else {
                0
            };
            let ty = match t {
                VREG => EntryType::File,
                VLNK => EntryType::Symlink,
                _ => EntryType::Other,
            };
            (ty, size, 0)
        }
    };
    Ok(Some(RawEntry {
        name,
        entry_type,
        size,
        mtime_ns: ns(sec, nsec),
        file_id: Some(ino_id(fileid)),
        dev: Some(dev.to_string()),
        attrs: Attrs::Macos { flags },
        reparse_tag: None,
        mount_root: mount_status & DIR_MNTSTATUS_MNTPOINT != 0 || dev != dir_dev,
        automount: mount_status & DIR_MNTSTATUS_TRIGGER != 0,
    }))
}

/// A directory's entries, in `getattrlistbulk` order.
pub(crate) fn list(dir: &OwnedFd) -> io::Result<Vec<RawEntry>> {
    let dir_dev = i64::from(fstat(dir.as_raw_fd())?.st_dev);
    let fresh = reopen_dir(dir)?;
    // SAFETY: `attrlist` is plain data; all-zero is valid.
    let mut al: libc::attrlist = unsafe { MaybeUninit::zeroed().assume_init() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr = COMMON;
    al.dirattr = libc::ATTR_DIR_MOUNTSTATUS;
    al.fileattr = libc::ATTR_FILE_DATALENGTH;
    // u64 words: records are 4-byte aligned and read with byte copies.
    let mut buf = vec![0u64; BUF / 8];
    let mut out = Vec::new();
    loop {
        let n = retry(|| {
            // SAFETY: `al` and `buf` are valid for the sizes passed.
            cvt(unsafe {
                libc::getattrlistbulk(
                    fresh.as_raw_fd(),
                    (&raw mut al).cast::<c_void>(),
                    buf.as_mut_ptr().cast::<c_void>(),
                    BUF,
                    u64::from(libc::FSOPT_PACK_INVAL_ATTRS),
                )
            })
        })?;
        if n == 0 {
            break;
        }
        // SAFETY: `buf` is BUF bytes long and initialised.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), BUF) };
        let mut p = 0usize;
        for _ in 0..n {
            let len = u32_at(bytes, p)? as usize;
            let rec = bytes
                .get(p..p + len)
                .ok_or_else(|| bad("record past the buffer"))?;
            if let Some(e) = parse(rec, dir_dev)? {
                out.push(e);
            }
            p += len;
        }
    }
    Ok(out)
}

/// Whether an open directory is a mount point (`ATTR_DIR_MOUNTSTATUS`).
pub(crate) fn mount_status(fd: &OwnedFd) -> io::Result<u32> {
    // SAFETY: plain data, all-zero valid.
    let mut al: libc::attrlist = unsafe { MaybeUninit::zeroed().assume_init() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.dirattr = libc::ATTR_DIR_MOUNTSTATUS;
    let mut buf = [0u32; 4];
    retry(|| {
        // SAFETY: `al` and `buf` are valid for the sizes passed.
        cvt(unsafe {
            libc::fgetattrlist(
                fd.as_raw_fd(),
                (&raw mut al).cast::<c_void>(),
                buf.as_mut_ptr().cast::<c_void>(),
                std::mem::size_of_val(&buf),
                0,
            )
        })
    })?;
    Ok(buf[1])
}

/// The facts of a directory as a root (Decision 17).
pub(crate) fn root_stat(fd: &OwnedFd) -> io::Result<RootStat> {
    let me = fstat(fd.as_raw_fd())?;
    let parent = lstat_at(fd.as_raw_fd(), c"..").ok();
    let fields = volume::macos::fields(fd)?;
    let status = mount_status(fd).unwrap_or(0);
    let mount_root = status & DIR_MNTSTATUS_MNTPOINT != 0
        || parent.as_ref().is_some_and(|p| p.st_dev != me.st_dev);
    let entries = list(fd)?;
    Ok(RootStat {
        key: fields.key(),
        kind: fields.fstype.clone(),
        dev_id: me.st_dev.to_string(),
        parent_dev_id: parent.map(|p| p.st_dev.to_string()),
        mount_root_attr: Some(mount_root),
        case_sensitive: fields.case_sensitive,
        enumeration: "getattrlistbulk".into(),
        remote: fields.remote(),
        root_file_id: Some(ino_id(me.st_ino)),
        sample: volume::sample(&entries),
        entries: entries.len() as u64,
    })
}
