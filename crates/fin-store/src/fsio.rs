//! File primitives for the project folder, which may be local, a NAS share or a sync folder
//! (sub-project 4 spec §3.4; parent spec §27.9 for the SMB/NAS findings).
//!
//! - Everything there is write-once ([`write_once`]): a temp file, fsync, a rename that never
//!   replaces, then a directory fsync (a no-op on Windows, where it is ACCESS_DENIED).
//! - The exceptions, `lease.json` and the lease probe file, are rewritten in place
//!   ([`write_in_place`]), never renamed over: after a rename-over, Windows reports the file
//!   missing for seconds.
//! - Lease and verification reads bypass the client cache ([`read_uncached`]): default SMB
//!   client caching held stale content for 14–36 s.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::names;

mod transient;

/// [`write_in_place`] (and the Windows rename) retry a transient failure this many times…
pub const RETRIES: u32 = 60;
/// …this far apart.
pub const RETRY_DELAY: Duration = Duration::from_millis(25);

/// Writes `dir/name` once: `bytes` go to a `.~fin-<ulid>.tmp` in `dir`, which is fsynced and
/// renamed to `name` without ever replacing it, then `dir` is fsynced.
///
/// `AlreadyExists` if `dir/name` exists (its content is untouched). The temp file is removed
/// on failure. An error from the final directory fsync is returned although the file is in
/// place, so callers verify by reading back rather than trusting a retry.
pub fn write_once(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let target = dir.join(name);
    let tmp = dir.join(names::temp_name());
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        sync_file(&file)?;
        drop(file);
        rename_noreplace(&tmp, &target)?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Overwrites the start of the existing file `path` with `bytes` (never truncating) and syncs
/// it, retrying transient errors ([`is_transient`]) [`RETRIES`] times [`RETRY_DELAY`] apart.
/// Each attempt reopens the file, so a stale handle is not reused. Callers write fixed-size
/// records.
///
/// `NotFound` if `path` is missing: a heartbeat must never silently recreate a deleted
/// `lease.json`. [`write_in_place_or_create`] is the creating variant.
pub fn write_in_place(path: &Path, bytes: &[u8]) -> io::Result<()> {
    in_place(path, bytes, false)
}

/// [`write_in_place`], creating `path` if it is missing (for the lease probe file).
pub fn write_in_place_or_create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    in_place(path, bytes, true)
}

fn in_place(path: &Path, bytes: &[u8], create: bool) -> io::Result<()> {
    retry(|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(create)
            .truncate(false)
            .open(path)?;
        file.write_all(bytes)?;
        sync_file(&file)
    })
}

/// Makes `file`'s data and metadata durable. `file` must be open for writing on Windows
/// (`FlushFileBuffers` needs write access).
///
/// On Apple platforms `File::sync_all` is `F_FULLFSYNC` with no fallback, which is `ENOTSUP`
/// on macOS smbfs (parent spec §27.9), so every write to a Mac SMB share would fail. Here
/// `F_FULLFSYNC` (which also flushes the drive's cache on local disks) is tried first and a
/// filesystem that refuses it (`ENOTSUP`, `ENOTTY`, `EINVAL`) gets the plain `fsync` §27.9
/// prescribes. Elsewhere this is `sync_all`.
#[cfg(target_vendor = "apple")]
pub fn sync_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let call = |f: &dyn Fn() -> libc::c_int| loop {
        if f() != -1 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != ErrorKind::Interrupted {
            return Err(e);
        }
    };
    // SAFETY: `file` owns `fd` for the whole call; F_FULLFSYNC takes no argument.
    match call(&|| unsafe { libc::fcntl(fd, libc::F_FULLFSYNC) }) {
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::ENOTSUP | libc::ENOTTY | libc::EINVAL)
            ) =>
        {
            // SAFETY: as above.
            call(&|| unsafe { libc::fsync(fd) })
        }
        result => result,
    }
}

#[cfg(not(target_vendor = "apple"))]
pub fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()
}

pub use transient::is_transient;

fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match f() {
            Err(e) if attempt < RETRIES && is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(RETRY_DELAY);
            }
            result => return result,
        }
    }
}

/// Whether the file is a cloud-sync placeholder whose data has not been downloaded. Pass the
/// metadata from `DirEntry::metadata()`: on Windows that comes from the directory listing,
/// while `fs::metadata` opens the file, which can trigger the download.
///
/// - Windows: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN` or `OFFLINE`.
/// - macOS: the `SF_DATALESS` flag (the legacy `.<name>.icloud` stubs are matched by name in
///   the scan).
/// - Elsewhere: never.
pub fn is_placeholder(_path: &Path, meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        };
        let mask = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_OFFLINE;
        meta.file_attributes() & mask != 0
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        /// `<sys/stat.h>`: the file's data is not local.
        const SF_DATALESS: u32 = 0x4000_0000;
        meta.st_flags() & SF_DATALESS != 0
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = meta;
        false
    }
}

/// Fsyncs a directory so a rename in it is durable. A no-op on Windows (not possible there).
/// Filesystems that cannot sync a directory (`EINVAL`/`ENOTSUP`, allowed by POSIX) are taken
/// as having nothing to sync.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    match sync_file(&File::open(dir)?) {
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) => Ok(()),
        result => result,
    }
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Renames `from` to `to` unless `to` exists (`AlreadyExists`).
///
/// 1. `link` + `unlink`: atomic on local disks and NFS.
/// 2. Where hard links are unsupported (SMB mounts, FAT), the kernel's no-replace rename:
///    `renameat2(RENAME_NOREPLACE)` on Linux, `renamex_np(RENAME_EXCL)` on macOS.
/// 3. Where that is unsupported too, check then rename. Two writers could race between the
///    check and the rename, but only the lease holder writes, and a name carries the epoch,
///    the seq range and the content hash, so two files with one name hold the same records.
#[cfg(unix)]
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let unsupported = |e: &io::Error| {
        e.kind() == ErrorKind::Unsupported
            || e.raw_os_error().is_some_and(|code| {
                // ENOTSUP and EOPNOTSUPP are one value on Linux, two on macOS.
                [
                    libc::EPERM,
                    libc::ENOTSUP,
                    libc::EOPNOTSUPP,
                    libc::ENOSYS,
                    libc::EINVAL,
                ]
                .contains(&code)
            })
    };
    match fs::hard_link(from, to) {
        Ok(()) => {
            // The target is complete; a leftover temp is removed as a crash leftover later.
            let _ = fs::remove_file(from);
            return Ok(());
        }
        Err(e) if !unsupported(&e) => return Err(e),
        Err(_) => {}
    }
    match rename_exclusive(from, to) {
        Err(e) if unsupported(&e) => rename_checked(from, to),
        result => result,
    }
}

/// The last resort of [`rename_noreplace`]: racy between writers, see there.
#[cfg(unix)]
fn rename_checked(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(io::Error::from(ErrorKind::AlreadyExists)),
        Err(e) if e.kind() == ErrorKind::NotFound => fs::rename(from, to),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn c_path(p: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path contains a NUL byte"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call. The raw syscall
    // works whatever the libc version; a kernel or filesystem without it fails with
    // ENOSYS/EINVAL, which the caller treats as unsupported.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both pointers are NUL-terminated strings that outlive the call.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn rename_exclusive(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(ErrorKind::Unsupported.into())
}

/// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` (so an existing target is
/// `ERROR_ALREADY_EXISTS`, i.e. `AlreadyExists`), written through, retried while transient.
#[cfg(windows)]
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let (from, to) = (verbatim_wide(from)?, verbatim_wide(to)?);
    retry(|| {
        // SAFETY: both pointers are NUL-terminated UTF-16 strings that outlive the call.
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

/// `path` as a NUL-terminated `\\?\` (or `\\?\UNC\`) wide string, so paths past `MAX_PATH`
/// work. `absolute` has already normalised separators and `..`.
#[cfg(windows)]
fn verbatim_wide(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let abs = std::path::absolute(path)?;
    let wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let text = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let mut out = if wide.starts_with(&text(r"\\?\")) {
        wide
    } else if let Some(rest) = wide.strip_prefix(text(r"\\").as_slice()) {
        [text(r"\\?\UNC\").as_slice(), rest].concat()
    } else {
        [text(r"\\?\").as_slice(), wide.as_slice()].concat()
    };
    out.push(0);
    Ok(out)
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static REFUSE_DIRECT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FALLBACKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Test hook: the next direct read on this thread is refused as a filesystem without direct
/// I/O would (`EINVAL` on Linux, `ERROR_INVALID_PARAMETER` on Windows).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn refuse_next_direct_read() {
    REFUSE_DIRECT.with(|f| f.set(true));
}

/// Test hook: how many reads on this thread fell back to a buffered read.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn buffered_fallbacks() -> u32 {
    FALLBACKS.with(std::cell::Cell::get)
}

/// `refused` if the [`refuse_next_direct_read`] hook is armed (disarming it), else `result`.
#[cfg(any(target_os = "linux", target_os = "android", windows))]
#[cfg_attr(not(any(test, feature = "test-hooks")), allow(unused_variables))]
fn hooked_direct(result: io::Result<File>, refused: i32) -> io::Result<File> {
    #[cfg(any(test, feature = "test-hooks"))]
    if REFUSE_DIRECT.with(|f| f.replace(false)) {
        return Err(io::Error::from_raw_os_error(refused));
    }
    result
}

/// Counts a buffered fallback for [`buffered_fallbacks`].
#[cfg(any(target_os = "linux", target_os = "android", windows))]
fn note_fallback() {
    #[cfg(any(test, feature = "test-hooks"))]
    FALLBACKS.with(|n| n.set(n.get() + 1));
}

/// Direct I/O alignment (buffer address and read sizes): a multiple of 512- and 4096-byte
/// sectors.
#[cfg(any(target_os = "linux", target_os = "android", windows))]
const ALIGN: usize = 4096;

/// Reads the whole file bypassing the OS cache, so a change made on another host is seen as
/// soon as the server has it.
///
/// - macOS: `F_NOCACHE`.
/// - Linux: `O_DIRECT` with an aligned buffer. Filesystems that refuse it (tmpfs, some network
///   mounts: `EINVAL`) fall back to a buffered read after `POSIX_FADV_DONTNEED`.
/// - Windows: `FILE_FLAG_NO_BUFFERING` with an aligned buffer, opened with read, write and
///   delete sharing so writers are never blocked (parent spec §27.9). `ERROR_INVALID_PARAMETER`
///   falls back to a buffered read.
/// - Elsewhere: a plain read.
#[cfg(target_os = "macos")]
pub fn read_uncached(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::fd::AsRawFd;
    let mut file = File::open(path)?;
    // SAFETY: `file` owns the descriptor for the whole call; F_NOCACHE takes an int.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    file.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn read_uncached(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let refused = |e: &io::Error| matches!(e.raw_os_error(), Some(libc::EINVAL | libc::EOPNOTSUPP));
    let direct = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECT)
        .open(path);
    match hooked_direct(direct, libc::EINVAL).and_then(read_aligned) {
        Err(e) if refused(&e) => read_dropping_cache(path),
        Ok(None) => read_dropping_cache(path),
        Ok(Some(bytes)) => Ok(bytes),
        Err(e) => Err(e),
    }
}

/// A buffered read after asking the kernel to drop the file's cached pages.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_dropping_cache(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::fd::AsRawFd;
    note_fallback();
    let mut file = File::open(path)?;
    // SAFETY: `file` owns the descriptor for the whole call. Advice only: failure is harmless.
    unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    let mut out = Vec::new();
    file.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(windows)]
pub fn read_uncached(path: &Path) -> io::Result<Vec<u8>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_NO_BUFFERING, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let buffered = || {
        note_fallback();
        let mut out = Vec::new();
        OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(path)?
            .read_to_end(&mut out)?;
        Ok(out)
    };
    let direct = OpenOptions::new()
        .read(true)
        .share_mode(share)
        .custom_flags(FILE_FLAG_NO_BUFFERING)
        .open(path);
    match hooked_direct(direct, ERROR_INVALID_PARAMETER as i32).and_then(read_aligned) {
        Err(e) if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => buffered(),
        Ok(None) => buffered(),
        Ok(Some(bytes)) => Ok(bytes),
        Err(e) => Err(e),
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    windows
)))]
pub fn read_uncached(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

/// Reads `file` (opened for direct I/O) to the end in [`ALIGN`]ed chunks into an aligned
/// buffer. `None` if the read ended short of the file's size: direct reads can only continue
/// from an aligned offset, so after an unaligned short read the caller rereads buffered.
#[cfg(any(target_os = "linux", target_os = "android", windows))]
fn read_aligned(mut file: File) -> io::Result<Option<Vec<u8>>> {
    const CHUNK_MAX: usize = 4 << 20;
    let size = file.metadata()?.len();
    let hint = usize::try_from(size).unwrap_or(usize::MAX);
    let chunk = hint
        .saturating_add(1)
        .next_multiple_of(ALIGN)
        .clamp(ALIGN, CHUNK_MAX);
    // Over-allocate and start at the first aligned address: safe alignment without a custom
    // allocator.
    let mut raw = vec![0u8; chunk + ALIGN];
    let start = raw.as_ptr().align_offset(ALIGN);
    let buf = &mut raw[start..start + chunk];
    let mut out = Vec::with_capacity(hint.min(CHUNK_MAX));
    loop {
        let n = match file.read(buf) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.extend_from_slice(&buf[..n]);
        if n == 0 || n % ALIGN != 0 {
            break;
        }
    }
    Ok((out.len() as u64 >= size).then_some(out))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The fallbacks `rename_noreplace` uses where `link` is unsupported.
    #[test]
    fn rename_fallbacks_never_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("c"),
        );
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let exclusive: [fn(&Path, &Path) -> io::Result<()>; 2] = [rename_exclusive, rename_checked];
        for (i, rename) in exclusive.into_iter().enumerate() {
            match rename(&a, &b) {
                Err(e) if i == 0 && e.kind() == ErrorKind::Unsupported => continue,
                Err(e) => assert_eq!(e.kind(), ErrorKind::AlreadyExists, "{i}: {e}"),
                Ok(()) => panic!("{i} replaced the target"),
            }
            assert_eq!(fs::read(&b).unwrap(), b"b");
            rename(&a, &c).unwrap();
            assert_eq!(fs::read(&c).unwrap(), b"a");
            assert!(!a.exists());
            fs::rename(&c, &a).unwrap();
        }
    }

    #[test]
    fn retry_gives_up_after_the_limit() {
        let mut calls = 0;
        let started = std::time::Instant::now();
        let err = retry::<()>(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(libc::EBUSY))
        })
        .unwrap_err();
        assert_eq!(
            (calls, err.raw_os_error()),
            (RETRIES + 1, Some(libc::EBUSY))
        );
        assert!(started.elapsed() >= RETRY_DELAY * RETRIES);
        let mut calls = 0;
        let err = retry::<()>(|| {
            calls += 1;
            Err(io::Error::from_raw_os_error(libc::EACCES))
        })
        .unwrap_err();
        assert_eq!((calls, err.kind()), (1, ErrorKind::PermissionDenied));
    }
}
