//! Opening a file to probe. An open never blocks: a FIFO, socket, device or folder is refused
//! from its open handle before any read (a FIFO's open would otherwise wait for a writer and
//! hold the caller's thread). [`open_beneath`] also refuses a file not reached through
//! folders beneath a root, or reached through a symlink, decided on the handle, not the path.
//!
//! - Linux: `openat2(root, rel, RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`; where the kernel lacks
//!   it (before 5.6, or a seccomp filter that refuses it), the walk below.
//! - Other Unix (macOS too): a walk from the root's descriptor, one `openat(O_NOFOLLOW)` per
//!   component (`O_DIRECTORY` for folders), so no component may be a symlink.
//! - Windows: the file opened as itself (`FILE_FLAG_OPEN_REPARSE_POINT`); a reparse point is
//!   refused, and the handle's final path must be inside the root's. Windows has no FIFOs in
//!   folders; a non-file handle is still refused before any read.

use std::fs::{File, Metadata};
use std::io;
use std::path::{Component, Path};

use fin_core::CoreError;
use serde_json::json;

use crate::{absolute, io_error};

/// Why an open was refused.
#[derive(Debug)]
pub(crate) enum Refused {
    Io(io::Error),
    /// A folder, FIFO, socket or device.
    NotAFile,
    /// Not beneath the root, or reached through a symlink (`reason`).
    Outside(&'static str),
}

impl From<io::Error> for Refused {
    fn from(e: io::Error) -> Refused {
        Refused::Io(e)
    }
}

impl From<Refused> for io::Error {
    fn from(r: Refused) -> io::Error {
        match r {
            Refused::Io(e) => e,
            Refused::NotAFile => io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
            Refused::Outside(reason) => io::Error::new(io::ErrorKind::PermissionDenied, reason),
        }
    }
}

impl Refused {
    /// The `CoreError` for `path` (`root`: the root of an [`open_beneath`]).
    pub(crate) fn into_core(self, path: &Path, root: Option<&Path>) -> CoreError {
        match self {
            Refused::Io(e) => io_error(path, &e),
            Refused::NotAFile => {
                CoreError::new("ASSET_UNREADABLE", "the media path is not a regular file")
                    .with_details(
                        json!({ "path": absolute(path).to_string_lossy(), "reason": "NotAFile" }),
                    )
                    .with_fix("use a regular file, not a folder, FIFO, socket or device")
            }
            // The path as given, never canonicalised: its symlink's target is not reported.
            Refused::Outside(reason) => CoreError::new(
                "PATH_NOT_ALLOWED",
                "the media path is not a file inside the allowed folder",
            )
            .with_details(json!({
                "path": path.to_string_lossy(),
                "root": root.map(|r| r.to_string_lossy().into_owned()),
                "reason": reason,
            }))
            .with_fix("use a file inside the folder, reached without symlinks"),
        }
    }
}

/// `file` and its metadata when it is a regular file, now in blocking mode.
fn regular(file: File) -> Result<(File, Metadata), Refused> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Refused::NotAFile);
    }
    sys::blocking(&file)?;
    Ok((file, meta))
}

/// `path` (symlinks followed), when it is a regular file.
pub(crate) fn open(path: &Path) -> Result<(File, Metadata), Refused> {
    regular(sys::open(path)?)
}

/// `path` when it is a regular file inside `root`, reached from it through folders only, with
/// no symlink in any component below the root. `root` itself is opened as given (a caller
/// passes it canonical); `path` must be `root` joined with plain components (no `..`).
pub(crate) fn open_beneath(root: &Path, path: &Path) -> Result<(File, Metadata), Refused> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| Refused::Outside("outsideRoot"))?;
    if rel.as_os_str().is_empty() {
        return Err(Refused::NotAFile);
    }
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(Refused::Outside("outsideRoot"));
    }
    regular(sys::open_beneath(root, rel)?)
}

#[cfg(unix)]
mod sys {
    use std::ffi::CString;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use super::Refused;

    /// Never blocks on a FIFO; never makes a terminal the controlling one.
    const FLAGS: libc::c_int = libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;

    pub(super) fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)
    }

    /// Clears `O_NONBLOCK` (set by every open here) before the file is read.
    pub(super) fn blocking(file: &File) -> io::Result<()> {
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is open for the duration of the calls; F_GETFL/F_SETFL take ints.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn c_path(p: &std::ffi::OsStr) -> io::Result<CString> {
        CString::new(p.as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    /// A symlink met where none may be: `ELOOP` (`O_NOFOLLOW`, `RESOLVE_NO_SYMLINKS`),
    /// `EXDEV` (`RESOLVE_BENEATH`), `EMLINK` (`O_NOFOLLOW` on FreeBSD).
    pub(super) fn refused(e: io::Error) -> Refused {
        match e.raw_os_error() {
            Some(libc::ELOOP | libc::EXDEV | libc::EMLINK) => Refused::Outside("symlink"),
            _ => Refused::Io(e),
        }
    }

    pub(super) fn open_beneath(root: &Path, rel: &Path) -> Result<File, Refused> {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NONBLOCK)
            .open(root)?;
        #[cfg(target_os = "linux")]
        match openat2(&dir, rel) {
            // No openat2 here: the walk decides the same.
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) => {}
            other => return other.map_err(refused),
        }
        walk(&dir, rel).map_err(refused)
    }

    #[cfg(target_os = "linux")]
    fn openat2(dir: &File, rel: &Path) -> io::Result<File> {
        let c = c_path(rel.as_os_str())?;
        // SAFETY: `open_how` is plain integers (non-exhaustive, so built zeroed).
        let mut how: libc::open_how = unsafe { std::mem::zeroed() };
        how.flags = FLAGS as u64;
        how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
        // SAFETY: valid fd, NUL-terminated path, and `how` with its size, as openat2(2) takes.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                dir.as_raw_fd(),
                c.as_ptr(),
                &how as *const libc::open_how,
                std::mem::size_of::<libc::open_how>(),
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat2 returned a new descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(fd as libc::c_int) })
    }

    /// One `openat(O_NOFOLLOW)` per component of `rel` from `dir`. A folder is checked on its
    /// handle rather than with `O_DIRECTORY`, which Linux answers `ENOTDIR` for a symlink.
    pub(super) fn walk(dir: &File, rel: &Path) -> io::Result<File> {
        let parts: Vec<_> = rel.iter().collect();
        let mut held: Option<File> = None;
        for (i, part) in parts.iter().enumerate() {
            let at = held.as_ref().unwrap_or(dir).as_raw_fd();
            let c = c_path(part)?;
            // SAFETY: valid fd and NUL-terminated name.
            let fd = unsafe { libc::openat(at, c.as_ptr(), FLAGS | libc::O_NOFOLLOW) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a new descriptor that nothing else owns.
            let file = unsafe { File::from_raw_fd(fd) };
            if i + 1 < parts.len() && !file.metadata()?.is_dir() {
                return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
            }
            held = Some(file);
        }
        held.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    use super::Refused;

    /// A folder opens too (`FILE_FLAG_BACKUP_SEMANTICS`), to be refused as not a file.
    pub(super) fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    pub(super) fn blocking(_file: &File) -> io::Result<()> {
        Ok(())
    }

    /// The handle's path after every link, `\\?\` form.
    fn final_path(file: &File) -> io::Result<PathBuf> {
        let mut buf = vec![0u16; 512];
        loop {
            let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            // SAFETY: a live handle and a buffer of `len` UTF-16 units.
            let n = unsafe {
                GetFinalPathNameByHandleW(
                    file.as_raw_handle(),
                    buf.as_mut_ptr(),
                    len,
                    FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                )
            } as usize;
            if n == 0 {
                return Err(io::Error::last_os_error());
            }
            if n < buf.len() {
                return Ok(PathBuf::from(OsString::from_wide(&buf[..n])));
            }
            buf.resize(n + 1, 0);
        }
    }

    pub(super) fn open_beneath(root: &Path, rel: &Path) -> Result<File, Refused> {
        let flags = FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS;
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(root.join(rel))?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Refused::Outside("symlink"));
        }
        // A junction or symlink above the file was followed: where it led must be inside.
        if !final_path(&file)?.starts_with(final_path(&dir)?) {
            return Err(Refused::Outside("outsideRoot"));
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_walk_refuses_symlinks_at_every_level() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.txt"), b"a").unwrap();
        std::fs::write(t.path().join("secret"), b"s").unwrap();
        symlink(t.path().join("secret"), root.join("sub/link")).unwrap();
        symlink(root.join("sub"), root.join("dirlink")).unwrap();
        symlink("/nonexistent", root.join("dangling")).unwrap();
        let dir = File::open(&root).unwrap();
        let walk = |rel: &str| sys::walk(&dir, Path::new(rel)).map_err(sys::refused);
        assert!(walk("sub/a.txt").is_ok());
        for rel in ["sub/link", "dirlink/a.txt", "dangling"] {
            assert!(
                matches!(walk(rel), Err(Refused::Outside("symlink"))),
                "{rel}: {:?}",
                walk(rel)
            );
        }
        // A FIFO opens at once (to be refused as not a file); a file is not a folder.
        let fifo = std::ffi::CString::new(root.join("fifo").into_os_string().into_vec()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let opened = walk("fifo").unwrap();
        assert!(matches!(regular(opened), Err(Refused::NotAFile)));
        let through = walk("sub/a.txt/b").unwrap_err();
        assert!(matches!(through, Refused::Io(e) if e.raw_os_error() == Some(libc::ENOTDIR)));
    }

    #[test]
    fn a_path_outside_or_with_dot_dot_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(t.path().join("x"), b"x").unwrap();
        for p in [t.path().join("x"), root.join("..").join("x")] {
            assert!(
                matches!(open_beneath(&root, &p), Err(Refused::Outside(_))),
                "{p:?}"
            );
        }
        assert!(matches!(open_beneath(&root, &root), Err(Refused::NotAFile)));
    }
}
