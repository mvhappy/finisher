//! Cloud-sync placeholders: telling them apart ([`is_placeholder`]) and reading one whose
//! download times out ([`hydrating`]).
//!
//! Reading a placeholder makes the sync client download it. On iCloud Drive (macOS 27) that
//! first read blocks about a second, and once in about 25 reads of a freshly uploaded file it
//! failed with `ETIMEDOUT` instead (spec §3.13 item 16); the next read worked. So a read that
//! times out is tried again, a few times, but only when the file is a placeholder: anywhere
//! else a timeout is a real outage, which the caller decides about (`super::transient`).

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

/// A timed-out placeholder read is retried this many times…
pub(crate) const RETRIES: u32 = 3;
/// …this far apart.
pub(crate) const RETRY_DELAY: Duration = Duration::from_secs(1);

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

/// Whether `e` is a timeout of the kind a placeholder's download gives: `ETIMEDOUT` on
/// Unix, `ERROR_CLOUD_FILE_REQUEST_TIMEOUT` (426) on Windows (documented for Cloud Files;
/// not seen in a test).
fn is_timeout(e: &io::Error) -> bool {
    #[cfg(unix)]
    let code = libc::ETIMEDOUT;
    #[cfg(windows)]
    let code = 426;
    #[cfg(not(any(unix, windows)))]
    let code = i32::MIN;
    e.kind() == io::ErrorKind::TimedOut || e.raw_os_error() == Some(code)
}

/// Runs `read` of `path`; when it times out and `path` is (still) a placeholder, runs it
/// again, up to [`RETRIES`] more times [`RETRY_DELAY`] apart. Any other error, or a timeout
/// of a file that is not a placeholder, is returned at once.
pub(crate) fn hydrating<T>(path: &Path, read: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    hydrating_with(path, RETRIES, RETRY_DELAY, read)
}

fn hydrating_with<T>(
    path: &Path,
    retries: u32,
    delay: Duration,
    mut read: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        let (result, placeholder) = match hooked() {
            Some(placeholder) => (Err(io::Error::from(io::ErrorKind::TimedOut)), placeholder),
            None => {
                let r = read();
                // Only on a timeout: the file's metadata from its directory entry, which
                // does not open it.
                let placeholder = r.as_ref().is_err_and(is_timeout)
                    && fs::symlink_metadata(path).is_ok_and(|m| is_placeholder(path, &m));
                (r, placeholder)
            }
        };
        match result {
            Err(e) if attempt < retries && placeholder && is_timeout(&e) => {
                attempt += 1;
                std::thread::sleep(delay);
            }
            r => return r,
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static TIMEOUTS: std::cell::Cell<(u32, bool)> = const { std::cell::Cell::new((0, false)) };
}

/// Test hook: on this thread, the next `n` reads through [`hydrating`] (every
/// `read_uncached`, project-file and rebuild-seed read) time out without reading, the file
/// counting as a placeholder if `placeholder`.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn time_out_next_reads_for_test(n: u32, placeholder: bool) {
    TIMEOUTS.with(|t| t.set((n, placeholder)));
}

/// `Some(placeholder)` if the hook makes this read time out (counting it down).
fn hooked() -> Option<bool> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        let (n, placeholder) = TIMEOUTS.with(std::cell::Cell::get);
        if n > 0 {
            TIMEOUTS.with(|t| t.set((n - 1, placeholder)));
            return Some(placeholder);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn left() -> u32 {
        TIMEOUTS.with(std::cell::Cell::get).0
    }

    #[test]
    fn a_placeholder_read_that_times_out_is_retried_a_bounded_number_of_times() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("seg");
        fs::write(&path, b"bytes").unwrap();
        // A timeout, then the real read: retried.
        time_out_next_reads_for_test(1, true);
        assert_eq!(super::super::read_uncached(&path).unwrap(), b"bytes");
        assert_eq!(left(), 0);
        // Never more than RETRIES + 1 attempts.
        time_out_next_reads_for_test(10, true);
        let e = hydrating_with(&path, RETRIES, Duration::ZERO, || fs::read(&path)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert_eq!(left(), 10 - (RETRIES + 1));
        time_out_next_reads_for_test(0, false);
    }

    #[test]
    fn a_timeout_of_a_file_that_is_not_a_placeholder_is_returned_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("seg");
        fs::write(&path, b"bytes").unwrap();
        time_out_next_reads_for_test(2, false);
        let e = super::super::read_uncached(&path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert_eq!(left(), 1);
        time_out_next_reads_for_test(0, false);
        // A real timeout of a file that is no placeholder (this OS has none here).
        let mut calls = 0;
        let e = hydrating_with(&path, RETRIES, Duration::ZERO, || {
            calls += 1;
            Err::<(), _>(io::Error::from(io::ErrorKind::TimedOut))
        })
        .unwrap_err();
        assert_eq!((calls, e.kind()), (1, io::ErrorKind::TimedOut));
        // Other errors are never retried here.
        let mut calls = 0;
        let _ = hydrating_with(&path, RETRIES, Duration::ZERO, || {
            calls += 1;
            Err::<(), _>(io::Error::from(io::ErrorKind::NotFound))
        });
        assert_eq!(calls, 1);
    }
}
