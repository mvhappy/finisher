//! Opening and moving job files under the rules of spec §3.4: no link at any segment, a
//! regular file with one link, a size cap; bounded positioned reads that turn a short read
//! into `truncated`.

use std::fs::File;
use std::path::{Component, Path};

use fin_core::CoreError;
use serde_json::json;

use crate::SHM_REFUSED;
use crate::checks::{Facts, Fail, Reason, check_file};
use crate::hooks;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

/// How a file is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Read-only, sharing writes (a reader of a ring another party is writing).
    Read,
    /// Read and write (a writer opening the ring 5C created for it).
    ReadWrite,
    /// Read-only, for the move out of `out/` (Windows: with `DELETE`, refusing a file another
    /// handle can still write).
    Commit,
}

/// An opened, checked file and what the OS needs to move it.
pub(crate) struct Opened {
    pub file: File,
    pub facts: Facts,
    #[cfg_attr(windows, allow(dead_code))]
    pub anchor: os::Anchor,
}

/// The checks every open performs (spec §3.4): no segment of `path` is a link (unix
/// `O_NOFOLLOW` per segment; Windows: no reparse point at any segment), the file is a regular
/// file with `nlink == 1` and at most `cap` bytes. `path` must be absolute without `.`/`..`.
/// A ring's size is checked against its header by [`crate::RingReader`]. Never maps.
///
/// Refusals are `SHM_REFUSED {file, reason}` ([`Reason`]).
pub fn checked_open(path: &Path, cap: u64) -> Result<File, CoreError> {
    open_checked(path, cap, Mode::Read).map(|o| o.file)
}

pub(crate) fn open_checked(path: &Path, cap: u64, mode: Mode) -> Result<Opened, CoreError> {
    check_path(path).map_err(|f| to_error(path, f))?;
    let opened = os::open(path, mode).map_err(|f| to_error(path, f))?;
    check_file(&opened.facts, cap).map_err(|f| to_error(path, f))?;
    Ok(opened)
}

/// Creates `path` (it must not exist; no segment may be a link), read-write, mode 0600.
pub(crate) fn create_new(path: &Path) -> Result<Opened, CoreError> {
    check_path(path).map_err(|f| to_error(path, f))?;
    os::create(path).map_err(|f| to_error(path, f))
}

/// Creates the output file `path` for a worker to fill: no segment may be a link (so a linked
/// `out/` is refused), the file itself is made `O_EXCL` and `O_NOFOLLOW`, read-write, mode 0600.
/// An entry of that name already there (a retried attempt's earlier output, or a link or file a
/// worker or a previous run left) is unlinked first, never followed, and the file made new.
pub fn create_output(path: &Path) -> Result<File, CoreError> {
    check_path(path).map_err(|f| to_error(path, f))?;
    let made = match os::create(path) {
        Err(f) if f.reason == Reason::Exists => {
            os::remove(path).map_err(|f| to_error(path, f))?;
            os::create(path)
        }
        other => other,
    };
    made.map(|o| o.file).map_err(|f| to_error(path, f))
}

/// [`create_output`] that never replaces: an entry of that name already there is
/// `SHM_REFUSED {reason: exists}`.
pub fn create_new_output(path: &Path) -> Result<File, CoreError> {
    check_path(path).map_err(|f| to_error(path, f))?;
    create_new(path).map(|o| o.file)
}

/// Writes `bytes` as `dir/name` for a worker to read: to `dir/<name>.tmp` made new (an earlier
/// attempt's leftover is unlinked first), synced, then renamed over `dir/name`. No segment of
/// `dir` may be a link, and the rename happens on the descriptor of the directory the
/// temporary file was made in, so a link swapped in for `dir` between the two steps cannot
/// redirect either (plan 6A Task 11-12 review L2). A link planted at `dir/name` is replaced,
/// not followed.
pub fn write_input(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), CoreError> {
    use std::io::Write;
    let tmp = dir.join(format!("{name}.tmp"));
    let mut file = create_output(&tmp)?;
    let done = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| io_refusal(&tmp, &e));
    drop(file);
    done?;
    os::rename_within(&tmp, std::ffi::OsStr::new(name)).map_err(|f| to_error(&tmp, f))
}

/// The handle's current facts (`fstat`).
pub(crate) fn facts(file: &File) -> Result<Facts, Fail> {
    os::facts(file)
}

/// The move of a finished output from `out/` to its destination (spec §3.3, §3.4):
/// [`checked_open`] of `from` (with `cap`), then a rename from the checked directory to `to`
/// (replacing it), or, across volumes, a bounded copy into a temporary file beside `to` that
/// is synced and renamed over it, after which `from` is removed. After a rename the file at
/// `to` must be the one that was checked, with one link and at most `cap` bytes; otherwise it
/// is removed and the move is refused (`changed`, `hardLinked`, `overCap`).
///
/// `to`'s directory is the host's own and must exist. **A rename gives the destination the
/// writer's own inode**, so call this only once the writer has exited (5C's `onSucceeded` of a
/// cold job): on Windows a file another handle can still write is refused (`busy`); unix has
/// no such check, so a live writer could still change the moved file. For a writer that may
/// still be alive (a warm worker) use [`commit_output_copy`].
pub fn commit_output(from: &Path, to: &Path, cap: u64) -> Result<(), CoreError> {
    commit(from, to, cap, false)
}

/// [`commit_output`] that never renames: the bytes of the checked handle are copied (bounded
/// by its size, which must not change meanwhile: a file that grew is `changed`, one that shrank
/// `truncated`) into a temporary file the host creates beside `to` (`O_EXCL`, no links, 0600),
/// synced and renamed over `to`; the source is then unlinked. The destination is a new inode no
/// other process holds, so a writer that outlives the call (a warm worker, spec §3.2) cannot
/// change it afterwards.
pub fn commit_output_copy(from: &Path, to: &Path, cap: u64) -> Result<(), CoreError> {
    commit(from, to, cap, true)
}

fn commit(from: &Path, to: &Path, cap: u64, copy: bool) -> Result<(), CoreError> {
    // A copy reads a file a writer may hold open (Windows: `Commit` refuses that as `busy`).
    let mode = if copy { Mode::Read } else { Mode::Commit };
    let src = open_checked(from, cap, mode)?;
    let (Some(_), Some(_)) = (to.parent(), to.file_name()) else {
        return Err(refusal(
            to,
            Reason::NotAbsolute,
            "the destination needs a directory and a name",
        ));
    };
    if !to.is_absolute() {
        return Err(refusal(
            to,
            Reason::NotAbsolute,
            "the destination must be an absolute path",
        ));
    }
    hooks::run_before_rename();
    os::commit(&src, to, cap, copy || hooks::copy_forced()).map_err(|f| to_error(from, f))
}

/// A path the walk accepts: absolute, no `.` or `..` segment, at least one normal segment.
fn check_path(path: &Path) -> Result<(), Fail> {
    if !path.is_absolute() {
        return Err(Fail::new(Reason::NotAbsolute, "the path must be absolute"));
    }
    let mut normal = false;
    for c in path.components() {
        match c {
            Component::CurDir | Component::ParentDir => {
                return Err(Fail::new(
                    Reason::NotAbsolute,
                    "the path has a `.` or `..` segment",
                ));
            }
            Component::Normal(_) => normal = true,
            Component::Prefix(_) | Component::RootDir => {}
        }
    }
    if normal {
        Ok(())
    } else {
        Err(Fail::new(Reason::NotRegular, "the path is a root"))
    }
}

/// Reads exactly `buf.len()` bytes at `offset`; a short read (the file was truncated) is a
/// `Truncated` failure, never a fault.
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<(), Fail> {
    let mut done = 0usize;
    while done < buf.len() {
        let at = offset + done as u64;
        match read_at(file, &mut buf[done..], at) {
            Ok(0) => {
                return Err(Fail::new(
                    Reason::Truncated,
                    format!(
                        "the file ended at {at} bytes while {} were read at {offset}",
                        buf.len()
                    ),
                ));
            }
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Fail::io(e)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

/// Copies exactly `size` bytes of `src` into `dst` and syncs it; `src` must end there (a file
/// that grew is `Changed`, one that shrank `Truncated`).
pub(crate) fn copy_exact(src: &File, size: u64, dst: &mut File) -> Result<(), Fail> {
    use std::io::Write;
    const CHUNK: u64 = 1 << 20;
    let mut buf = vec![0u8; CHUNK.min(size.max(1)) as usize];
    let mut at = 0u64;
    while at < size {
        let n = (size - at).min(CHUNK) as usize;
        read_exact_at(src, &mut buf[..n], at)?;
        dst.write_all(&buf[..n]).map_err(Fail::io)?;
        at += n as u64;
    }
    let mut one = [0u8; 1];
    loop {
        match read_at(src, &mut one, size) {
            Ok(0) => break,
            Ok(_) => {
                return Err(Fail::new(
                    Reason::Changed,
                    "the file grew while it was copied",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Fail::io(e)),
        }
    }
    dst.sync_all().map_err(Fail::io)
}

/// A temporary name beside a destination: `.<name>.fin-shm-<pid>-<n>.tmp`.
pub(crate) fn temp_name(name: &std::ffi::OsStr) -> std::ffi::OsString {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut s = std::ffi::OsString::from(".");
    s.push(name);
    s.push(format!(
        ".fin-shm-{}-{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    s
}

/// `SHM_REFUSED {file, reason}` for `path`.
pub(crate) fn refusal(path: &Path, reason: Reason, message: &str) -> CoreError {
    to_error(path, Fail::new(reason, message))
}

pub(crate) fn io_refusal(path: &Path, e: &std::io::Error) -> CoreError {
    to_error(path, Fail::io(std::io::Error::new(e.kind(), e.to_string())))
}

pub(crate) fn to_error(path: &Path, f: Fail) -> CoreError {
    let file = path.display().to_string();
    let mut details = json!({ "file": file, "reason": f.reason.as_str() });
    if let Some(e) = &f.cause {
        details["cause"] = json!(
            e.raw_os_error()
                .map_or_else(|| format!("{:?}", e.kind()), |c| c.to_string())
        );
    }
    let fix = match f.reason {
        Reason::Link | Reason::HardLinked | Reason::NotRegular => Some(
            "a job's files must be regular files the job created, never links or special files",
        ),
        Reason::OverCap | Reason::SizeMismatch | Reason::Header | Reason::Protocol => {
            Some("the writer broke the ring rules; re-run the job, and report it if it repeats")
        }
        _ => None,
    };
    let e = CoreError::new(SHM_REFUSED, format!("{file}: {}", f.message)).with_details(details);
    match fix {
        Some(fix) => e.with_fix(fix),
        None => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_dotted_paths_are_refused() {
        assert_eq!(
            check_path(Path::new("a/b")).unwrap_err().reason,
            Reason::NotAbsolute
        );
        #[cfg(unix)]
        {
            assert_eq!(
                check_path(Path::new("/a/../b")).unwrap_err().reason,
                Reason::NotAbsolute
            );
            assert_eq!(
                check_path(Path::new("/")).unwrap_err().reason,
                Reason::NotRegular
            );
            assert!(check_path(Path::new("/a/b")).is_ok());
        }
        #[cfg(windows)]
        {
            for p in [r"C:\a\..\b", r"\\?\C:\a\..\b", r"C:a\b", r"\a\b"] {
                assert_eq!(
                    check_path(Path::new(p)).unwrap_err().reason,
                    Reason::NotAbsolute,
                    "{p}"
                );
            }
            assert_eq!(
                check_path(Path::new(r"C:\")).unwrap_err().reason,
                Reason::NotRegular
            );
            assert!(check_path(Path::new(r"\\?\C:\a\b")).is_ok());
            assert!(check_path(Path::new(r"\\server\share\a")).is_ok());
        }
    }

    #[test]
    fn a_refusal_names_the_file_and_the_reason() {
        let e = refusal(Path::new("/j/out/x"), Reason::Link, "the file is a link");
        assert_eq!(e.code, SHM_REFUSED);
        assert_eq!(e.details["file"], "/j/out/x");
        assert_eq!(e.details["reason"], "link");
        assert!(e.fix.is_some());
    }
}
