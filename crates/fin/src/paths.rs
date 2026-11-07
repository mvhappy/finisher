//! Paths agents type: a leading `~` or `~/` is expanded to the home directory (fin's job, not
//! the server's or fin-probe's), then the path is made absolute against the working directory
//! and normalised, because the server refuses `.` and `..` segments (`INVALID_PATH`, 5A
//! Decision 35): a path with `..` that exists is resolved by the OS (so `link/..` means what
//! the kernel says; on Unix only), otherwise `.`, `..` and doubled separators are removed
//! lexically. Symlinks without `..` and Unicode are passed on verbatim.

use std::path::{Component, Path, PathBuf};

/// The user's home directory: `HOME`, else `USERPROFILE` (Windows).
pub fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Expands a leading `~` component (`~`, `~/x`, and `~\x` on Windows); `~user` is left alone.
pub fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), home()) {
        (Ok(rest), Some(home)) if rest.as_os_str().is_empty() => home,
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// `~` expansion, then absolute against the working directory, then normalised (module docs).
pub fn resolve(path: &Path) -> PathBuf {
    let expanded = expand_home(path);
    normalise(std::path::absolute(&expanded).unwrap_or(expanded))
}

/// `path` without `.`/`..` segments, doubled separators or a trailing separator.
fn normalise(path: PathBuf) -> PathBuf {
    #[cfg(unix)]
    if path.components().any(|c| c == Component::ParentDir)
        && let Ok(real) = std::fs::canonicalize(&path)
    {
        return real;
    }
    // Rebuilt from its components: `.` (except a leading one), doubled and trailing
    // separators are gone; `..` pops the last normal segment (never past the root).
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
}

pub fn resolve_str(path: &str) -> PathBuf {
    resolve(Path::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_to_home_and_others_do_not() {
        let home = home().expect("HOME is set in tests");
        assert_eq!(resolve_str("~"), home);
        assert_eq!(resolve_str("~/clips/a b.mp4"), home.join("clips/a b.mp4"));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_str("~bob/x"), cwd.join("~bob/x"));
        assert_eq!(resolve_str("x/~/y"), cwd.join("x/~/y"));
        assert_eq!(
            resolve_str("відео/кліп 1.mp4"),
            cwd.join("відео/кліп 1.mp4")
        );
    }

    #[test]
    fn dot_segments_are_removed() {
        #[cfg(unix)]
        {
            assert_eq!(resolve_str("/media/../a.mp4"), PathBuf::from("/a.mp4"));
            assert_eq!(resolve_str("/a/./b//c/"), PathBuf::from("/a/b/c"));
            assert_eq!(resolve_str("/../x"), PathBuf::from("/x"));
            let tmp = tempfile::tempdir().unwrap();
            let real = tmp.path().canonicalize().unwrap();
            std::fs::create_dir_all(real.join("d/e")).unwrap();
            std::os::unix::fs::symlink(real.join("d/e"), real.join("link")).unwrap();
            // An existing path: `..` as the kernel resolves it (through the symlink).
            assert_eq!(resolve(&real.join("link/..")), real.join("d"));
            // A missing one: lexically.
            assert_eq!(resolve(&real.join("new/../p")), real.join("p"));
        }
        #[cfg(windows)]
        assert_eq!(
            resolve_str(r"C:\media\..\a.mp4"),
            PathBuf::from(r"C:\a.mp4")
        );
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_str("./x/../y"), cwd.join("y"));
    }

    #[test]
    fn absolute_paths_are_kept() {
        #[cfg(unix)]
        assert_eq!(
            resolve_str("/media/a b.mp4"),
            PathBuf::from("/media/a b.mp4")
        );
        #[cfg(windows)]
        {
            assert_eq!(
                resolve_str(r"C:\media\a.mp4"),
                PathBuf::from(r"C:\media\a.mp4")
            );
            assert_eq!(
                resolve_str(r"\\nas\share\a.mp4"),
                PathBuf::from(r"\\nas\share\a.mp4")
            );
        }
    }
}
