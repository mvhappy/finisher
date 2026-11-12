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
    #[cfg(any(test, feature = "test-hooks"))]
    if failing(dir) {
        return Err(io::Error::other("injected link-check failure"));
    }
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

/// The real directories directly in `dir` (links are skipped), or none when `dir` itself or a
/// directory above it up to `.finisher` is a link.
pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    if no_links(dir).is_err() {
        return vec![];
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// `.finisher` of `project_dir` is a real directory (not missing, not a link, not a file).
pub(crate) fn dot_is_dir(project_dir: &Path) -> bool {
    std::fs::symlink_metadata(project_dir.join(DOT)).is_ok_and(|m| m.is_dir())
}

#[cfg(any(test, feature = "test-hooks"))]
fn failing_set() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

#[cfg(any(test, feature = "test-hooks"))]
fn failing(dir: &Path) -> bool {
    let set = failing_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set.iter().any(|p| dir.starts_with(p))
}

/// Test hook: [`no_links`] of `dir` or anything below it fails (on any thread) with an error
/// that is not a link, as an `lstat` on a share that drops for a moment, until called again
/// with `false`.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn fail_link_checks_for_test(dir: &Path, fail: bool) {
    let mut set = failing_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if fail {
        set.insert(dir.to_path_buf());
    } else {
        set.remove(dir);
    }
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
