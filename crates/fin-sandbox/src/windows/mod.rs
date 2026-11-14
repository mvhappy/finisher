//! Windows: AppContainer, job objects, the private desktop and fault reporting without UI
//! (plan 7C Task 9; spec §8.4; S9 Windows results).
//!
//! - [`capability`] and [`plan`] (pure, every OS): the names and SIDs of containers and
//!   capabilities, and what each profile's process gets: one container per project for
//!   `agent` and `hook`, per family for `worker`, per (plugin, project) for `plugin`, and the
//!   per-session (per-call, per-job) capability SIDs the ACL broker grants on their roots
//!   (Decision 16; S9 confirmed that capability SIDs are honoured, so Decision 17's fallback
//!   is not built).
//! - [`dacl`] (pure): whether a DACL already gives a grant's access (`ALL APPLICATION
//!   PACKAGES` on the toolchain roots).
//! - [`grants`] (pure): the ACEs the broker makes for a launch, enumerated against the policy;
//!   [`job_grants`] the same for the job host's `worker` and `plugin` processes.
//! - [`appcontainer`], [`job`], [`desktop`], [`wer`], [`apply`]: the Win32 parts.
//! - [`launcher`]: `fin-sandbox-exec` on Windows (agents and hooks).
//! - [`acl`]: allow-only ACE grants and revokes (the broker's primitives, Task 10).
//! - [`token`]: what this process's token says (the self-sandbox check of workers).

pub mod capability;
pub mod dacl;
pub mod grants;
pub mod job_grants;
pub mod plan;

#[cfg(windows)]
pub mod acl;
#[cfg(windows)]
pub mod appcontainer;
#[cfg(windows)]
pub mod apply;
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod launcher;
#[cfg(windows)]
pub mod sid;
#[cfg(windows)]
pub mod token;
#[cfg(windows)]
pub mod wer;

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

/// `Ok` for a non-zero `BOOL`, else the last OS error.
#[cfg(windows)]
pub(crate) fn check(ok: windows_sys::core::BOOL) -> std::io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Takes ownership of a handle a Win32 call returned (null or `INVALID_HANDLE_VALUE`: the last
/// OS error).
#[cfg(windows)]
pub(crate) fn owned(h: windows_sys::Win32::Foundation::HANDLE) -> std::io::Result<OwnedHandle> {
    if h.is_null() || h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh handle nobody else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
}

/// The raw value of a handle we keep.
#[cfg(windows)]
pub(crate) fn raw(h: &impl AsRawHandle) -> windows_sys::Win32::Foundation::HANDLE {
    h.as_raw_handle() as _
}
