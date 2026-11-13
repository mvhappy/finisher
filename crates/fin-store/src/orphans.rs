//! Databases a crashed create left in server data (`Store::create` steps 3–4).
//!
//! A create works in a fresh `<server-data>/projects/<id>/`. Before it builds the database it
//! writes [`MARKER`] there, naming the project folder; once its project file is in the folder
//! (the commit point) it removes the marker. A crash in between leaves a database no project
//! file names. Nothing would ever open it, so [`sweep`] removes it.
//!
//! **Only a directory still holding the marker is ever looked at**, and it is removed only
//! when all of these hold:
//! - the marker is older than [`MIN_AGE`] (local clock against a local file: server data is on
//!   a local disk), so a create still running in another process is never touched;
//! - the database's lock (`db.lock`) can be taken exclusively: no store has it open;
//! - the marker names a project folder that is present and shows no project file and no
//!   `.finisher/` (the create never reached its commit point), or whose project file names
//!   another project (another create won the folder).
//!
//! A folder that is missing or cannot be read (an unmounted share, an offline drive, a typo
//! of the user's later) proves nothing: the directory is kept, however old. A folder whose
//! project file names this id is a real project whose create crashed after its commit point:
//! only the marker is removed. A database without a marker (every project that was ever
//! created, opened or rebuilt) is never touched.
//!
//! The sweep runs once per process and server data, on the first create or open
//! ([`sweep_once`]), and can be run directly ([`sweep`]).

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::{PROJECT_FILE, dblock, fsio, project_file as pf};

/// The marker of a create in progress, in `<server-data>/projects/<id>/`: the project folder's
/// absolute path, UTF-8 (a folder whose path is not UTF-8 gets an empty marker, never swept).
pub(crate) const MARKER: &str = "create.pending";

/// A marker younger than this belongs to a create that may still be running.
pub const MIN_AGE: Duration = Duration::from_secs(60);

/// Writes the marker for a create of `project_dir` into `project_data` (step 3 of create).
pub(crate) fn mark(project_data: &Path, project_dir: &Path) -> std::io::Result<()> {
    let abs = std::path::absolute(project_dir)?;
    let text = abs.to_str().unwrap_or_default();
    fsio::write_once(project_data, MARKER, text.as_bytes())
}

/// Removes the marker once the project file is in place (best effort: a marker left behind is
/// removed by the sweep, which finds the project file naming this id).
pub(crate) fn unmark(project_data: &Path) {
    let _ = fs::remove_file(project_data.join(MARKER));
}

/// [`sweep`] once per process for `server_data` (module docs). Never fails the caller.
pub(crate) fn sweep_once(server_data: &Path) {
    static DONE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let key = fs::canonicalize(server_data).unwrap_or_else(|_| server_data.to_path_buf());
    let first = DONE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key);
    if first {
        sweep(server_data, MIN_AGE);
    }
}

/// Removes the orphan databases of crashed creates in `server_data` (module docs) whose marker
/// is at least `min_age` old; returns their project ids.
pub fn sweep(server_data: &Path, min_age: Duration) -> Vec<String> {
    let Ok(entries) = fs::read_dir(server_data.join("projects")) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if sweep_one(&dir, &id, min_age) {
            removed.push(id);
        }
    }
    removed.sort();
    removed
}

/// What the marker's folder says about project `id`.
enum Folder {
    /// The project file names `id`: a real project.
    Ours,
    /// The create never reached its commit point there.
    Orphan,
    /// Missing or unreadable: proves nothing.
    Unknown,
}

fn sweep_one(dir: &Path, id: &str, min_age: Duration) -> bool {
    let marker = dir.join(MARKER);
    let Ok(meta) = fs::symlink_metadata(&marker) else {
        return false;
    };
    let old = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age >= min_age);
    if !old || !meta.is_file() {
        return false;
    }
    let Ok(lock) = dblock::exclusive_now(dir) else {
        return false;
    };
    let folder = match fs::read_to_string(&marker) {
        Ok(text) if !text.is_empty() => look(Path::new(&text), id),
        _ => Folder::Unknown,
    };
    match folder {
        Folder::Ours => {
            drop(lock);
            unmark(dir);
            false
        }
        Folder::Unknown => false,
        Folder::Orphan => remove_locked(dir, lock),
    }
}

/// Removes an orphan's directory (a fresh id's: everything in it is that create's) while
/// holding its exclusive `db.lock`, so no store can open the database halfway through. The
/// lock file itself goes last, after the lock is let go: Windows cannot delete a file that is
/// open. Nothing can open a database in between: no project file names this id.
fn remove_locked(dir: &Path, lock: fs::File) -> bool {
    let lock_name = std::ffi::OsStr::new(dblock::FILE);
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for e in entries.flatten() {
        if e.file_name() == lock_name {
            continue;
        }
        let p = e.path();
        let gone = match e.file_type() {
            Ok(t) if t.is_dir() => fs::remove_dir_all(&p),
            _ => fs::remove_file(&p),
        };
        if gone.is_err() {
            return false;
        }
    }
    drop(lock);
    fs::remove_dir_all(dir).is_ok()
}

fn look(project_dir: &Path, id: &str) -> Folder {
    match fs::symlink_metadata(project_dir) {
        Ok(m) if m.is_dir() => {}
        _ => return Folder::Unknown,
    }
    match fs::symlink_metadata(project_dir.join(PROJECT_FILE)) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // `.finisher/` is made only after the project file: with it present, the file
            // may merely be late to show (a Windows not-found cache).
            match fs::symlink_metadata(project_dir.join(".finisher")) {
                Err(e) if e.kind() == ErrorKind::NotFound => Folder::Orphan,
                _ => Folder::Unknown,
            }
        }
        Err(_) => Folder::Unknown,
        Ok(_) => match pf::read_project_file_once(project_dir) {
            Ok(f) if f.id == id => Folder::Ours,
            Ok(_) => Folder::Orphan,
            Err(_) => Folder::Unknown,
        },
    }
}
