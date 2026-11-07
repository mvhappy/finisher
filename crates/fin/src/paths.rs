//! Paths agents type: a leading `~` or `~/` is expanded to the home directory (fin's job, not
//! the server's or fin-probe's), then the path is made absolute against the working directory
//! and normalised, because the server refuses `.` and `..` segments (`INVALID_PATH`, 5A
//! Decision 35).
//!
//! `..` means what the kernel would make of it: the part before the **last** `..` is resolved
//! through the file system (the deepest ancestor of it that exists is canonicalised, so
//! `link/..` with `link → d/e` is `d`; names below that ancestor do not exist, so they cannot
//! be symlinks and are dropped by `..` as written). Only that prefix is canonicalised: what
//! follows the last `..`, a final symlink included, is passed on verbatim, as is every path
//! without `..`. `.` segments and doubled or trailing separators are removed. A `..` whose
//! prefix cannot be read (not a directory, no permission) is a `USAGE` error
//! ([`resolve_checked`]); paths fin only uses itself fall back to removing `..` lexically
//! ([`resolve`]).

use std::path::{Component, Path, PathBuf};

use fin_core::CoreError;
use serde_json::json;

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

/// `~` expansion, absolute, `..` resolved (module docs): the form fin sends to the server.
pub fn resolve_checked(path: &Path) -> Result<PathBuf, CoreError> {
    let expanded = expand_home(path);
    let abs = std::path::absolute(&expanded).unwrap_or(expanded);
    let parts: Vec<Component> = abs.components().collect();
    let Some(last) = parts.iter().rposition(|c| *c == Component::ParentDir) else {
        return Ok(lexical(abs.components()));
    };
    // The deepest existing ancestor of the prefix before the last `..`, canonicalised.
    let mut k = last;
    let base = loop {
        if k == 0 {
            break PathBuf::new();
        }
        let ancestor: PathBuf = parts[..k].iter().collect();
        let refuse = |why: String| {
            crate::output::usage(
                format!("cannot resolve .. in {}: {why}", abs.display()),
                json!({ "path": abs.display().to_string(), "at": ancestor.display().to_string() }),
                "give the path without .. or through folders fin can read",
            )
        };
        match std::fs::canonicalize(&ancestor) {
            // Something follows it (at least the `..`): it must be a folder.
            Ok(real) if !real.is_dir() => return Err(refuse("not a folder".to_string())),
            Ok(real) => break strip_verbatim(real),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => k -= 1,
            Err(e) => return Err(refuse(e.to_string())),
        }
    };
    Ok(lexical(base.components().chain(parts[k..].iter().copied())))
}

/// [`resolve_checked`], or with `..` removed lexically when it cannot be resolved (paths fin
/// reads or names itself: a socket, server data, a file it canonicalises anyway).
pub fn resolve(path: &Path) -> PathBuf {
    resolve_checked(path).unwrap_or_else(|_| {
        let expanded = expand_home(path);
        lexical(
            std::path::absolute(&expanded)
                .unwrap_or(expanded)
                .components(),
        )
    })
}

/// Rebuilt from components: `.` (except a leading one), doubled and trailing separators are
/// gone; `..` pops the last normal segment (never past the root).
fn lexical<'a>(parts: impl Iterator<Item = Component<'a>>) -> PathBuf {
    let mut out = PathBuf::new();
    for c in parts {
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

/// Windows `canonicalize` answers `\\?\C:\…` / `\\?\UNC\…`: back to `C:\…` / `\\server\…`.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return PathBuf::from(rest);
        }
    }
    path
}

/// [`resolve_checked`] of a string.
pub fn resolve_checked_str(path: &str) -> Result<PathBuf, CoreError> {
    resolve_checked(Path::new(path))
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
        }
        #[cfg(windows)]
        assert_eq!(
            resolve_str(r"C:\media\..\a.mp4"),
            PathBuf::from(r"C:\a.mp4")
        );
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let cwd = strip_verbatim(cwd);
        assert_eq!(resolve_checked_str("./x/../y").unwrap(), cwd.join("y"));
    }

    #[cfg(unix)]
    #[test]
    fn dot_dot_goes_through_symlinks_before_it_only() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(real.join("d/e")).unwrap();
        std::os::unix::fs::symlink(real.join("d/e"), real.join("link")).unwrap();
        // `link/..` is `d`, existing or not after it.
        assert_eq!(resolve(&real.join("link/..")), real.join("d"));
        assert_eq!(
            resolve_checked(&real.join("link/../newproj")).unwrap(),
            real.join("d/newproj")
        );
        assert_eq!(
            resolve_checked(&real.join("link/../../d/e/./x")).unwrap(),
            real.join("d/e/x")
        );
        // Names below the deepest existing ancestor do not exist: `..` drops them.
        assert_eq!(
            resolve_checked(&real.join("new/../p")).unwrap(),
            real.join("p")
        );
        assert_eq!(
            resolve_checked(&real.join("link/new/a/../../q")).unwrap(),
            real.join("d/e/q")
        );
        // A final symlink after the last `..` is passed verbatim.
        assert_eq!(
            resolve_checked(&real.join("d/../link")).unwrap(),
            real.join("link")
        );
        // Without `..`, nothing is canonicalised.
        assert_eq!(
            resolve_checked(&real.join("link/x")).unwrap(),
            real.join("link/x")
        );
        // A prefix that cannot be read is a usage error.
        std::fs::write(real.join("file"), b"x").unwrap();
        let e = resolve_checked(&real.join("file/../x")).unwrap_err();
        assert_eq!(e.code, "USAGE");
        // `resolve` falls back to removing `..` as written.
        assert_eq!(resolve(&real.join("file/../x")), real.join("x"));
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
