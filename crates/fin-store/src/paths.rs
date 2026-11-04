//! Where things live: the server data dir (decision 6), the per-project database and the
//! project folder's `finisher.project.json`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Name of the marker file in the project folder.
pub const PROJECT_FILE: &str = "finisher.project.json";

/// `FIN_SERVER_DATA` if set and non-empty; else macOS `~/Library/Application Support/Finisher`,
/// Linux `${XDG_DATA_HOME:-~/.local/share}/finisher`, Windows `%LOCALAPPDATA%\Finisher`.
pub fn server_data_dir() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    resolve_server_data_dir(
        var("FIN_SERVER_DATA"),
        var("HOME"),
        var("XDG_DATA_HOME"),
        var("LOCALAPPDATA"),
    )
}

/// The pure part of [`server_data_dir`], for tests.
pub(crate) fn resolve_server_data_dir(
    fin: Option<OsString>,
    home: Option<OsString>,
    xdg: Option<OsString>,
    local_app_data: Option<OsString>,
) -> PathBuf {
    if let Some(dir) = fin {
        return PathBuf::from(dir);
    }
    let home = || PathBuf::from(home.clone().unwrap_or_else(|| OsString::from(".")));
    if cfg!(target_os = "macos") {
        home()
            .join("Library")
            .join("Application Support")
            .join("Finisher")
    } else if cfg!(windows) {
        local_app_data
            .map_or_else(|| home().join("AppData").join("Local"), PathBuf::from)
            .join("Finisher")
    } else {
        xdg.map_or_else(|| home().join(".local").join("share"), PathBuf::from)
            .join("finisher")
    }
}

/// `<server_data>/projects/<id>/project.fdb` (never inside the project folder).
pub fn db_path(server_data: &Path, project_id: &str) -> PathBuf {
    server_data
        .join("projects")
        .join(project_id)
        .join("project.fdb")
}

pub fn project_file(project_dir: &Path) -> PathBuf {
    project_dir.join(PROJECT_FILE)
}

/// Writes `bytes` to `path` atomically: a `.~fin-<ulid>.tmp` in the same directory, fsync'd,
/// renamed over `path`, then the directory fsync'd (Unix) so the rename itself is durable.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(crate::names::temp_name());
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        crate::fsio::sync_file(&file)?;
        drop(file);
        fs::rename(&tmp, path)?;
        crate::fsio::sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(OsString::from(s))
    }

    #[test]
    fn fin_server_data_wins() {
        assert_eq!(
            resolve_server_data_dir(os("/srv/fin"), os("/home/u"), os("/x"), os("C:\\L")),
            PathBuf::from("/srv/fin")
        );
    }

    #[test]
    fn platform_default() {
        let got = resolve_server_data_dir(None, os("/home/u"), None, None);
        if cfg!(target_os = "macos") {
            assert_eq!(
                got,
                PathBuf::from("/home/u/Library/Application Support/Finisher")
            );
        } else if cfg!(windows) {
            assert!(got.ends_with("Finisher"));
        } else {
            assert_eq!(got, PathBuf::from("/home/u/.local/share/finisher"));
            assert_eq!(
                resolve_server_data_dir(None, os("/home/u"), os("/xdg"), None),
                PathBuf::from("/xdg/finisher")
            );
        }
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("файл.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, "два".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "два");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
