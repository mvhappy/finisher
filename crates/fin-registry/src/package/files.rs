//! [`PackageFiles`] over an unpacked package directory.

use std::path::Path;

use fin_plugin_api::PackageFiles;

use super::{Limits, check_path};

/// The files of a package unpacked (or laid out) under a directory. Paths outside the
/// reader's name rules are reported missing.
pub struct DirFiles<'a>(pub &'a Path);

impl PackageFiles for DirFiles<'_> {
    fn exists(&self, path: &str) -> bool {
        check_path(path, &Limits::default()).is_ok() && self.0.join(path).is_file()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if check_path(path, &Limits::default()).is_err() {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }
}
