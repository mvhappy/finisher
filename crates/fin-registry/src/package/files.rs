//! [`PackageFiles`] over an unpacked package directory.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use fin_plugin_api::PackageFiles;

use super::{Limits, check_path};

/// The files of a package unpacked (or laid out) under a directory. Paths outside the
/// reader's name rules are reported missing, and so are paths spelt otherwise than on disk.
pub struct DirFiles<'a>(pub &'a Path);

impl PackageFiles for DirFiles<'_> {
    fn exists(&self, path: &str) -> bool {
        check_path(path, &Limits::default()).is_ok() && is_exact_file(self.0, path)
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if !self.exists(path) {
            return None;
        }
        fs::read(self.0.join(path)).ok()
    }
}

/// Whether `root/<path>` is a file whose every component is spelt exactly as in `path` (the
/// `/`-separated package path). `Path::is_file` alone answers through a case- or
/// normalisation-insensitive file system: on APFS and NTFS `Styles/Bold.json` finds
/// `styles/bold.json`, on Linux it does not, so a manifest naming it would be valid on one OS
/// and invalid on another (and an executable named that way would be unpacked 0644, as the
/// reader matches executables by exact name).
pub(crate) fn is_exact_file(root: &Path, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let mut at = root.to_path_buf();
    for (i, c) in parts.iter().enumerate() {
        let Ok(entries) = fs::read_dir(&at) else {
            return false;
        };
        if !entries.flatten().any(|e| e.file_name() == OsStr::new(c)) {
            return false;
        }
        at.push(c);
        if i + 1 == parts.len() {
            return at.is_file();
        } else if !at.is_dir() {
            return false;
        }
    }
    false
}
