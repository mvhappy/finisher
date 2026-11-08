//! The process's dataless policy (Decision 6, frozen): **every process that may open a
//! library file calls [`deny_dataless_materialisation`] first**, so on macOS an accidental
//! access to a dataless file or directory fails `EDEADLK` instead of making the File Provider
//! download it. Only a cold job of a `hydrate: true` library calls
//! [`allow_dataless_materialisation`], for its own process.
//!
//! macOS: `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_PROCESS,
//! IOPOL_MATERIALIZE_DATALESS_FILES_OFF | _ON)` (declared here: `libc` 0.2.191 has neither the
//! function nor the constants). Elsewhere there is no such policy and both calls succeed.

use std::io;

#[cfg(target_os = "macos")]
mod mac {
    use libc::c_int;

    pub const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
    pub const IOPOL_SCOPE_PROCESS: c_int = 0;
    pub const IOPOL_MATERIALIZE_DATALESS_FILES_DEFAULT: c_int = 0;
    pub const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;
    pub const IOPOL_MATERIALIZE_DATALESS_FILES_ON: c_int = 2;

    unsafe extern "C" {
        pub fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
        pub fn getiopolicy_np(iotype: c_int, scope: c_int) -> c_int;
    }

    pub fn set(policy: c_int) -> std::io::Result<()> {
        // SAFETY: a plain libc call with constant arguments.
        let r = unsafe {
            setiopolicy_np(
                IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                IOPOL_SCOPE_PROCESS,
                policy,
            )
        };
        if r < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// The process's materialisation policy as the OS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatalessPolicy {
    /// The OS default (macOS materialises on access).
    Default,
    /// Access to dataless items fails (`EDEADLK`).
    Off,
    /// Access materialises.
    On,
    /// This OS has no such policy.
    NotApplicable,
}

/// Turns dataless materialisation off for the whole process (Decision 6).
pub fn deny_dataless_materialisation() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        mac::set(mac::IOPOL_MATERIALIZE_DATALESS_FILES_OFF)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// Turns it back on (a cold job of a `hydrate: true` library only).
pub fn allow_dataless_materialisation() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        mac::set(mac::IOPOL_MATERIALIZE_DATALESS_FILES_ON)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// Reads the policy back (`getiopolicy_np` on macOS).
pub fn dataless_policy() -> io::Result<DatalessPolicy> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a plain libc call with constant arguments.
        let r = unsafe {
            mac::getiopolicy_np(
                mac::IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                mac::IOPOL_SCOPE_PROCESS,
            )
        };
        match r {
            mac::IOPOL_MATERIALIZE_DATALESS_FILES_DEFAULT => Ok(DatalessPolicy::Default),
            mac::IOPOL_MATERIALIZE_DATALESS_FILES_OFF => Ok(DatalessPolicy::Off),
            mac::IOPOL_MATERIALIZE_DATALESS_FILES_ON => Ok(DatalessPolicy::On),
            r if r < 0 => Err(io::Error::last_os_error()),
            r => Err(io::Error::other(format!("unknown dataless policy {r}"))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(DatalessPolicy::NotApplicable)
    }
}
