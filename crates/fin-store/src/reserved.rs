//! `.finisher/` and the directories reserved in it (`segments/`, `snapshots/`,
//! `compaction/` and the branch directories under them) are never followed through a link
//! (sub-project 4 spec §3.6.1 "Never followed"). A symlink, or on Windows a junction or any
//! other name-surrogate reparse point, in place of one of them would make scans read history
//! from outside the project and maintenance write or delete there.
//!
//! [`no_links`] is called right before each listing, write and delete, and again after a
//! listing; it checks every directory from `.finisher` down with `symlink_metadata` (which does
//! not follow). Rust's `FileType::is_symlink` is true for symlinks on Unix and for
//! name-surrogate reparse points (symlinks, junctions) on Windows, but not for cloud
//! placeholders, which are reparse points of another kind and stay readable.
//!
//! The check and the operation are separate path lookups: a link swapped in between them is
//! not seen until the next check (module users re-check after listing and after writing).

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The name of the project folder's Finisher directory.
const DOT: &str = ".finisher";

/// A reserved directory that is a link.
#[derive(Debug)]
struct Linked(PathBuf);

impl fmt::Display for Linked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is a symbolic link or junction, and links in .finisher are never followed",
            self.0.display()
        )
    }
}

impl std::error::Error for Linked {}

/// `Ok` when no directory from the last `.finisher` component of `dir` down to `dir` itself is
/// a link (all of `dir` when it has no such component). A missing directory ends the check:
/// nothing below it exists. Any other `symlink_metadata` failure is returned.
pub(crate) fn no_links(dir: &Path) -> io::Result<()> {
    let comps: Vec<Component<'_>> = dir.components().collect();
    let start = comps
        .iter()
        .rposition(|c| c.as_os_str() == DOT)
        .unwrap_or(comps.len().saturating_sub(1));
    let mut path: PathBuf = comps[..start].iter().collect();
    for c in &comps[start..] {
        path.push(c);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::other(Linked(path)));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Whether `e` is [`no_links`] refusing a link.
pub(crate) fn is_linked(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|i| i.is::<Linked>())
}

/// `.finisher` of `project_dir` is a real directory (not missing, not a link, not a file).
pub(crate) fn dot_is_dir(project_dir: &Path) -> bool {
    std::fs::symlink_metadata(project_dir.join(DOT)).is_ok_and(|m| m.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn checks_from_dot_down() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // A link above `.finisher` is the person's choice of project path.
        let project = d.path().join("project");
        std::os::unix::fs::symlink(outside.path(), &project).unwrap();
        let dot = project.join(DOT);
        std::fs::create_dir_all(dot.join("segments")).unwrap();
        assert!(no_links(&dot.join("segments/main")).is_ok());
        assert!(dot_is_dir(&project));
        let real = outside.path().join("elsewhere");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dot.join("segments/main")).unwrap();
        let e = no_links(&dot.join("segments/main")).unwrap_err();
        assert!(is_linked(&e), "{e}");
        assert!(e.to_string().contains("segments/main"), "{e}");
        assert!(!is_linked(&io::Error::other("x")));
    }
}
