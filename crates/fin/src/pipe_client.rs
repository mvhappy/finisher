//! `fin`'s end of finisherd's named pipe on Windows (sub-project 5A Task 11, spec §4.3).
//!
//! - **Access:** `GENERIC_READ | FILE_WRITE_DATA`, what the pipe's descriptor grants an allowed
//!   SID. `GENERIC_WRITE` would also ask for `FILE_APPEND_DATA`, which on a pipe is
//!   `FILE_CREATE_PIPE_INSTANCE`, so a sandboxed `fin` (sub-project 7) could not open it.
//! - **`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`:** whoever holds the pipe may learn
//!   who connected but never act as fin's user. A pipe client otherwise connects at the
//!   `SecurityImpersonation` level (as `interprocess`, which fin used before, does), and a
//!   squatter could impersonate the user before the handshake refuses it.
//! - **Busy:** every instance taken (`ERROR_PIPE_BUSY`) waits in `WaitNamedPipeW` until `wait`
//!   has passed (`TimedOut`). A pipe that does not exist is `NotFound`.
//!
//! The handle is synchronous: `TimedConn` runs one read or write at a time on its own thread.

use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use std::ptr::null;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_WRITE_DATA, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

/// Opens the client end of `path` (`\\.\pipe\…`), waiting at most `wait` for a free instance.
pub fn connect(path: &str, wait: Duration) -> io::Result<File> {
    let name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let until = Instant::now() + wait;
    loop {
        // SAFETY: `name` is NUL-terminated; no security attributes or template.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | FILE_WRITE_DATA,
                0,
                null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            // SAFETY: a new handle that only this `File` owns.
            return Ok(unsafe { File::from_raw_handle(h) });
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
            return Err(e);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let ms = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: as above. A false return (timeout, or the pipe vanished) loops back to
        // `CreateFileW`, which reports which.
        unsafe { WaitNamedPipeW(name.as_ptr(), ms) };
    }
}
