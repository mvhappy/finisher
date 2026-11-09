//! Where a grant lands: [`Apply`] is the DACL layer under the log. On Windows [`Platform`]
//! is `fin_sandbox::windows::acl` (`SetNamedSecurityInfoW` on allow-only ACEs); elsewhere it
//! changes nothing, so the log, the replay and the sweep are the same code on every OS and are
//! tested on Linux against fakes.

use std::io;
use std::path::Path;

/// Adds and removes one SID's explicit allow ACEs on a path.
pub trait Apply: Send + Sync {
    /// Adds an allow ACE for `sid` with `mask`; `inherit` makes the subtree inherit it.
    fn grant(&self, path: &Path, sid: &str, mask: u32, inherit: bool) -> io::Result<()>;
    /// Removes every explicit allow ACE of `sid`; how many. A missing path is `NotFound`.
    fn revoke(&self, path: &Path, sid: &str) -> io::Result<usize>;
    /// Whether the path's DACL still gives `sid` (or every AppContainer) `mask`, and its
    /// subtree too when `inherit`: the broker reads a static grant back before it skips it as
    /// already done. By default the log is trusted.
    fn holds(&self, _path: &Path, _sid: &str, _mask: u32, _inherit: bool) -> io::Result<bool> {
        Ok(true)
    }
}

/// Off Windows: nothing to apply (Landlock and Seatbelt grant by path at start).
#[derive(Debug, Default, Clone, Copy)]
#[cfg_attr(windows, allow(dead_code))]
pub struct Noop;

impl Apply for Noop {
    fn grant(&self, _path: &Path, _sid: &str, _mask: u32, _inherit: bool) -> io::Result<()> {
        Ok(())
    }

    fn revoke(&self, _path: &Path, _sid: &str) -> io::Result<usize> {
        Ok(0)
    }
}

/// Windows: the DACL itself.
#[cfg(windows)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Windows;

#[cfg(windows)]
impl Apply for Windows {
    /// A read grant on a root whose DACL already admits every AppContainer (`ALL APPLICATION
    /// PACKAGES`, as `C:\Windows\System32` and `%ProgramFiles%` do) or `sid` with `mask` is
    /// done without a change: the user may not re-ACL those roots (no `WRITE_DAC`, review B1).
    /// A root that does not admit containers and cannot be changed is an error naming it.
    fn grant(&self, path: &Path, sid: &str, mask: u32, inherit: bool) -> io::Result<()> {
        use fin_sandbox::windows::{acl, sid::Sid};
        let named = |e: io::Error, why: &str| {
            io::Error::new(e.kind(), format!("{}{why}: {e}", path.display()))
        };
        let sid = Sid::parse(sid)?;
        let write = mask & !acl::READ_EXECUTE != 0;
        if !write && acl::admits(path, &sid, mask, inherit).map_err(|e| named(e, ""))? {
            return Ok(());
        }
        acl::grant(path, &sid, mask, inherit).map_err(|e| {
            let why = if e.kind() == io::ErrorKind::PermissionDenied {
                " does not admit AppContainers and this user may not change its DACL"
            } else {
                ""
            };
            named(e, why)
        })
    }

    fn revoke(&self, path: &Path, sid: &str) -> io::Result<usize> {
        use fin_sandbox::windows::{acl, sid::Sid};
        acl::revoke(path, &Sid::parse(sid)?)
    }

    fn holds(&self, path: &Path, sid: &str, mask: u32, inherit: bool) -> io::Result<bool> {
        use fin_sandbox::windows::{acl, sid::Sid};
        acl::admits(path, &Sid::parse(sid)?, mask, inherit)
    }
}

/// This OS's layer.
#[cfg(windows)]
pub type Platform = Windows;
/// This OS's layer.
#[cfg(not(windows))]
pub type Platform = Noop;
