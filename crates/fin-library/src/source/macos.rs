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
//!
//! **Mount points** are the one exception to "nothing per entry": their record describes the
//! covered directory, so each is re-stat'ed ([`mounted`]).

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, OwnedFd};

use libc::c_void;

use super::unix::{cvt, fstat, ino_id, lstat_at, ns, reopen_dir, retry};
use super::{Attrs, Listing, RawEntry, RootStat};
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

/// What one record says.
#[derive(Debug)]
enum Record {
    Entry(RawEntry),
    /// The entry vanished (`ENOENT`).
    Gone,
    /// The entry's attributes could not be read (its `ATTR_CMN_ERROR`): the other entries
    /// stand, the listing is incomplete.
    Failed(io::Error),
}

/// One record (`Err`: the record itself is malformed, which fails the listing).
fn parse(rec: &[u8], dir_dev: i64) -> io::Result<Record> {
    let returned_common = u32_at(rec, OFF_RETURNED)?;
    if returned_common & ATTR_CMN_ERROR != 0 {
        let err = u32_at(rec, OFF_ERROR)?;
        if err != 0 {
            if err as i32 == libc::ENOENT {
                return Ok(Record::Gone);
            }
            return Ok(Record::Failed(io::Error::from_raw_os_error(err as i32)));
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
    Ok(Record::Entry(RawEntry {
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

/// A directory's entries, in `getattrlistbulk` order. An entry whose attributes could not be
/// read (its `ATTR_CMN_ERROR`) makes the listing partial ([`Listing::collect`]); a malformed
/// record fails it. `progress` is called after each `getattrlistbulk` buffer.
pub(crate) fn list(dir: &OwnedFd, progress: &dyn Fn()) -> io::Result<Listing> {
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
    let mut records = Vec::new();
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
            records.push(parse(rec, dir_dev)?);
            p += len;
        }
        progress();
    }
    Listing::collect(
        records,
        |r| match r {
            Record::Entry(e) if e.mount_root && e.entry_type == EntryType::Dir && !e.automount => {
                mounted(dir, e)
            }
            Record::Entry(e) => Ok(Some(e)),
            Record::Gone => Ok(None),
            Record::Failed(e) => Err(e),
        },
        progress,
    )
}

/// A mount point's entry as the mounted volume's root. `getattrlistbulk` describes the
/// covered directory (its device is the parent's, its id and mtime the hidden directory's),
/// while `fstatat` of the name, like a poll's [`stat_child`] and Linux's `statx`, gives the
/// mounted root: re-stat it so the row's device, id and mtime are the volume's. Metadata
/// only; an automount trigger never reaches here.
fn mounted(dir: &OwnedFd, mut e: RawEntry) -> io::Result<Option<RawEntry>> {
    let c = super::unix::cname(&e.name)?;
    let st = match lstat_at(dir.as_raw_fd(), &c) {
        Ok(st) => st,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        // Replaced since the listing: the next listing tells.
        return Ok(Some(e));
    }
    e.dev = Some(i64::from(st.st_dev).to_string());
    e.file_id = Some(ino_id(st.st_ino));
    e.mtime_ns = ns(st.st_mtime, st.st_mtime_nsec);
    e.attrs = Attrs::Macos { flags: st.st_flags };
    Ok(Some(e))
}

/// One entry of `dir` by name, from `fstatat(AT_SYMLINK_NOFOLLOW)` (no listing, nothing
/// materialised: a stat reads metadata only); `None` when it is gone. A trigger directory is
/// not recognised here (only a listing's `ATTR_DIR_MOUNTSTATUS` sees it); polls stat only
/// directories a listing already descended, which were not triggers.
pub(crate) fn stat_child(dir: &OwnedFd, name: &[u8]) -> io::Result<Option<RawEntry>> {
    let dir_dev = i64::from(fstat(dir.as_raw_fd())?.st_dev);
    let c = super::unix::cname(name)?;
    let st = match super::unix::lstat_at(dir.as_raw_fd(), &c) {
        Ok(st) => st,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let entry_type = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => EntryType::File,
        libc::S_IFDIR => EntryType::Dir,
        libc::S_IFLNK => EntryType::Symlink,
        _ => EntryType::Other,
    };
    let dev = i64::from(st.st_dev);
    Ok(Some(RawEntry {
        name: name.to_vec(),
        entry_type,
        size: if entry_type == EntryType::Dir {
            0
        } else {
            st.st_size.max(0) as u64
        },
        mtime_ns: ns(st.st_mtime, st.st_mtime_nsec),
        file_id: Some(ino_id(st.st_ino)),
        dev: Some(dev.to_string()),
        attrs: Attrs::Macos { flags: st.st_flags },
        reparse_tag: None,
        mount_root: dev != dir_dev,
        automount: false,
    }))
}

/// An open directory's mtime.
pub(crate) fn dir_mtime(dir: &OwnedFd) -> io::Result<i64> {
    let st = fstat(dir.as_raw_fd())?;
    Ok(ns(st.st_mtime, st.st_mtime_nsec))
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
    // A root's identity does not depend on every entry: an unreadable one leaves the sample
    // to the others (the walker's listing of the root reports it).
    let entries = list(fd, &|| {})?.entries;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placeholder::{self, SF_DATALESS};
    use crate::types::Placeholder;

    /// Common attributes `parse` reads, then the type's tail.
    struct Rec<'a> {
        error: Option<u32>,
        name: &'a [u8],
        dev: i32,
        objtype: u32,
        flags: u32,
        tail: Vec<u8>,
    }

    /// One record laid out as `getattrlistbulk` packs it (see the module docs).
    fn bytes(r: &Rec) -> Vec<u8> {
        let mut b = vec![0u8; 4];
        let mut common = COMMON & !ATTR_CMN_ERROR;
        if r.error.is_some() {
            common |= ATTR_CMN_ERROR;
        }
        for w in [common, 0, 0, 0, 0] {
            b.extend_from_slice(&w.to_ne_bytes());
        }
        b.extend_from_slice(&r.error.unwrap_or(0).to_ne_bytes());
        let name_at = b.len();
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&r.dev.to_ne_bytes());
        b.extend_from_slice(&r.objtype.to_ne_bytes());
        b.extend_from_slice(&1_700_000_000i64.to_ne_bytes());
        b.extend_from_slice(&123i64.to_ne_bytes());
        b.extend_from_slice(&r.flags.to_ne_bytes());
        b.extend_from_slice(&42u64.to_ne_bytes());
        b.extend_from_slice(&r.tail);
        let off = (b.len() - name_at) as i32;
        let len = r.name.len() as u32 + 1;
        b[name_at..name_at + 4].copy_from_slice(&off.to_ne_bytes());
        b[name_at + 4..name_at + 8].copy_from_slice(&len.to_ne_bytes());
        b.extend_from_slice(r.name);
        b.push(0);
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let total = b.len() as u32;
        b[..4].copy_from_slice(&total.to_ne_bytes());
        b
    }

    fn file(name: &[u8], flags: u32, size: i64) -> Rec<'_> {
        Rec {
            error: Some(0),
            name,
            dev: 7,
            objtype: VREG,
            flags,
            tail: size.to_ne_bytes().to_vec(),
        }
    }

    fn entry(r: &Rec) -> RawEntry {
        match parse(&bytes(r), 7).unwrap() {
            Record::Entry(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn file_record_fields() {
        let e = entry(&file(b"a.mov", 0, 5 << 30));
        assert_eq!(e.name, b"a.mov");
        assert_eq!(e.entry_type, EntryType::File);
        assert_eq!(e.size, 5 << 30);
        assert_eq!(e.mtime_ns, 1_700_000_000_000_000_123);
        assert_eq!(e.file_id, Some(42u64.to_be_bytes().to_vec()));
        assert_eq!(e.dev.as_deref(), Some("7"));
        assert!(!e.mount_root && !e.automount);
    }

    #[test]
    fn dataless_flag_read_from_the_record() {
        let f = entry(&file(b"evicted.mov", SF_DATALESS, 99));
        assert_eq!(f.attrs, Attrs::Macos { flags: SF_DATALESS });
        assert_eq!(f.size, 99, "a dataless file keeps its logical size");
        assert_eq!(
            placeholder::from_attrs(f.attrs),
            Some(Placeholder::Dataless)
        );
        let d = entry(&Rec {
            error: Some(0),
            name: b"cloud",
            dev: 7,
            objtype: VDIR,
            flags: SF_DATALESS,
            tail: 0u32.to_ne_bytes().to_vec(),
        });
        assert_eq!(d.entry_type, EntryType::Dir);
        assert_eq!(
            placeholder::from_attrs(d.attrs),
            Some(Placeholder::Dataless)
        );
    }

    #[test]
    fn mount_status_and_device() {
        let dir = |status: u32, dev: i32| {
            entry(&Rec {
                error: Some(0),
                name: b"d",
                dev,
                objtype: VDIR,
                flags: 0,
                tail: status.to_ne_bytes().to_vec(),
            })
        };
        assert!(dir(DIR_MNTSTATUS_MNTPOINT, 7).mount_root);
        assert!(dir(DIR_MNTSTATUS_TRIGGER, 7).automount);
        assert!(dir(0, 8).mount_root, "another device is a mount root");
        let plain = dir(0, 7);
        assert!(!plain.mount_root && !plain.automount);
    }

    #[test]
    fn entry_errors() {
        let mut r = file(b"x", 0, 1);
        r.error = Some(libc::ENOENT as u32);
        assert!(matches!(parse(&bytes(&r), 7).unwrap(), Record::Gone));
        r.error = Some(libc::EACCES as u32);
        match parse(&bytes(&r), 7).unwrap() {
            Record::Failed(e) => assert_eq!(e.raw_os_error(), Some(libc::EACCES)),
            other => panic!("{other:?}"),
        }
        // The slot is read only when ATTR_CMN_ERROR is returned.
        r.error = None;
        assert!(matches!(parse(&bytes(&r), 7).unwrap(), Record::Entry(_)));
    }

    #[test]
    fn malformed_records_fail() {
        let mut b = bytes(&file(b"x", 0, 1));
        b[OFF_NAME..OFF_NAME + 4].copy_from_slice(&4096i32.to_ne_bytes());
        assert!(parse(&b, 7).is_err(), "name past the record");
        let short = bytes(&file(b"x", 0, 1));
        assert!(parse(&short[..OFF_FLAGS], 7).is_err(), "truncated");
    }
}
