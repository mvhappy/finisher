//! Paths agents type: a leading `~` or `~/` is expanded to the home directory (fin's job, not
//! the server's or fin-probe's), then the path is made absolute against the working directory.
//! Nothing else is normalised: `..`, symlinks and Unicode are passed on verbatim.

use std::path::{Path, PathBuf};

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

/// `~` expansion, then absolute against the working directory.
pub fn resolve(path: &Path) -> PathBuf {
    let expanded = expand_home(path);
    std::path::absolute(&expanded).unwrap_or(expanded)
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
        assert_eq!(resolve_str("відео/кліп 1.mp4"), cwd.join("відео/кліп 1.mp4"));
    }

    #[test]
    fn absolute_paths_are_kept() {
        #[cfg(unix)]
        assert_eq!(resolve_str("/media/../a.mp4"), PathBuf::from("/media/../a.mp4"));
        #[cfg(windows)]
        {
            assert_eq!(resolve_str(r"C:\media\a.mp4"), PathBuf::from(r"C:\media\a.mp4"));
            assert_eq!(resolve_str(r"\\nas\share\a.mp4"), PathBuf::from(r"\\nas\share\a.mp4"));
        }
    }
}
