//! The safe opener (Decision 19, frozen): [`open_for_read`] is **the only open of a file under
//! a library root**, in every process and sub-project (Decision 20).
//!
//! 1. **Re-check without opening** (Linux `statx(AT_SYMLINK_NOFOLLOW | AT_NO_AUTOMOUNT)` from
//!    the parent's descriptor; macOS `getattrlistat(FSOPT_NOFOLLOW)`): a placeholder is
//!    [`OpenError::CloudOnly`] unless `hydrate`, a link [`OpenError::NameSurrogate`], anything
//!    but a regular file [`OpenError::Changed`]. A file that is gone while a Nextcloud,
//!    ownCloud or iCloud stub for it is listed is `CloudOnly` (a stub has no data to hydrate).
//! 2. **Open without following links**, by the raw component chain (`rel`), never a key or a
//!    display string: Linux `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS |
//!    RESOLVE_NO_MAGICLINKS)` from the root's descriptor (with `O_NOATIME` when we own the
//!    file, dropped after one `EPERM`), falling back on `ENOSYS` or `EPERM` (seccomp in
//!    containers) to per-component `openat(O_NOFOLLOW | O_DIRECTORY)`, each component checked
//!    to be a directory; macOS `O_NOFOLLOW_ANY`. **Nested mounts below the root are crossed**:
//!    the walker catalogues them (Decision 17, per directory), so the opened handle must be on
//!    the row's device ([`Expect::check_volume`]: another device is [`OpenError::Changed`]);
//!    a row without a device must be on the root's mount ([`OpenError::Escapes`]).
//! 3. **Identity:** the handle's size, mtime and file id must equal `expect`
//!    ([`Expect::matches`]); a mismatch is [`OpenError::Changed`] (its directory is rescanned),
//!    never a read.
//!
//! **Residual window:** between (1) and (2) a provider may evict a file. On macOS the opened
//! handle's `st_flags` are checked again (`SF_DATALESS` → `CloudOnly`), and with the process's
//! dataless policy off ([`crate::process`]) a read of an evicted file fails `EDEADLK` instead
//! of downloading; on Linux no in-kernel placeholder exists to race.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::raw::RawRel;
use crate::types::{Expect, OpenError};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;

/// Opens the file at `rel` below `root` for reading (Decision 19). `rel` is the catalogued raw
/// chain; `expect` the catalogued size, mtime and file id; `hydrate` the library's setting.
pub fn open_for_read(
    root: &Path,
    rel: &RawRel,
    expect: &Expect,
    hydrate: bool,
) -> Result<File, OpenError> {
    if rel.validate().is_err() {
        return Err(OpenError::Escapes);
    }
    #[cfg(feature = "test-hooks")]
    if let Some(refused) = crate::overlay::on_open(root, rel, hydrate) {
        return Err(refused);
    }
    let Some((name, dir)) = rel.components().split_last() else {
        return Err(OpenError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the root is not a file",
        )));
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        os::open(root, dir, name, expect, hydrate)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (root, dir, name, expect, hydrate);
        Err(OpenError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "this OS's opener is not built yet",
        )))
    }
}

/// The stub names that stand for `name` (Decision 12): `<name>.nextcloud`, `<name>.owncloud`,
/// `.<name>.icloud`.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn stub_names(name: &[u8]) -> [Vec<u8>; 3] {
    [
        [name, b".nextcloud"].concat(),
        [name, b".owncloud"].concat(),
        [b".", name, b".icloud"].concat(),
    ]
}

/// Common `errno` → [`OpenError`] for an open that follows no link.
#[cfg(unix)]
pub(crate) fn map_errno(e: io::Error) -> OpenError {
    match e.raw_os_error() {
        Some(libc::ENOENT) | Some(libc::ENOTDIR) => OpenError::NotFound,
        Some(libc::ELOOP) => OpenError::NameSurrogate,
        Some(libc::EXDEV) => OpenError::Escapes,
        _ => OpenError::Io(e),
    }
}

/// Test hooks (feature `test-hooks`): force the failures a host does not give on demand
/// (`openat2` refused by seccomp or an old kernel, `O_NOATIME` refused for files we do not
/// own) and count the fallbacks taken. Per thread: an open runs on its caller's thread, so
/// parallel tests never see each other's hooks. Without the feature every hook is inert.
#[cfg(feature = "test-hooks")]
pub mod hooks {
    // Each hook applies to the OSes whose opener has that path (`openat2`, `O_NOATIME`: Linux).
    #![allow(dead_code)]

    use std::cell::RefCell;

    /// What to force.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Forced {
        /// Every `openat2` fails with this errno (`ENOSYS`, `EPERM`).
        pub openat2_errno: Option<i32>,
        /// Every open asking for `O_NOATIME` fails `EPERM`.
        pub noatime_eperm: bool,
    }

    /// What happened while the hooks were set.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Counts {
        /// Opens that took the per-component fallback.
        pub fallbacks: u32,
        /// Times `O_NOATIME` was dropped after an `EPERM`.
        pub noatime_drops: u32,
    }

    #[derive(Default)]
    struct State {
        forced: Option<Forced>,
        counts: Counts,
        noatime: Option<bool>,
    }

    thread_local! { static STATE: RefCell<State> = RefCell::default(); }

    /// Runs `f` with `forced` on this thread and returns what happened.
    pub fn with<R>(forced: Forced, f: impl FnOnce() -> R) -> (R, Counts) {
        STATE.with(|s| {
            *s.borrow_mut() = State {
                forced: Some(forced),
                counts: Counts::default(),
                noatime: Some(true),
            }
        });
        let r = f();
        let counts = STATE.with(|s| std::mem::take(&mut *s.borrow_mut()).counts);
        (r, counts)
    }

    pub(crate) fn active() -> bool {
        STATE.with(|s| s.borrow().forced.is_some())
    }

    pub(crate) fn openat2_errno() -> Option<i32> {
        STATE.with(|s| s.borrow().forced.and_then(|f| f.openat2_errno))
    }

    pub(crate) fn noatime_eperm() -> bool {
        STATE.with(|s| s.borrow().forced.is_some_and(|f| f.noatime_eperm))
    }

    pub(crate) fn count(f: impl FnOnce(&mut Counts)) {
        STATE.with(|s| f(&mut s.borrow_mut().counts));
    }

    /// The opener's `O_NOATIME` permission while hooks are set (`None`: not set, use the
    /// process-wide one).
    pub(crate) fn noatime() -> Option<bool> {
        STATE.with(|s| s.borrow().noatime)
    }

    pub(crate) fn set_noatime(v: bool) {
        STATE.with(|s| s.borrow_mut().noatime = Some(v));
    }
}

/// The inert hooks of a build without `test-hooks`.
#[cfg(not(feature = "test-hooks"))]
pub(crate) mod hooks {
    #![allow(dead_code)]

    #[derive(Debug, Clone, Copy, Default)]
    pub(crate) struct Counts {
        pub fallbacks: u32,
        pub noatime_drops: u32,
    }

    pub(crate) fn active() -> bool {
        false
    }

    pub(crate) fn openat2_errno() -> Option<i32> {
        None
    }

    pub(crate) fn noatime_eperm() -> bool {
        false
    }

    pub(crate) fn count(_: impl FnOnce(&mut Counts)) {}

    pub(crate) fn noatime() -> Option<bool> {
        None
    }

    pub(crate) fn set_noatime(_: bool) {}
}
