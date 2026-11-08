//! `hdiutil` disk images for the macOS tests: a fresh volume of a given format at a private
//! mount point, detached on drop.

use std::fs;
use std::path::{Path, PathBuf};

use super::{run, skip};

/// A disk image attached at a private mount point; detached on drop.
pub struct Image {
    dmg: PathBuf,
    pub mnt: PathBuf,
    attached: bool,
}

impl Image {
    pub fn create(dir: &Path, fs_name: &str) -> Option<Image> {
        Self::create_at(dir, fs_name, &dir.join("m"))
    }

    /// The image kept in `dir`, attached at `mnt` (created when missing).
    pub fn create_at(dir: &Path, fs_name: &str, mnt: &Path) -> Option<Image> {
        let dmg = dir.join("t.dmg");
        run(
            "hdiutil",
            &[
                "create",
                "-quiet",
                "-ov",
                "-size",
                "16m",
                "-fs",
                fs_name,
                "-volname",
                "FinTest",
                dmg.to_str()?,
            ],
        )?;
        fs::create_dir_all(mnt).ok()?;
        let mut img = Image {
            dmg,
            mnt: mnt.to_path_buf(),
            attached: false,
        };
        img.attach().then_some(img)
    }

    pub fn attach(&mut self) -> bool {
        self.attached = run(
            "hdiutil",
            &[
                "attach",
                "-quiet",
                "-nobrowse",
                "-mountpoint",
                self.mnt.to_str().unwrap(),
                self.dmg.to_str().unwrap(),
            ],
        )
        .is_some();
        self.attached
    }

    pub fn detach(&mut self) {
        let m = self.mnt.to_str().unwrap();
        if run("hdiutil", &["detach", "-quiet", m]).is_none() {
            run("hdiutil", &["detach", "-quiet", "-force", m]);
        }
        self.attached = false;
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if self.attached {
            self.detach();
        }
    }
}

pub fn image(test: &str, dir: &Path, fs_name: &str) -> Option<Image> {
    let img = Image::create(dir, fs_name);
    if img.is_none() {
        skip(
            test,
            "hdiutil could not create and attach a disk image here",
        );
    }
    img
}
