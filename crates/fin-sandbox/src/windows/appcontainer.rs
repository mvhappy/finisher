//! AppContainer profiles (plan 7C Decision 16): created on first use, reused after, deleted
//! when their project, family or plugin goes away. A normal AppContainer, not LPAC (S9).

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Security::FreeSid;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::PSID;

use super::capability::{MAX_CONTAINER_NAME, check_component};
use super::sid::{Sid, wide};

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`, `DeleteAppContainerProfile` of a missing one.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`.
const ELEMENT_NOT_FOUND: i32 = 0x8007_0490_u32 as i32;

fn check(name: &str) -> io::Result<()> {
    if !name.starts_with("finisher-") || name.len() > MAX_CONTAINER_NAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a Finisher container name"),
        ));
    }
    check_component("container", name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn hresult(call: &str, hr: i32) -> io::Error {
    io::Error::other(format!("{call}: HRESULT {:#010x}", hr as u32))
}

/// Takes a SID the AppContainer API allocated (`FreeSid`) into an owned [`Sid`].
fn take(p: PSID) -> io::Result<Sid> {
    // SAFETY: a valid SID the call returned; copied, then freed once.
    let sid = unsafe { Sid::copy_of(p) };
    // SAFETY: as above.
    unsafe { FreeSid(p) };
    sid
}

/// Holds `Local\finisher-appcontainer-<name>` for one profile creation: Windows' profile API
/// fails (`ERROR_PATH_NOT_FOUND`) when two processes create the same profile at once.
struct Lock(windows_sys::Win32::Foundation::HANDLE);

impl Lock {
    fn take(name: &str) -> io::Result<Lock> {
        use windows_sys::Win32::System::Threading::{CreateMutexW, INFINITE, WaitForSingleObject};
        let n = wide(format!("Local\\finisher-appcontainer-{name}"));
        // SAFETY: a NUL-terminated name, default security, not initially owned.
        let h = unsafe { CreateMutexW(null(), 0, n.as_ptr()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: our mutex handle. An abandoned mutex (its holder died) is ours too.
        unsafe { WaitForSingleObject(h, INFINITE) };
        Ok(Lock(h))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;
        // SAFETY: a mutex this thread owns, released and closed once.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// The container SID of `name`, creating its profile when it does not exist.
pub fn ensure(name: &str) -> io::Result<Sid> {
    check(name)?;
    let _lock = Lock::take(name)?;
    let n = wide(name);
    let desc = wide("Finisher sandbox");
    let mut p: PSID = null_mut();
    // SAFETY: NUL-terminated strings, no capabilities, a valid out-pointer.
    let hr = unsafe {
        CreateAppContainerProfile(n.as_ptr(), n.as_ptr(), desc.as_ptr(), null(), 0, &mut p)
    };
    match hr {
        0 => take(p),
        ALREADY_EXISTS => sid(name),
        hr => Err(hresult("CreateAppContainerProfile", hr)),
    }
}

/// The container SID of `name` (derived; the profile need not exist).
pub fn sid(name: &str) -> io::Result<Sid> {
    check(name)?;
    let mut p: PSID = null_mut();
    // SAFETY: a NUL-terminated string and a valid out-pointer.
    let hr = unsafe { DeriveAppContainerSidFromAppContainerName(wide(name).as_ptr(), &mut p) };
    if hr != 0 {
        return Err(hresult("DeriveAppContainerSidFromAppContainerName", hr));
    }
    take(p)
}

/// Deletes the profile of `name` (its `Packages` folder and registry keys); a missing one is
/// not an error.
pub fn delete(name: &str) -> io::Result<()> {
    check(name)?;
    // SAFETY: a NUL-terminated string.
    match unsafe { DeleteAppContainerProfile(wide(name).as_ptr()) } {
        0 | NOT_FOUND | ELEMENT_NOT_FOUND => Ok(()),
        hr => Err(hresult("DeleteAppContainerProfile", hr)),
    }
}
