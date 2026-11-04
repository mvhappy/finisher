//! Project copies (plan Decision 6): the database records the folder it belongs to
//! (`meta.projectDir`). Opening a folder whose project id this server data already knows from
//! a *different, existing* folder that still holds the same project is `PROJECT_COPY`: a copy
//! of a project folder must never share the original's database. If the recorded folder no
//! longer holds the project (deleted, renamed, unmounted), the project moved, and
//! `projectDir` is updated.
//!
//! Two spellings of one folder (a symlink, a second mount of the same share) are the same
//! folder when they canonicalise alike or, on Unix, when their project files are the same
//! file (device and inode).
//!
//! `projectDir` is JSON, lossless for any OS path: `{"utf8": "<path>"}`, or for a path that is
//! not valid Unicode `{"unix": "<hex bytes>"}` / `{"wide": "<hex of UTF-16 units>"}`.
//!
//! Known limit: a folder on a share that is unmounted while its copy is opened counts as
//! moved; once the share is back, opening the original is then refused as the copy.

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::{StoreError, db, project_file};

const KEY: &str = "projectDir";

/// The folder as recorded: canonical while it exists, else absolute.
fn canonical(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir)
        .or_else(|_| std::path::absolute(dir))
        .unwrap_or_else(|_| dir.to_path_buf())
}

pub(crate) fn encode(path: &Path) -> String {
    let value = match path.to_str() {
        Some(s) => json!({ "utf8": s }),
        None => raw(path),
    };
    value.to_string()
}

#[cfg(unix)]
fn raw(path: &Path) -> Value {
    use std::os::unix::ffi::OsStrExt;
    json!({ "unix": hex(path.as_os_str().as_bytes()) })
}

#[cfg(windows)]
fn raw(path: &Path) -> Value {
    use std::os::windows::ffi::OsStrExt;
    let bytes: Vec<u8> = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    json!({ "wide": hex(&bytes) })
}

#[cfg(not(any(unix, windows)))]
fn raw(path: &Path) -> Value {
    json!({ "utf8": path.to_string_lossy() })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2))
        .then(|| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
                .collect()
        })
        .flatten()
}

pub(crate) fn decode(s: &str) -> Option<PathBuf> {
    let v: Value = serde_json::from_str(s).ok()?;
    if let Some(p) = v.get("utf8").and_then(Value::as_str) {
        return Some(PathBuf::from(p));
    }
    #[cfg(unix)]
    if let Some(h) = v.get("unix").and_then(Value::as_str) {
        use std::os::unix::ffi::OsStringExt;
        return Some(PathBuf::from(std::ffi::OsString::from_vec(unhex(h)?)));
    }
    #[cfg(windows)]
    if let Some(h) = v.get("wide").and_then(Value::as_str) {
        use std::os::windows::ffi::OsStringExt;
        let units: Vec<u16> = unhex(h)?
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return Some(PathBuf::from(std::ffi::OsString::from_wide(&units)));
    }
    let _ = unhex;
    None
}

/// Whether `a` and `b` are one folder.
fn same_folder(a: &Path, b: &Path) -> bool {
    if canonical(a) == canonical(b) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let id = |d: &Path| {
            std::fs::metadata(crate::project_file(d))
                .ok()
                .map(|m| (m.dev(), m.ino()))
        };
        if let (Some(x), Some(y)) = (id(a), id(b)) {
            return x == y;
        }
    }
    false
}

/// Refuses `project_dir` if the database belongs to another existing copy of the project
/// (module docs). Reads only.
pub(crate) fn check(conn: &Connection, project_dir: &Path, id: &str) -> Result<(), StoreError> {
    let Some(stored) = db::meta(conn, KEY)? else {
        return Ok(());
    };
    let Some(recorded) = decode(&stored) else {
        return Ok(());
    };
    if same_folder(&recorded, project_dir) {
        return Ok(());
    }
    let still_there = project_file::read_project_file_once(&recorded).is_ok_and(|f| f.id == id);
    if !still_there {
        return Ok(()); // moved
    }
    let here = canonical(project_dir);
    Err(CoreError::new(
        "PROJECT_COPY",
        format!(
            "{} is a copy of the project at {}, which this server already uses",
            here.display(),
            recorded.display()
        ),
    )
    .with_details(json!({
        "projectId": id,
        "path": here.to_string_lossy(),
        "original": recorded.to_string_lossy(),
    }))
    .with_fix("open the original folder; to keep the copy as a separate project, create a new project and import its media")
    .into())
}

/// Records `project_dir` as the project's folder (after [`check`] passed).
pub(crate) fn record(conn: &Connection, project_dir: &Path) -> Result<(), StoreError> {
    db::set_meta(conn, KEY, &encode(&canonical(project_dir)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip() {
        let p = PathBuf::from("/Volumes/NAS/Мой проект 1");
        assert_eq!(decode(&encode(&p)), Some(p));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_round_trip() {
        use std::os::unix::ffi::OsStringExt;
        let p = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/\xff\xfe x".to_vec()));
        let e = encode(&p);
        assert!(e.contains("unix"), "{e}");
        assert_eq!(decode(&e), Some(p));
    }
}
