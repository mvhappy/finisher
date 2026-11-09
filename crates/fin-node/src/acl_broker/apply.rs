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
    fn grant(&self, path: &Path, sid: &str, mask: u32, inherit: bool) -> io::Result<()> {
        use fin_sandbox::windows::{acl, sid::Sid};
        acl::grant(path, &Sid::parse(sid)?, mask, inherit)
    }

    fn revoke(&self, path: &Path, sid: &str) -> io::Result<usize> {
        use fin_sandbox::windows::{acl, sid::Sid};
        acl::revoke(path, &Sid::parse(sid)?)
    }
}

/// This OS's layer.
#[cfg(windows)]
pub type Platform = Windows;
/// This OS's layer.
#[cfg(not(windows))]
pub type Platform = Noop;
