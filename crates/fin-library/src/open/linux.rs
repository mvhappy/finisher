//! The Linux opener (Decision 19; see [`super`] for the three steps).

use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use libc::c_int;

use super::{hooks, map_errno, stub_names};
use crate::source::linux::{ENTRY_FLAGS, file_type, mtime_ns, same_mount, statx, statx_fd};
use crate::source::unix::{O_NOATIME, cname, errno_is, ino_id, open_root, openat};
use crate::types::{EntryType, Expect, OpenError};

const RESOLVE: u64 = libc::RESOLVE_BENEATH
    | libc::RESOLVE_NO_SYMLINKS
    | libc::RESOLVE_NO_MAGICLINKS
    | libc::RESOLVE_NO_XDEV;

/// `openat2` answered `ENOSYS` or `EPERM` once: this process uses the fallback from then on.
static OPENAT2_REFUSED: AtomicBool = AtomicBool::new(false);
/// `O_NOATIME` was refused once (`EPERM`): not asked for again.
static NOATIME_REFUSED: AtomicBool = AtomicBool::new(false);

fn noatime_allowed() -> bool {
    hooks::noatime().unwrap_or_else(|| !NOATIME_REFUSED.load(Ordering::Relaxed))
}

fn drop_noatime() {
    hooks::count(|c| c.noatime_drops += 1);
    if hooks::noatime().is_some() {
        hooks::set_noatime(false);
    } else {
        NOATIME_REFUSED.store(true, Ordering::Relaxed);
    }
}

fn openat2_refused() -> bool {
    hooks::openat2_errno().is_some() || OPENAT2_REFUSED.load(Ordering::Relaxed)
}

/// `ENOSYS` (an old kernel or a sandbox without the syscall) and `EPERM` (seccomp) mean
/// "use the per-component fallback".
fn is_refusal(e: &io::Error) -> bool {
    errno_is(e, libc::ENOSYS) || errno_is(e, libc::EPERM)
}

fn note_refusal() {
    if !hooks::active() {
        OPENAT2_REFUSED.store(true, Ordering::Relaxed);
    }
}

/// `openat2(dir, path, flags, RESOLVE)`, retried on `EINTR`.
fn openat2(dir: RawFd, path: &CStr, flags: c_int) -> io::Result<OwnedFd> {
    if let Some(e) = hooks::openat2_errno() {
        return Err(io::Error::from_raw_os_error(e));
    }
    if flags & O_NOATIME != 0 && hooks::noatime_eperm() {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    // SAFETY: `open_how` is plain data; all-zero is a valid value.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = flags as u64;
    how.resolve = RESOLVE;
    crate::source::unix::retry(|| {
        // SAFETY: valid C string and `open_how` pointer with its size.
        let r = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                dir,
                path.as_ptr(),
                &raw const how,
                std::mem::size_of::<libc::open_how>(),
            )
        };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the syscall returned a new descriptor we own.
        Ok(unsafe { OwnedFd::from_raw_fd(r as c_int) })
    })
}

/// Why a no-follow open of `name` below `dir` failed: a link is `NameSurrogate` (`O_PATH |
/// O_NOFOLLOW | O_DIRECTORY` on a link answers `ENOTDIR`, a final `O_NOFOLLOW` `ELOOP`).
fn component_error(dir: RawFd, name: &CStr, e: io::Error) -> OpenError {
    if (errno_is(&e, libc::ELOOP) || errno_is(&e, libc::ENOTDIR))
        && let Ok(st) = statx(dir, name, ENTRY_FLAGS)
        && file_type(&st) == EntryType::Symlink
    {
        return OpenError::NameSurrogate;
    }
    map_errno(e)
}

/// The fallback walk: each directory component by `openat(O_PATH | O_DIRECTORY |
/// O_NOFOLLOW)`, checked to be a directory on the root's mount.
fn walk_components(
    root: &OwnedFd,
    root_st: &libc::statx,
    dir: &[Vec<u8>],
) -> Result<OwnedFd, OpenError> {
    hooks::count(|c| c.fallbacks += 1);
    let mut cur: Option<OwnedFd> = None;
    for c in dir {
        let base = cur.as_ref().unwrap_or(root).as_raw_fd();
        let cn = cname(c)?;
        let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = openat(base, &cn, flags).map_err(|e| component_error(base, &cn, e))?;
        let st = statx_fd(fd.as_raw_fd())?;
        if file_type(&st) != EntryType::Dir {
            return Err(OpenError::NotFound);
        }
        if !same_mount(&st, root_st) {
            return Err(OpenError::Escapes);
        }
        cur = Some(fd);
    }
    cur.ok_or(OpenError::NotFound)
}

/// The parent directory of the file (`None`: the root itself).
fn open_parent(
    root: &OwnedFd,
    root_st: &libc::statx,
    dir: &[Vec<u8>],
) -> Result<Option<OwnedFd>, OpenError> {
    if dir.is_empty() {
        return Ok(None);
    }
    if !openat2_refused() {
        let path = cname(&dir.join(&b'/'))?;
        let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC;
        match openat2(root.as_raw_fd(), &path, flags) {
            Ok(fd) => return Ok(Some(fd)),
            // `O_PATH` never asks for `O_NOATIME`: an `EPERM` here is the sandbox.
            Err(e) if is_refusal(&e) => note_refusal(),
            // `ELOOP`: a link on the way; `EXDEV`: a mount or an escape.
            Err(e) => return Err(map_errno(e)),
        }
    }
    walk_components(root, root_st, dir).map(Some)
}

/// The fallback open of the file: `openat(O_NOFOLLOW)` from its parent, then a check that it
/// is on the root's mount (a bind-mounted file is a mount of its own).
fn fallback_open(
    parent: RawFd,
    name: &CStr,
    flags: c_int,
    root_st: &libc::statx,
) -> io::Result<OwnedFd> {
    hooks::count(|c| c.fallbacks += 1);
    let fd = openat(parent, name, flags)?;
    let st = statx_fd(fd.as_raw_fd())?;
    if !same_mount(&st, root_st) {
        return Err(io::Error::from_raw_os_error(libc::EXDEV));
    }
    Ok(fd)
}

/// One attempt to open the file with `flags`. An `openat2` `EPERM` is either the sandbox or
/// `O_NOATIME` on a file we do not own: the fallback `openat` tells them apart (it succeeds
/// under a sandbox that refuses only `openat2`, and fails `EPERM` again for `O_NOATIME`).
fn open_last_once(
    parent: RawFd,
    name: &CStr,
    flags: c_int,
    root_st: &libc::statx,
) -> io::Result<OwnedFd> {
    if openat2_refused() {
        return fallback_open(parent, name, flags, root_st);
    }
    match openat2(parent, name, flags) {
        Err(e) if errno_is(&e, libc::ENOSYS) => {
            note_refusal();
            fallback_open(parent, name, flags, root_st)
        }
        Err(e) if errno_is(&e, libc::EPERM) => {
            let r = fallback_open(parent, name, flags, root_st);
            if r.is_ok() {
                note_refusal();
            }
            r
        }
        r => r,
    }
}

/// Opens the file, with `O_NOATIME` when we own it (dropped for good after one `EPERM`).
fn open_last(
    parent: RawFd,
    name: &CStr,
    own: bool,
    root_st: &libc::statx,
) -> Result<OwnedFd, OpenError> {
    let base =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    let mut flags = if own && noatime_allowed() {
        base | O_NOATIME
    } else {
        base
    };
    loop {
        match open_last_once(parent, name, flags, root_st) {
            Err(e) if errno_is(&e, libc::EPERM) && flags & O_NOATIME != 0 => {
                drop_noatime();
                flags = base;
            }
            r => return r.map_err(|e| component_error(parent, name, e)),
        }
    }
}

/// A missing file with a stub listed for it is cloud-only; else it is gone.
fn missing(parent: RawFd, name: &[u8]) -> OpenError {
    for stub in stub_names(name) {
        if let Ok(c) = cname(&stub)
            && statx(parent, &c, ENTRY_FLAGS).is_ok_and(|st| file_type(&st) == EntryType::File)
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
    _hydrate: bool,
) -> Result<File, OpenError> {
    let root_fd = open_root(root).map_err(map_errno)?;
    let root_st = statx_fd(root_fd.as_raw_fd())?;
    let parent = open_parent(&root_fd, &root_st, dir)?;
    let pfd = parent.as_ref().unwrap_or(&root_fd).as_raw_fd();
    let cn = cname(name)?;

    // 1. Re-check without opening. Linux has no in-kernel placeholder: only stubs count.
    let st = match statx(pfd, &cn, ENTRY_FLAGS) {
        Ok(st) => st,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(missing(pfd, name)),
        Err(e) => return Err(map_errno(e)),
    };
    match file_type(&st) {
        EntryType::File => {}
        EntryType::Symlink => return Err(OpenError::NameSurrogate),
        _ => return Err(OpenError::Changed),
    }

    // 2. Open without following links.
    // SAFETY: geteuid has no preconditions.
    let own = st.stx_uid == unsafe { libc::geteuid() };
    let fd = open_last(pfd, &cn, own, &root_st)?;

    // 3. Identity.
    let now = statx_fd(fd.as_raw_fd())?;
    if !same_mount(&now, &root_st) {
        return Err(OpenError::Escapes);
    }
    if file_type(&now) != EntryType::File
        || !expect.matches(now.stx_size, mtime_ns(&now), Some(&ino_id(now.stx_ino)))
    {
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
