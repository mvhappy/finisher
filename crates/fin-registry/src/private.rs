//! Owner-only folders and files for private keys (spec §5.6), and the check that refuses a key
//! someone else can reach, as `ssh` refuses an unprotected private key file.
//!
//! - **Unix:** folders are created 0700 and files 0600, each with that mode from creation. A
//!   key folder or key file with any group or other bit is refused (`chmod 700` / `chmod 600`).
//! - **Windows:** folders and files are created with an explicit, protected DACL (nothing is
//!   inherited from the parent) in the same call that creates them, so the bytes are never
//!   reachable through the parent's ACL, not even for a moment:
//!   `O:<user>D:P(A;OICI;FA;;;<user>)(A;OICI;FA;;;SY)` for a folder (its children inherit it),
//!   `O:<user>D:P(A;;FA;;;<user>)(A;;FA;;;SY)` for a file. `SYSTEM` is granted because it is the
//!   OS itself: it can already read any file (backup and take-ownership privileges), so the
//!   grant exposes nothing, while services that run as `SYSTEM` without backup semantics
//!   (Windows Backup and File History through VSS, Defender scans, most backup agents) would
//!   otherwise fail on the key folder. Administrators are not granted (an elevated process can
//!   take ownership anyway, but needs to do so deliberately).
//!
//!   A key folder or key file is refused when its owner is not the current user, `SYSTEM` or
//!   Administrators, its DACL is NULL (anyone, which is also what FAT and exFAT volumes report)
//!   or an allow ACE with any access names another SID. Administrators and `SYSTEM` are
//!   accepted, as OpenSSH for Windows accepts them: they can take ownership of any file, and a
//!   folder under the user's profile lists both. `CREATOR OWNER` and `OWNER RIGHTS` stand for
//!   the owner, who is checked.

use std::fs::File;
use std::io;
use std::path::Path;

/// What a key folder or key file is, for the fix text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Folder,
    File,
}

/// Why a key folder or key file is not private, and the command that makes it so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unprotected {
    /// `"mode 0755"` (Unix), `"owner <sid>"`, `"allows <sid>"`, `"null dacl"`,
    /// `"unsupported ace <type>"` (Windows).
    pub problems: Vec<String>,
    pub fix: String,
}

/// Creates the folder `path` (its parent must exist) owner-only from creation.
pub(crate) fn create_dir(path: &Path) -> io::Result<()> {
    imp::create_dir(path)
}

/// Creates the new file `path` for writing, owner-only from creation; an existing file is an
/// `AlreadyExists` error.
pub(crate) fn create_new(path: &Path) -> io::Result<File> {
    imp::create_new(path)
}

/// `Ok(None)` when only the current user (and the OS) can reach `path`.
pub(crate) fn check(path: &Path, kind: Kind) -> io::Result<Option<Unprotected>> {
    imp::check(path, kind)
}

#[cfg(unix)]
mod imp {
    use std::fs::{self, File};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    use super::{Kind, Unprotected};

    pub fn create_dir(path: &Path) -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(path)
    }

    pub fn create_new(path: &Path) -> io::Result<File> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    pub fn check(path: &Path, kind: Kind) -> io::Result<Option<Unprotected>> {
        let mode = fs::metadata(path)?.permissions().mode() & 0o7777;
        if mode & 0o077 == 0 {
            return Ok(None);
        }
        let want = match kind {
            Kind::Folder => "700",
            Kind::File => "600",
        };
        Ok(Some(Unprotected {
            problems: vec![format!("mode {mode:04o}")],
            fix: format!("chmod {want} '{}'", path.display()),
        }))
    }
}

#[cfg(windows)]
#[path = "private_windows.rs"]
mod imp;

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn unix_modes_from_creation_and_the_check() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let d = tmp.path().join("keys");
        create_dir(&d).unwrap();
        assert_eq!(mode(&d), 0o700);
        let f = d.join("k.pem");
        drop(create_new(&f).unwrap());
        assert_eq!(mode(&f), 0o600);
        assert_eq!(
            create_new(&f).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(check(&d, Kind::Folder).unwrap(), None);
        assert_eq!(check(&f, Kind::File).unwrap(), None);
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o750)).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o604)).unwrap();
        let u = check(&d, Kind::Folder).unwrap().unwrap();
        assert_eq!(u.problems, ["mode 0750"]);
        assert_eq!(u.fix, format!("chmod 700 '{}'", d.display()));
        let u = check(&f, Kind::File).unwrap().unwrap();
        assert_eq!(u.problems, ["mode 0604"]);
        assert_eq!(u.fix, format!("chmod 600 '{}'", f.display()));
    }
}
