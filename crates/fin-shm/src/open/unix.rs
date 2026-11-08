//! The unix opener: each directory segment by `openat(O_NOFOLLOW | O_DIRECTORY)` from the
//! previous one's descriptor (Linux: `O_PATH`, so a segment needs only search permission), so
//! no segment can be a link and no ancestor can be swapped under the walk; the file is
//! `fstatat(AT_SYMLINK_NOFOLLOW)`-checked before it is opened (a FIFO or device is never
//! opened), then opened `O_NOFOLLOW | O_NONBLOCK` and checked again on its descriptor.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use libc::c_int;

use super::{Mode, Opened, copy_exact, temp_name};
use crate::checks::{Facts, Fail, Kind, Reason, check_file};

/// Where a checked file is: its parent's descriptor (from the walk) and its name.
pub(crate) struct Anchor {
    parent: OwnedFd,
    name: CString,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const DIR_FLAGS: c_int = libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const DIR_FLAGS: c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

fn cname(s: &OsStr) -> Result<CString, Fail> {
    CString::new(s.as_bytes()).map_err(|_| Fail::new(Reason::BadName, "the path has a NUL byte"))
}

fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            r => return r,
        }
    }
}

fn openat(dir: RawFd, name: &CStr, flags: c_int, mode: libc::mode_t) -> io::Result<OwnedFd> {
    retry(|| {
        // SAFETY: a valid C string; `mode` is read only with `O_CREAT`.
        let fd = unsafe { libc::openat(dir, name.as_ptr(), flags, libc::c_uint::from(mode)) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a new descriptor that we own.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    })
}

fn stat_nofollow(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
    // SAFETY: `stat` is plain data, filled by the call.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid descriptor, C string and out pointer.
    let r = unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(st)
    }
}

fn stat_fd(fd: RawFd) -> io::Result<libc::stat> {
    // SAFETY: as above.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid descriptor and out pointer.
    let r = unsafe { libc::fstat(fd, &mut st) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(st)
    }
}

fn facts_of(st: &libc::stat) -> Facts {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => Kind::File,
        libc::S_IFDIR => Kind::Dir,
        libc::S_IFLNK => Kind::Link,
        _ => Kind::Other,
    };
    #[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
    Facts {
        kind,
        nlink: st.st_nlink as u64,
        size: u64::try_from(st.st_size).unwrap_or(0),
        id: (st.st_dev as u64, st.st_ino as u64),
    }
}

fn unlinkat(dir: RawFd, name: &CStr) {
    // SAFETY: valid descriptor and C string. Best effort: the caller is already failing.
    unsafe { libc::unlinkat(dir, name.as_ptr(), 0) };
}

fn renameat(from_dir: RawFd, from: &CStr, to_dir: RawFd, to: &CStr) -> io::Result<()> {
    retry(|| {
        // SAFETY: valid descriptors and C strings.
        let r = unsafe { libc::renameat(from_dir, from.as_ptr(), to_dir, to.as_ptr()) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

/// Why a segment or the file could not be opened: a link (`ELOOP`; `ENOTDIR` for a link
/// opened with `O_DIRECTORY`; `EMLINK` on the BSDs) is `Link`, a non-directory segment
/// `NotRegular`, a missing one `Missing`.
fn open_error(dir: RawFd, name: &CStr, e: io::Error) -> Fail {
    let errno = e.raw_os_error();
    if matches!(
        errno,
        Some(libc::ELOOP) | Some(libc::ENOTDIR) | Some(libc::EMLINK)
    ) {
        let shown = name.to_string_lossy();
        return match stat_nofollow(dir, name).map(|st| facts_of(&st).kind) {
            Ok(Kind::Link) => Fail::new(
                Reason::Link,
                format!("the path segment `{shown}` is a link"),
            ),
            Ok(Kind::Dir) | Err(_) => Fail::io(e),
            Ok(_) => Fail::new(
                Reason::NotRegular,
                format!("the path segment `{shown}` is not a directory"),
            ),
        };
    }
    Fail::io(e)
}

/// Opens every directory of `path` without following a link; answers the parent's
/// descriptor and the file's name. `path` is absolute without `.`/`..` (`check_path`).
fn walk(path: &Path) -> Result<(OwnedFd, CString), Fail> {
    let mut segments: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    let name = segments
        .pop()
        .ok_or_else(|| Fail::new(Reason::NotRegular, "the path is a root"))?;
    let mut cur = openat(libc::AT_FDCWD, c"/", DIR_FLAGS, 0).map_err(Fail::io)?;
    for s in segments {
        let cn = cname(s)?;
        cur = openat(cur.as_raw_fd(), &cn, DIR_FLAGS, 0)
            .map_err(|e| open_error(cur.as_raw_fd(), &cn, e))?;
    }
    Ok((cur, cname(name)?))
}

pub(crate) fn open(path: &Path, mode: Mode) -> Result<Opened, Fail> {
    let (parent, name) = walk(path)?;
    let pfd = parent.as_raw_fd();
    // Before opening: a link, FIFO, socket or device is refused without an `open` (which
    // could block on a FIFO or have a device's side effects).
    let before = facts_of(&stat_nofollow(pfd, &name).map_err(Fail::io)?);
    if before.kind != Kind::File {
        check_file(&before, u64::MAX)?;
    }
    let access = match mode {
        Mode::Read | Mode::Commit => libc::O_RDONLY,
        Mode::ReadWrite => libc::O_RDWR,
    };
    let flags = access | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    let fd = openat(pfd, &name, flags, 0).map_err(|e| open_error(pfd, &name, e))?;
    let facts = facts_of(&stat_fd(fd.as_raw_fd()).map_err(Fail::io)?);
    if facts.kind != Kind::File {
        check_file(&facts, u64::MAX)?;
    }
    if facts.id != before.id {
        return Err(Fail::new(
            Reason::Changed,
            "the file was replaced while it was opened",
        ));
    }
    // SAFETY: plain `fcntl` calls on our own descriptor.
    unsafe {
        let fl = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, fl & !libc::O_NONBLOCK);
        }
    }
    Ok(Opened {
        file: File::from(fd),
        facts,
        anchor: Anchor { parent, name },
    })
}

pub(crate) fn create(path: &Path) -> Result<Opened, Fail> {
    let (parent, name) = walk(path)?;
    let flags = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let fd = openat(parent.as_raw_fd(), &name, flags, 0o600).map_err(Fail::io)?;
    let facts = facts_of(&stat_fd(fd.as_raw_fd()).map_err(Fail::io)?);
    Ok(Opened {
        file: File::from(fd),
        facts,
        anchor: Anchor { parent, name },
    })
}

pub(crate) fn facts(file: &File) -> Result<Facts, Fail> {
    stat_fd(file.as_raw_fd())
        .map(|st| facts_of(&st))
        .map_err(Fail::io)
}

/// The destination's directory, opened following links: it is the host's own (the cache, a
/// project folder), not a job's.
fn open_dest_dir(to: &Path) -> Result<(OwnedFd, CString, &OsStr), Fail> {
    let (Some(dir), Some(name)) = (to.parent(), to.file_name()) else {
        return Err(Fail::new(
            Reason::NotAbsolute,
            "the destination needs a directory and a name",
        ));
    };
    let cdir = cname(dir.as_os_str())?;
    let fd = openat(
        libc::AT_FDCWD,
        &cdir,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        0,
    )
    .map_err(Fail::io)?;
    Ok((fd, cname(name)?, name))
}

fn sync_dir(dir: &OwnedFd) {
    // SAFETY: our own descriptor. Best effort: not every filesystem syncs directories.
    unsafe { libc::fsync(dir.as_raw_fd()) };
}

pub(crate) fn commit(src: &Opened, to: &Path, cap: u64, force_copy: bool) -> Result<(), Fail> {
    let (dir, name, os_name) = open_dest_dir(to)?;
    if !force_copy {
        src.file.sync_all().map_err(Fail::io)?;
        match renameat(
            src.anchor.parent.as_raw_fd(),
            &src.anchor.name,
            dir.as_raw_fd(),
            &name,
        ) {
            Ok(()) => return arrived(src, &dir, &name, cap),
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
            Err(e) => return Err(Fail::io(e)),
        }
    }
    copy_across(src, &dir, &name, os_name)
}

/// After a rename: what is at `name` must be the checked file, still one link and under the
/// cap; else it is removed and the move refused.
fn arrived(src: &Opened, dir: &OwnedFd, name: &CStr, cap: u64) -> Result<(), Fail> {
    let at = stat_nofollow(dir.as_raw_fd(), name).map(|st| facts_of(&st));
    let verdict = match at {
        Ok(f) if f.kind == Kind::File && f.id == src.facts.id => {
            facts(&src.file).and_then(|now| check_file(&now, cap))
        }
        Ok(_) => Err(Fail::new(
            Reason::Changed,
            "the file was replaced between its checks and its move",
        )),
        Err(e) => Err(Fail::io(e)),
    };
    if verdict.is_err() {
        unlinkat(dir.as_raw_fd(), name);
    }
    sync_dir(dir);
    verdict
}

/// Across volumes: a private copy (`O_EXCL` beside the destination), synced and renamed over
/// it; then the source is removed if it is still the checked file.
fn copy_across(src: &Opened, dir: &OwnedFd, name: &CStr, os_name: &OsStr) -> Result<(), Fail> {
    let tmp = cname(&temp_name(os_name))?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let fd = openat(dir.as_raw_fd(), &tmp, flags, 0o600).map_err(Fail::io)?;
    let mut out = File::from(fd);
    let done = copy_exact(&src.file, src.facts.size, &mut out)
        .and_then(|()| renameat(dir.as_raw_fd(), &tmp, dir.as_raw_fd(), name).map_err(Fail::io));
    if let Err(f) = done {
        unlinkat(dir.as_raw_fd(), &tmp);
        return Err(f);
    }
    sync_dir(dir);
    let pfd = src.anchor.parent.as_raw_fd();
    if stat_nofollow(pfd, &src.anchor.name).is_ok_and(|st| facts_of(&st).id == src.facts.id) {
        unlinkat(pfd, &src.anchor.name);
    }
    Ok(())
}
