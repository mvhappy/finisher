//! The macOS opener (Decision 19; see [`super`] for the three steps).

use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;

use libc::c_void;

use super::{map_errno, stub_names};
use crate::placeholder::SF_DATALESS;
use crate::source::macos::{VLNK, VREG};
use crate::source::unix::{
    cname, cvt, errno_is, fstat, ino_id, lstat_at, ns, open_root, openat, retry,
};
use crate::types::{Expect, OpenError};

const DIR_FLAGS: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

fn same_dev(fd: RawFd, root_dev: libc::dev_t) -> Result<(), OpenError> {
    if fstat(fd)?.st_dev == root_dev {
        Ok(())
    } else {
        Err(OpenError::Escapes)
    }
}

/// The per-component walk, for a system without `O_NOFOLLOW_ANY` (`EINVAL`).
fn walk_components(
    root: &OwnedFd,
    root_dev: libc::dev_t,
    dir: &[Vec<u8>],
) -> Result<OwnedFd, OpenError> {
    let mut cur: Option<OwnedFd> = None;
    for c in dir {
        let base = cur.as_ref().unwrap_or(root).as_raw_fd();
        let cn = cname(c)?;
        let fd = openat(base, &cn, DIR_FLAGS | libc::O_NOFOLLOW).map_err(|e| {
            if lstat_at(base, &cn).is_ok_and(|st| st.st_mode & libc::S_IFMT == libc::S_IFLNK) {
                OpenError::NameSurrogate
            } else {
                map_errno(e)
            }
        })?;
        same_dev(fd.as_raw_fd(), root_dev)?;
        cur = Some(fd);
    }
    cur.ok_or(OpenError::NotFound)
}

fn open_parent(
    root: &OwnedFd,
    root_dev: libc::dev_t,
    dir: &[Vec<u8>],
) -> Result<Option<OwnedFd>, OpenError> {
    if dir.is_empty() {
        return Ok(None);
    }
    let path = cname(&dir.join(&b'/'))?;
    match openat(root.as_raw_fd(), &path, DIR_FLAGS | libc::O_NOFOLLOW_ANY) {
        Ok(fd) => {
            same_dev(fd.as_raw_fd(), root_dev)?;
            Ok(Some(fd))
        }
        Err(e) if errno_is(&e, libc::EINVAL) => walk_components(root, root_dev, dir).map(Some),
        Err(e) => Err(map_errno(e)),
    }
}

/// `getattrlistat(parent, name, OBJTYPE | FLAGS, FSOPT_NOFOLLOW)`: the object type and
/// `st_flags`, without opening the file.
fn recheck(parent: RawFd, name: &std::ffi::CStr) -> io::Result<(u32, u32)> {
    // SAFETY: plain data, all-zero valid.
    let mut al: libc::attrlist = unsafe { MaybeUninit::zeroed().assume_init() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr = libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_FLAGS;
    let mut buf = [0u32; 3];
    retry(|| {
        // SAFETY: `al` and `buf` are valid for the sizes passed.
        cvt(unsafe {
            libc::getattrlistat(
                parent,
                name.as_ptr(),
                (&raw mut al).cast::<c_void>(),
                buf.as_mut_ptr().cast::<c_void>(),
                std::mem::size_of_val(&buf),
                libc::FSOPT_NOFOLLOW as libc::c_ulong,
            )
        })
    })?;
    Ok((buf[1], buf[2]))
}

/// A missing file with a stub listed for it is cloud-only; else it is gone.
fn missing(parent: RawFd, name: &[u8]) -> OpenError {
    for stub in stub_names(name) {
        if let Ok(c) = cname(&stub)
            && lstat_at(parent, &c).is_ok_and(|st| st.st_mode & libc::S_IFMT == libc::S_IFREG)
        {
            return OpenError::CloudOnly;
        }
    }
    OpenError::NotFound
}

pub(super) fn open(
    root: &Path,
    dir: &[Vec<u8>],
    name: &[u8],
    expect: &Expect,
    hydrate: bool,
) -> Result<File, OpenError> {
    let root_fd = open_root(root).map_err(map_errno)?;
    let root_dev = fstat(root_fd.as_raw_fd())?.st_dev;
    let parent = open_parent(&root_fd, root_dev, dir)?;
    let pfd = parent.as_ref().unwrap_or(&root_fd).as_raw_fd();
    let cn = cname(name)?;

    // 1. Re-check without opening.
    let (objtype, flags) = match recheck(pfd, &cn) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(missing(pfd, name)),
        Err(e) => return Err(map_errno(e)),
    };
    match objtype {
        VREG => {}
        VLNK => return Err(OpenError::NameSurrogate),
        _ => return Err(OpenError::Changed),
    }
    if flags & SF_DATALESS != 0 && !hydrate {
        return Err(OpenError::CloudOnly);
    }

    // 2. Open without following links.
    let oflags =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    let fd = openat(pfd, &cn, oflags).map_err(map_errno)?;

    // 3. Identity (and the flags again: the residual window).
    let st = fstat(fd.as_raw_fd())?;
    if st.st_dev != root_dev {
        return Err(OpenError::Escapes);
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(OpenError::Changed);
    }
    if st.st_flags & SF_DATALESS != 0 && !hydrate {
        return Err(OpenError::CloudOnly);
    }
    let mtime = ns(st.st_mtime, st.st_mtime_nsec);
    if !expect.matches(st.st_size.max(0) as u64, mtime, Some(&ino_id(st.st_ino))) {
        return Err(OpenError::Changed);
    }
    // SAFETY: plain fcntl calls on our own descriptor.
    unsafe {
        let fl = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, fl & !libc::O_NONBLOCK);
        }
    }
    Ok(File::from(fd))
}
