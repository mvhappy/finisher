//! The Unix half shared by Linux and macOS (plan 5D Task 2): descriptors, raw-name C strings,
//! no-follow directory opens, and [`NativeSource`], whose listing and volume facts come from
//! the OS module (`source::linux`: `getdents64` + `statx`; `source::macos`:
//! `getattrlistbulk`).
//!
//! Every directory below the root is opened with `openat(parent, raw_name, O_RDONLY |
//! O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` (plus `O_NOATIME` on Linux until one `EPERM`, which
//! means we do not own the directory): no link is followed and no path string is resolved.
//! Nothing here opens a file.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use libc::c_int;

use super::{DirHandle, Listing, RawEntry, RootStat, Source};
use crate::open::hooks;
use crate::raw::RawRel;
use crate::types::VolumeIdentity;

#[cfg(target_os = "linux")]
use super::linux as os;
#[cfg(target_os = "macos")]
use super::macos as os;

/// `O_NOATIME` where the OS has it (Linux), else 0.
#[cfg(target_os = "linux")]
pub(crate) const O_NOATIME: c_int = libc::O_NOATIME;
#[cfg(not(target_os = "linux"))]
pub(crate) const O_NOATIME: c_int = 0;

/// `r`, or the thread's `errno` when `r` is negative.
pub(crate) fn cvt(r: c_int) -> io::Result<c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// Retries `f` while it fails with `EINTR` (NFS `intr` mounts, signals during a slow read).
pub(crate) fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match f() {
            Err(e) if e.raw_os_error() == Some(libc::EINTR) => continue,
            r => return r,
        }
    }
}

pub(crate) fn errno_is(e: &io::Error, code: c_int) -> bool {
    e.raw_os_error() == Some(code)
}

/// A raw component as a C string (a [`RawRel`] is validated, so NUL never reaches here).
pub(crate) fn cname(name: &[u8]) -> io::Result<CString> {
    CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a name holds NUL"))
}

/// `openat`, retried on `EINTR`; under `test-hooks`, a forced `EPERM` for `O_NOATIME`.
pub(crate) fn openat(dir: RawFd, name: &CStr, flags: c_int) -> io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    if flags & O_NOATIME != 0 && hooks::noatime_eperm() {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    retry(|| {
        // SAFETY: `name` is a valid C string; the descriptor returned is owned below.
        let fd = cvt(unsafe { libc::openat(dir, name.as_ptr(), flags, 0) })?;
        // SAFETY: `fd` was just opened and is owned by nobody else.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    })
}

/// Opens the root directory. Its path is the library's canonical root (fin-node, plan Task 8),
/// so it is resolved as given; nothing below it is ever resolved by path.
pub(crate) fn open_root(root: &Path) -> io::Result<OwnedFd> {
    let c = cname(root.as_os_str().as_bytes())?;
    openat(
        libc::AT_FDCWD,
        &c,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
    )
}

/// A fresh descriptor of the same directory with its own offset (a listing never shares a
/// read position with another listing of the same handle).
pub(crate) fn reopen_dir(dir: &OwnedFd) -> io::Result<OwnedFd> {
    openat(
        dir.as_raw_fd(),
        c".",
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
    )
}

/// `fstatat(dir, name, AT_SYMLINK_NOFOLLOW)`; `name` empty is not allowed here.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn lstat_at(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
    retry(|| {
        // SAFETY: `st` is a valid out-pointer; `name` a valid C string.
        cvt(unsafe {
            libc::fstatat(
                dir,
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })
    })?;
    // SAFETY: fstatat succeeded and filled `st`.
    Ok(unsafe { st.assume_init() })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `st` is a valid out-pointer.
    retry(|| cvt(unsafe { libc::fstat(fd, st.as_mut_ptr()) }))?;
    // SAFETY: fstat succeeded and filled `st`.
    Ok(unsafe { st.assume_init() })
}

/// Seconds and nanoseconds as nanoseconds since the Unix epoch (saturating).
pub(crate) fn ns(sec: i64, nsec: i64) -> i64 {
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

/// A file id as bytes: the inode number, big-endian (Linux and macOS alike).
pub(crate) fn ino_id(ino: u64) -> Vec<u8> {
    ino.to_be_bytes().to_vec()
}

/// Opens `name` below `parent` as a directory, following no link. `noatime` is the source's
/// "we may ask for `O_NOATIME`" flag, cleared after one `EPERM` (directories we do not own).
pub(crate) fn open_dir_at(parent: RawFd, name: &[u8], noatime: &AtomicBool) -> io::Result<OwnedFd> {
    let c = cname(name)?;
    let base = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if O_NOATIME != 0 && noatime.load(Ordering::Relaxed) {
        match openat(parent, &c, base | O_NOATIME) {
            Err(e) if errno_is(&e, libc::EPERM) => {
                noatime.store(false, Ordering::Relaxed);
                hooks::count(|c| c.noatime_drops += 1);
            }
            r => return r,
        }
    }
    openat(parent, &c, base)
}

/// The native source of a root on Linux or macOS. The root directory is opened by
/// [`Source::stat_root`] (the scan's first stat) and held for the scan, so every directory is
/// reached from that one handle; [`Source::open_dir`] opens it lazily when no stat came first.
#[derive(Debug)]
pub struct NativeSource {
    root: PathBuf,
    root_fd: Mutex<Option<Arc<OwnedFd>>>,
    noatime: AtomicBool,
}

impl NativeSource {
    /// A source for `root` (an absolute, canonical directory). Nothing is opened yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            root_fd: Mutex::new(None),
            noatime: AtomicBool::new(O_NOATIME != 0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether directory opens still ask for `O_NOATIME` (Linux; cleared after one `EPERM`).
    pub fn uses_noatime(&self) -> bool {
        self.noatime.load(Ordering::Relaxed)
    }

    fn root_fd(&self) -> io::Result<Arc<OwnedFd>> {
        let mut g = self.root_fd.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(fd) = g.as_ref() {
            return Ok(Arc::clone(fd));
        }
        let fd = Arc::new(open_root(&self.root)?);
        *g = Some(Arc::clone(&fd));
        Ok(fd)
    }

    fn fd(dir: &DirHandle) -> io::Result<&Arc<OwnedFd>> {
        dir.os::<Arc<OwnedFd>>().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "not a handle of this source")
        })
    }
}

impl Source for NativeSource {
    fn stat_root(&self) -> io::Result<RootStat> {
        let fd = Arc::new(open_root(&self.root)?);
        let stat = os::root_stat(&fd)?;
        *self.root_fd.lock().unwrap_or_else(|p| p.into_inner()) = Some(fd);
        Ok(stat)
    }

    fn open_dir(&self, rel: &RawRel) -> io::Result<DirHandle> {
        rel.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut cur = self.root_fd()?;
        for c in rel.components() {
            cur = Arc::new(open_dir_at(cur.as_raw_fd(), c, &self.noatime)?);
        }
        Ok(DirHandle::new(rel.clone(), cur))
    }

    fn open_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<DirHandle> {
        let rel = parent.rel().join(name);
        rel.validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let fd = open_dir_at(Self::fd(parent)?.as_raw_fd(), name, &self.noatime)?;
        Ok(DirHandle::new(rel, Arc::new(fd)))
    }

    fn read_dir(&self, dir: &DirHandle) -> io::Result<Vec<RawEntry>> {
        self.read_dir_with(dir, &|| {})?.strict()
    }

    // Linux lists partially (a `Listing` already); macOS's listing is all or nothing (a
    // `Vec`) until it does.
    #[cfg_attr(target_os = "linux", allow(clippy::useless_conversion))]
    fn read_dir_with(&self, dir: &DirHandle, progress: &dyn Fn()) -> io::Result<Listing> {
        os::list(Self::fd(dir)?, progress).map(Listing::from)
    }

    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> io::Result<Option<RawEntry>> {
        os::stat_child(Self::fd(parent)?, name)
    }

    fn dir_mtime(&self, dir: &DirHandle) -> io::Result<Option<i64>> {
        os::dir_mtime(Self::fd(dir)?).map(Some)
    }

    fn volume(&self, dir: &DirHandle) -> io::Result<VolumeIdentity> {
        os::root_stat(Self::fd(dir)?).map(|s| s.identity())
    }
}
