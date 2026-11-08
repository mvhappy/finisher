//! Linux listings (Decision 5): names by `fdopendir`/`readdir` (`getdents64`, which neither
//! opens nor stats an entry), then one `statx(dirfd, name, AT_SYMLINK_NOFOLLOW |
//! AT_NO_AUTOMOUNT)` per entry. Symlinks are reported as such and never followed; an
//! automount point is reported with `automount` (from `STATX_ATTR_AUTOMOUNT`) and is not
//! triggered, because `AT_NO_AUTOMOUNT` stops the walk at it; nested mounts carry
//! `mount_root` (`STATX_ATTR_MOUNT_ROOT`, which also sees bind mounts on one device).

use std::ffi::{CStr, CString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd, RawFd};

use libc::{c_int, c_uint};

use super::unix::{cname, cvt, ino_id, ns, reopen_dir, retry};
use super::{Attrs, Listing, RawEntry, RootStat};
use crate::types::EntryType;
use crate::volume;

/// What every `statx` here asks for: type, mode, owner, size, times, inode, and the mount id.
pub(crate) const MASK: c_uint = libc::STATX_BASIC_STATS | libc::STATX_MNT_ID;
/// The flags of every per-entry `statx`: no link followed, no automount triggered.
pub(crate) const ENTRY_FLAGS: c_int = libc::AT_SYMLINK_NOFOLLOW | libc::AT_NO_AUTOMOUNT;

/// `statx(dir, name, flags, MASK)`, retried on `EINTR`.
pub(crate) fn statx(dir: RawFd, name: &CStr, flags: c_int) -> io::Result<libc::statx> {
    let mut st = MaybeUninit::<libc::statx>::zeroed();
    retry(|| {
        // SAFETY: `name` is a valid C string and `st` a valid out-pointer.
        cvt(unsafe { libc::statx(dir, name.as_ptr(), flags, MASK, st.as_mut_ptr()) })
    })?;
    // SAFETY: statx succeeded and filled `st`.
    Ok(unsafe { st.assume_init() })
}

/// `statx` of an open descriptor itself.
pub(crate) fn statx_fd(fd: RawFd) -> io::Result<libc::statx> {
    statx(fd, c"", libc::AT_EMPTY_PATH | libc::AT_NO_AUTOMOUNT)
}

/// Whether the kernel reported `bit` (`STATX_ATTR_*`) as supported and set.
pub(crate) fn attr(st: &libc::statx, bit: c_int) -> bool {
    let b = bit as u64;
    st.stx_attributes_mask & b != 0 && st.stx_attributes & b != 0
}

/// `Some(set)` when the kernel reports `bit` at all, else `None`.
pub(crate) fn attr_known(st: &libc::statx, bit: c_int) -> Option<bool> {
    (st.stx_attributes_mask & bit as u64 != 0).then(|| attr(st, bit))
}

pub(crate) fn file_type(st: &libc::statx) -> EntryType {
    match u32::from(st.stx_mode) & libc::S_IFMT {
        libc::S_IFREG => EntryType::File,
        libc::S_IFDIR => EntryType::Dir,
        libc::S_IFLNK => EntryType::Symlink,
        _ => EntryType::Other,
    }
}

pub(crate) fn mtime_ns(st: &libc::statx) -> i64 {
    ns(st.stx_mtime.tv_sec, i64::from(st.stx_mtime.tv_nsec))
}

pub(crate) fn dev_id(st: &libc::statx) -> String {
    format!("{}:{}", st.stx_dev_major, st.stx_dev_minor)
}

/// Whether two `statx` results are on the same mount: by mount id when both have one (a bind
/// mount shares its device), else by device.
pub(crate) fn same_mount(a: &libc::statx, b: &libc::statx) -> bool {
    let id = |s: &libc::statx| s.stx_mask & libc::STATX_MNT_ID != 0;
    if id(a) && id(b) {
        a.stx_mnt_id == b.stx_mnt_id
    } else {
        (a.stx_dev_major, a.stx_dev_minor) == (b.stx_dev_major, b.stx_dev_minor)
    }
}

fn entry(name: Vec<u8>, st: &libc::statx) -> RawEntry {
    RawEntry {
        name,
        entry_type: file_type(st),
        size: st.stx_size,
        mtime_ns: mtime_ns(st),
        file_id: Some(ino_id(st.stx_ino)),
        dev: Some(dev_id(st)),
        attrs: Attrs::None,
        reparse_tag: None,
        mount_root: attr(st, libc::STATX_ATTR_MOUNT_ROOT),
        automount: attr(st, libc::STATX_ATTR_AUTOMOUNT),
    }
}

/// The raw names of a directory (without `.` and `..`), in `getdents64` order.
pub(crate) fn names(dir: &OwnedFd) -> io::Result<Vec<Vec<u8>>> {
    let fresh = reopen_dir(dir)?;
    // SAFETY: `fresh` is an open directory descriptor; `fdopendir` takes ownership of it.
    let d = unsafe { libc::fdopendir(fresh.into_raw_fd()) };
    if d.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    let res = loop {
        // SAFETY: resetting errno distinguishes the end of the stream from an error.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `d` is an open DIR stream.
        let e = unsafe { libc::readdir(d) };
        if e.is_null() {
            let err = io::Error::last_os_error();
            break match err.raw_os_error() {
                Some(0) | None => Ok(()),
                Some(libc::EINTR) => continue,
                _ => Err(err),
            };
        }
        // SAFETY: `d_name` of a returned entry is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*e).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            out.push(name.to_vec());
        }
    };
    // SAFETY: `d` is open and closed once.
    unsafe { libc::closedir(d) };
    res.map(|()| out)
}

/// Stats one listed name; `None` when it vanished since the listing.
fn stat_name(dir: &OwnedFd, name: &[u8]) -> io::Result<Option<libc::statx>> {
    let c: CString = cname(name)?;
    match statx(dir.as_raw_fd(), &c, ENTRY_FLAGS) {
        Ok(st) => Ok(Some(st)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A directory's entries; an entry that vanished between the listing and its stat is left
/// out, one whose stat fails otherwise makes the listing partial ([`Listing::collect`]).
/// `progress` is called after the names are read and after each entry's stat.
pub(crate) fn list(dir: &OwnedFd, progress: &dyn Fn()) -> io::Result<Listing> {
    let names = names(dir)?;
    progress();
    Listing::collect(
        names,
        |n| Ok(stat_name(dir, &n)?.map(|st| entry(n, &st))),
        progress,
    )
}

/// One entry of `dir` by name, with the listing's `statx` flags; `None` when it is gone.
pub(crate) fn stat_child(dir: &OwnedFd, name: &[u8]) -> io::Result<Option<RawEntry>> {
    Ok(stat_name(dir, name)?.map(|st| entry(name.to_vec(), &st)))
}

/// An open directory's mtime.
pub(crate) fn dir_mtime(dir: &OwnedFd) -> io::Result<i64> {
    statx_fd(dir.as_raw_fd()).map(|st| mtime_ns(&st))
}

/// The facts of a directory as a root (Decision 17). Only the [`volume::SAMPLE_MAX`] entries
/// the sample needs are stat'ed (the rest are only counted), so re-probing an offline root
/// with a large top level costs one listing.
pub(crate) fn root_stat(fd: &OwnedFd) -> io::Result<RootStat> {
    let me = statx_fd(fd.as_raw_fd())?;
    let parent = statx(fd.as_raw_fd(), c"..", ENTRY_FLAGS).ok();
    let fields = volume::linux::fields(fd, &me)?;
    let mut names = names(fd)?;
    let count = names.len() as u64;
    names.sort();
    let mut sampled = Vec::new();
    for n in names {
        if sampled.len() == volume::SAMPLE_MAX {
            break;
        }
        if let Some(st) = stat_name(fd, &n)?
            && matches!(file_type(&st), EntryType::File | EntryType::Dir)
        {
            sampled.push(entry(n, &st));
        }
    }
    Ok(RootStat {
        key: fields.key(),
        kind: fields.mount.fstype.clone(),
        dev_id: dev_id(&me),
        parent_dev_id: parent.as_ref().map(dev_id),
        mount_root_attr: attr_known(&me, libc::STATX_ATTR_MOUNT_ROOT),
        case_sensitive: fields.case_sensitive(),
        enumeration: "getdents64".into(),
        remote: fields.remote(),
        root_file_id: Some(ino_id(me.stx_ino)),
        sample: volume::sample(&sampled),
        entries: count,
    })
}
