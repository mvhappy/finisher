//! Project copies (plan Decision 6): the database records the folder it belongs to
//! (`meta.projectDir`). Opening a folder whose project id this server data already knows from
//! a *different, existing* folder that still holds the same project is `PROJECT_COPY`: a copy
//! of a project folder must never share the original's database.
//!
//! The recorded folder no longer holds the project: it **moved** (and `projectDir` is updated)
//! only if both
//! - its absence can be trusted: the nearest existing ancestor is not a mount root, a mount
//!   base or an empty mount point right below one, and can be listed (so it is not an
//!   unmounted share or a disconnected drive; a renamed parent folder is a move), and
//! - the folder being opened holds, per branch, the newest segment the database knows from
//!   the folder (so it is not an older copy).
//!
//! Otherwise `PROJECT_ORIGINAL_UNREACHABLE`, unless the caller says the project moved
//! ([`OpenOptions::relocate`](crate::OpenOptions::relocate)).
//!
//! Two spellings of one folder (a symlink, a second mount of the same share) are the same
//! folder when they canonicalise alike or the project *directories* are one directory
//! (`identity::dir_identity`). Files are not compared: a hard-linked copy (`cp -al`,
//! rsnapshot, `rsync --link-dest`) shares them.
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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
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
    // The directories themselves: a hard-linked copy shares the files, never the directories.
    let id = crate::identity::dir_identity;
    matches!((id(a), id(b)), (Some(x), Some(y)) if x == y)
}

/// Whether `dir` holds, per branch, the newest segment the database knows from the folder.
/// A database without any (nothing exported yet) cannot tell an old copy apart: yes.
fn holds_newest_segments(conn: &Connection, dir: &Path) -> bool {
    let newest = conn
        .prepare(
            "SELECT branch, name FROM segments s WHERE state IN ('exported', 'imported')
             AND last_seq = (SELECT MAX(last_seq) FROM segments t
                             WHERE t.branch = s.branch AND t.state IN ('exported', 'imported'))",
        )
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()
        });
    let Ok(newest) = newest else {
        return false;
    };
    let segments = dir.join(".finisher").join("segments");
    // Per branch, any of the rows ending at its newest seq (a segment or the pack replacing
    // it) will do.
    let mut by_branch: std::collections::BTreeMap<&str, bool> = Default::default();
    for (branch, name) in &newest {
        // Never through a link (`crate::reserved`).
        let dir = segments.join(branch);
        let present = crate::reserved::no_links(&dir).is_ok()
            && std::fs::symlink_metadata(dir.join(name)).is_ok_and(|m| m.is_file());
        *by_branch.entry(branch).or_default() |= present;
    }
    by_branch.values().all(|p| *p)
}

/// Directories a share is typically mounted under: a missing folder right below one is an
/// unmounted share.
const MOUNT_BASES: [&str; 6] = [
    "/Volumes",
    "/mnt",
    "/media",
    "/run/media",
    "/net",
    "/Network",
];

/// Whether `dir` is a mount root: its device differs from its parent's, or it has no parent
/// (`/`, a drive root `C:\`, a share root `\\host\share\`).
fn is_mount_root(dir: &Path) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    if MOUNT_ROOTS.with(|m| m.borrow().contains(&canonical(dir))) {
        return true;
    }
    let Some(parent) = dir.parent() else {
        return true;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(dir), std::fs::metadata(parent)) {
            return a.dev() != b.dev();
        }
    }
    let _ = parent;
    false
}

/// Whether the absence of `recorded` (a folder that no longer holds the project) can be
/// trusted: walk up to the nearest ancestor that exists. It is unreachable (perhaps an
/// unmounted share, a disconnected drive) when that ancestor is a mount root, a mount base
/// (`/Volumes`, `/mnt`, …) or right below one (an empty mount point), or cannot be listed;
/// otherwise the folder (or a parent of it) was renamed or deleted, so the project moved.
fn reachable(recorded: &Path) -> bool {
    let Some(ancestor) = recorded.ancestors().skip(1).find(|a| a.exists()) else {
        return false; // nothing of the path exists: a missing drive or volume
    };
    if std::fs::read_dir(ancestor).is_err() || is_mount_root(ancestor) {
        return false;
    }
    let base = |p: &Path| MOUNT_BASES.iter().any(|b| p == Path::new(b));
    !(base(ancestor) || ancestor.parent().is_some_and(base))
}

/// Refuses `project_dir` if the database belongs to another copy of the project, or to a
/// folder that cannot be reached to tell (module docs). `relocate`: the caller says the project
/// moved here. Reads only.
pub(crate) fn check(
    conn: &Connection,
    project_dir: &Path,
    id: &str,
    relocate: bool,
) -> Result<(), StoreError> {
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
    let here = canonical(project_dir);
    if !still_there {
        let reachable = reachable(&recorded);
        if relocate || (reachable && holds_newest_segments(conn, project_dir)) {
            return Ok(()); // moved
        }
        return Err(CoreError::new(
            "PROJECT_ORIGINAL_UNREACHABLE",
            format!(
                "this server uses project {id} at {}, which cannot be reached, and {} may be an older copy",
                recorded.display(),
                here.display()
            ),
        )
        .with_details(json!({
            "projectId": id,
            "path": here.to_string_lossy(),
            "original": recorded.to_string_lossy(),
            "originalParentReachable": reachable,
        }))
        .with_fix("connect the drive or share that holds the original folder and open it there; if the project really moved here, open this folder with relocate (Store OpenOptions::relocate; the fin and RPC option comes with plan Task 15)")
        .into());
    }
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

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static MOUNT_ROOTS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test hook: on this thread, `dir` counts as a mount root (a share's mount base).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn mount_root_for_test(dir: &Path) {
    MOUNT_ROOTS.with(|m| m.borrow_mut().push(canonical(dir)));
}
